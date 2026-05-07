use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText, ValidationResult, Validator};
use std::sync::Mutex;
use tree_sitter::{Node, Parser};

const MAX_HIGHLIGHT_BYTES: usize = 128 * 1024;

pub struct BashHighlighter {
    parser: Mutex<Parser>,
    max_bytes: usize,
}

impl BashHighlighter {
    pub fn new(max_bytes: usize) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("tree-sitter bash grammar should load");
        Self {
            parser: Mutex::new(parser),
            max_bytes,
        }
    }
}

impl Default for BashHighlighter {
    fn default() -> Self {
        Self::new(MAX_HIGHLIGHT_BYTES)
    }
}

impl Highlighter for BashHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        if line.len() > self.max_bytes {
            let mut text = StyledText::new();
            text.push((paste_style(), line.to_string()));
            return text;
        }

        let mut text = StyledText::new();
        text.push((Style::new(), line.to_string()));

        let Ok(mut parser) = self.parser.lock() else {
            return text;
        };
        let Some(tree) = parser.parse(line, None) else {
            return text;
        };
        style_node(tree.root_node(), &mut text, line);
        text
    }
}

pub struct BashValidator {
    max_bytes: usize,
}

impl BashValidator {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}

impl Validator for BashValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if line.len() > self.max_bytes {
            return ValidationResult::Complete;
        }
        match crate::parser::validate_with_brush(line) {
            Ok(()) => ValidationResult::Complete,
            Err(_) if likely_incomplete(line) => ValidationResult::Incomplete,
            Err(_) => ValidationResult::Complete,
        }
    }
}

fn style_node(node: Node<'_>, text: &mut StyledText, source: &str) {
    if node.is_named() {
        if let Some(style) = style_for_kind(node.kind(), node, source) {
            text.style_range(node.start_byte(), node.end_byte(), style);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        style_node(child, text, source);
    }
}

fn style_for_kind(kind: &str, node: Node<'_>, source: &str) -> Option<Style> {
    match kind {
        "command_name" => Some(Style::new().fg(Color::Green)),
        "string" | "raw_string" | "heredoc_body" => Some(Style::new().fg(Color::Yellow)),
        "variable_name" | "simple_expansion" | "expansion" => Some(Style::new().fg(Color::Cyan)),
        "comment" => Some(Style::new().fg(Color::DarkGray)),
        "redirected_statement" | "file_redirect" | "herestring_redirect" => {
            Some(Style::new().fg(Color::Purple))
        }
        "ERROR" => Some(Style::new().fg(Color::Red).bold()),
        "word" if is_known_command(node, source) => Some(Style::new().fg(Color::Green)),
        _ => None,
    }
}

fn is_known_command(node: Node<'_>, source: &str) -> bool {
    let Ok(text) = node.utf8_text(source.as_bytes()) else {
        return false;
    };
    matches!(
        text,
        "cd" | "export" | "unset" | "alias" | "source" | "jobs" | "fg" | "bg" | "disown"
    )
}

fn paste_style() -> Style {
    let light = std::env::var("COLORFGBG")
        .ok()
        .and_then(|value| value.rsplit(';').next().and_then(|n| n.parse::<u8>().ok()))
        .is_some_and(|bg| bg >= 7);
    if light {
        Style::new().on(Color::Fixed(250))
    } else {
        Style::new().on(Color::Fixed(238))
    }
}

fn likely_incomplete(line: &str) -> bool {
    let mut single = false;
    let mut double = false;
    let mut escape = false;
    let mut parens = 0i32;
    let mut braces = 0i32;
    for c in line.chars() {
        if escape {
            escape = false;
            continue;
        }
        match c {
            '\\' => escape = true,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '(' if !single && !double => parens += 1,
            ')' if !single && !double => parens -= 1,
            '{' if !single && !double => braces += 1,
            '}' if !single && !double => braces -= 1,
            _ => {}
        }
    }
    single || double || parens > 0 || braces > 0 || line.trim_end().ends_with('|')
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::Highlighter;

    #[test]
    fn highlights_large_paste_without_tree_sitter_work() {
        let highlighter = BashHighlighter::new(16);
        let styled = highlighter.highlight(&"x".repeat(1024 * 1024), 0);
        assert_eq!(styled.raw_string().len(), 1024 * 1024);
        assert_eq!(styled.buffer.len(), 1);
    }

    #[test]
    fn validator_marks_obvious_incomplete_input() {
        let validator = BashValidator::new(1024);
        assert!(matches!(
            validator.validate("echo 'unterminated"),
            ValidationResult::Incomplete
        ));
    }
}
