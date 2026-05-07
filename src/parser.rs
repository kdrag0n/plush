use crate::error::{PlushError, Result};
use brush_parser::{Parser, ParserOptions};
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub pipeline: Pipeline,
    pub connector: Connector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    Always,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<String>,
    pub assignments: Vec<(String, String)>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    Read {
        fd: i32,
        target: String,
    },
    Write {
        fd: i32,
        target: String,
        append: bool,
    },
    Duplicate {
        fd: i32,
        target: i32,
    },
    Close {
        fd: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    AndIf,
    OrIf,
    Semi,
    Amp,
    Redirect(String),
}

pub fn parse(input: &str) -> Result<Script> {
    validate_with_brush(input)?;
    parse_for_exec(input)
}

pub fn validate_with_brush(input: &str) -> Result<()> {
    if input.trim().is_empty() {
        return Ok(());
    }
    let mut parser = Parser::new(Cursor::new(input), &ParserOptions::default());
    parser
        .parse_program()
        .map(|_| ())
        .map_err(|err| PlushError::Syntax(pretty_parse_error(input, &err.to_string())))
}

fn pretty_parse_error(input: &str, raw: &str) -> String {
    let mut message = raw.replace("Parsing error", "invalid command");
    if let Some((line, col)) = parse_line_col(raw) {
        let source_line = input.lines().nth(line.saturating_sub(1)).unwrap_or(input);
        message.push('\n');
        message.push_str(source_line);
        message.push('\n');
        message.push_str(&" ".repeat(col.saturating_sub(1)));
        message.push('^');
    }
    message
}

fn parse_line_col(raw: &str) -> Option<(usize, usize)> {
    let marker = "line ";
    let line_start = raw.find(marker)? + marker.len();
    let line_end = raw[line_start..].find(' ')? + line_start;
    let line = raw[line_start..line_end].parse().ok()?;
    let col_marker = "col ";
    let col_start = raw.find(col_marker)? + col_marker.len();
    let col = raw[col_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    Some((line, col))
}

fn parse_for_exec(input: &str) -> Result<Script> {
    let tokens = tokenize(input)?;
    let mut p = ExecParser { tokens, pos: 0 };
    p.script()
}

struct ExecParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl ExecParser {
    fn script(&mut self) -> Result<Script> {
        let mut items = Vec::new();
        let mut connector = Connector::Always;

        while !self.is_eof() {
            self.eat_semis();
            if self.is_eof() {
                break;
            }

            let pipeline = self.pipeline()?;
            items.push(ListItem {
                pipeline,
                connector,
            });

            connector = match self.peek() {
                Some(Token::AndIf) => {
                    self.pos += 1;
                    Connector::And
                }
                Some(Token::OrIf) => {
                    self.pos += 1;
                    Connector::Or
                }
                Some(Token::Semi) => {
                    self.pos += 1;
                    Connector::Always
                }
                None => Connector::Always,
                other => {
                    return Err(PlushError::Syntax(format!(
                        "unexpected token after command: {other:?}"
                    )));
                }
            };
        }

        Ok(Script { items })
    }

    fn pipeline(&mut self) -> Result<Pipeline> {
        let mut commands = vec![self.command()?];
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.pos += 1;
            commands.push(self.command()?);
        }
        let background = if matches!(self.peek(), Some(Token::Amp)) {
            self.pos += 1;
            true
        } else {
            false
        };
        Ok(Pipeline {
            commands,
            background,
        })
    }

    fn command(&mut self) -> Result<Command> {
        let mut words = Vec::new();
        let mut redirects = Vec::new();
        let mut assignments = Vec::new();

        loop {
            match self.peek().cloned() {
                Some(Token::Word(word)) => {
                    self.pos += 1;
                    if words.is_empty() {
                        if let Some((name, value)) = split_assignment(&word) {
                            assignments.push((name.to_string(), value.to_string()));
                            continue;
                        }
                    }
                    words.push(word);
                }
                Some(Token::Redirect(op)) => {
                    self.pos += 1;
                    redirects.push(self.redirect(&op)?);
                }
                _ => break,
            }
        }

        if words.is_empty() && assignments.is_empty() && redirects.is_empty() {
            return Err(PlushError::Syntax("expected command".to_string()));
        }

        Ok(Command {
            words,
            assignments,
            redirects,
        })
    }

    fn redirect(&mut self, op: &str) -> Result<Redirect> {
        let target = match self.next_word() {
            Some(target) => target,
            None => return Err(PlushError::Syntax(format!("{op} requires a target"))),
        };

        let (fd, kind) = parse_redirect_op(op);
        match kind {
            RedirectKind::Read => Ok(Redirect::Read { fd, target }),
            RedirectKind::Write => Ok(Redirect::Write {
                fd,
                target,
                append: false,
            }),
            RedirectKind::Append => Ok(Redirect::Write {
                fd,
                target,
                append: true,
            }),
            RedirectKind::Duplicate => {
                if target == "-" {
                    Ok(Redirect::Close { fd })
                } else {
                    let target_fd = target.parse::<i32>().map_err(|_| {
                        PlushError::Syntax(format!("bad file descriptor: {target}"))
                    })?;
                    Ok(Redirect::Duplicate {
                        fd,
                        target: target_fd,
                    })
                }
            }
        }
    }

    fn eat_semis(&mut self) {
        while matches!(self.peek(), Some(Token::Semi)) {
            self.pos += 1;
        }
    }

    fn next_word(&mut self) -> Option<String> {
        match self.peek().cloned() {
            Some(Token::Word(word)) => {
                self.pos += 1;
                Some(word)
            }
            _ => None,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }
}

#[derive(Debug, Clone, Copy)]
enum RedirectKind {
    Read,
    Write,
    Append,
    Duplicate,
}

fn parse_redirect_op(op: &str) -> (i32, RedirectKind) {
    let split = op.find(|c: char| !c.is_ascii_digit()).unwrap_or(op.len());
    let fd = if split == 0 {
        if op.starts_with('<') { 0 } else { 1 }
    } else {
        op[..split].parse::<i32>().unwrap_or(1)
    };
    let body = &op[split..];
    let kind = match body {
        "<" => RedirectKind::Read,
        ">" | ">|" => RedirectKind::Write,
        ">>" => RedirectKind::Append,
        "<&" | ">&" => RedirectKind::Duplicate,
        _ => RedirectKind::Write,
    };
    (fd, kind)
}

fn split_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    if name.is_empty() {
        return None;
    }
    let mut chars = name.chars();
    if !matches!(chars.next(), Some('_' | 'a'..='z' | 'A'..='Z')) {
        return None;
    }
    if chars.all(|c| matches!(c, '_' | 'a'..='z' | 'A'..='Z' | '0'..='9')) {
        Some((name, value))
    } else {
        None
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' && at_word_boundary(&out) {
            break;
        }
        match c {
            ';' => {
                out.push(Token::Semi);
                i += 1;
            }
            '|' if bytes.get(i + 1) == Some(&b'|') => {
                out.push(Token::OrIf);
                i += 2;
            }
            '|' => {
                out.push(Token::Pipe);
                i += 1;
            }
            '&' if bytes.get(i + 1) == Some(&b'&') => {
                out.push(Token::AndIf);
                i += 2;
            }
            '&' => {
                out.push(Token::Amp);
                i += 1;
            }
            '<' | '>' => {
                let start = i;
                i += 1;
                if bytes.get(i) == Some(&bytes[start])
                    || bytes.get(i) == Some(&b'&')
                    || bytes.get(i) == Some(&b'|')
                {
                    i += 1;
                }
                out.push(Token::Redirect(input[start..i].to_string()));
            }
            '0'..='9' => {
                if let Some(end) = redirect_after_fd(input, i) {
                    out.push(Token::Redirect(input[i..end].to_string()));
                    i = end;
                } else {
                    let (word, end) = read_word(input, i)?;
                    out.push(Token::Word(word));
                    i = end;
                }
            }
            _ => {
                let (word, end) = read_word(input, i)?;
                out.push(Token::Word(word));
                i = end;
            }
        }
    }
    Ok(out)
}

fn at_word_boundary(tokens: &[Token]) -> bool {
    tokens.is_empty()
        || matches!(
            tokens.last(),
            Some(Token::Semi | Token::AndIf | Token::OrIf | Token::Pipe | Token::Amp)
        )
}

fn redirect_after_fd(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut i = start;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    match bytes.get(i).copied() {
        Some(b'<') | Some(b'>') => {
            i += 1;
            if matches!(
                bytes.get(i),
                Some(b'<') | Some(b'>') | Some(b'&') | Some(b'|')
            ) {
                i += 1;
            }
            Some(i)
        }
        _ => None,
    }
}

fn read_word(input: &str, start: usize) -> Result<(String, usize)> {
    let bytes = input.as_bytes();
    let mut word = String::new();
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>') {
            break;
        }
        match c {
            '\'' => {
                word.push('\'');
                i += 1;
                let quote_start = i;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(PlushError::Syntax("unterminated single quote".to_string()));
                }
                word.push_str(&input[quote_start..i]);
                word.push('\'');
                i += 1;
            }
            '"' => {
                word.push('"');
                i += 1;
                while i < bytes.len() {
                    let ch = bytes[i] as char;
                    if ch == '"' {
                        word.push('"');
                        i += 1;
                        break;
                    }
                    if ch == '\\' {
                        if let Some(next) = bytes.get(i + 1) {
                            word.push('\\');
                            word.push(*next as char);
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else {
                        word.push(ch);
                        i += 1;
                    }
                }
                if !word.ends_with('"') {
                    return Err(PlushError::Syntax("unterminated double quote".to_string()));
                }
            }
            '\\' => {
                if let Some(next) = bytes.get(i + 1) {
                    word.push(*next as char);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                word.push(c);
                i += 1;
            }
        }
    }
    Ok((word, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pipeline_and_connectors() {
        let script = parse_for_exec("echo hi | wc -c && false || echo ok").unwrap();
        assert_eq!(script.items.len(), 3);
        assert_eq!(script.items[0].pipeline.commands.len(), 2);
        assert_eq!(script.items[1].connector, Connector::And);
        assert_eq!(script.items[2].connector, Connector::Or);
    }

    #[test]
    fn parses_assignment_and_redirect() {
        let script = parse_for_exec("A=b printf hi >out").unwrap();
        let cmd = &script.items[0].pipeline.commands[0];
        assert_eq!(cmd.assignments, vec![("A".to_string(), "b".to_string())]);
        assert!(matches!(cmd.redirects[0], Redirect::Write { .. }));
    }

    #[test]
    fn validates_bash_syntax_with_brush() {
        assert!(validate_with_brush("if true; then echo ok; fi").is_ok());
        assert!(validate_with_brush("echo )").is_err());
        let err = validate_with_brush("echo )").unwrap_err().to_string();
        assert!(err.contains("echo )"));
        assert!(err.contains("^"));
    }
}
