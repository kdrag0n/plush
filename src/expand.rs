use crate::error::{PlushError, Result};
use glob::glob;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: BTreeMap<String, String>,
    last_status: i32,
}

impl Env {
    pub fn new() -> Self {
        let vars = std::env::vars().collect();
        Self {
            vars,
            last_status: 0,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }

    pub fn set_default(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.entry(key.into()).or_insert_with(|| value.into());
    }

    pub fn prepend_path(&mut self, path: PathBuf) {
        let mut entries = vec![path];
        if let Some(current) = self.get("PATH") {
            entries.extend(std::env::split_paths(current));
        }
        if let Ok(joined) = std::env::join_paths(entries) {
            self.set_os_string("PATH", joined);
        }
    }

    fn set_os_string(&mut self, key: impl Into<String>, value: OsString) {
        self.vars
            .insert(key.into(), value.to_string_lossy().into_owned());
    }

    pub fn unset(&mut self, key: &str) {
        self.vars.remove(key);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.vars.iter()
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }
}

pub fn expand_words(words: &[String], env: &Env) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for word in words {
        let expanded = expand_word(word, env)?;
        let globbed = expand_glob(&expanded)?;
        if globbed.is_empty() {
            out.push(expanded);
        } else {
            out.extend(globbed);
        }
    }
    Ok(out)
}

pub fn expand_assignment(value: &str, env: &Env) -> Result<String> {
    expand_word(value, env)
}

pub fn expand_word(word: &str, env: &Env) -> Result<String> {
    let mut out = String::new();
    let bytes = word.as_bytes();
    let mut i = 0;
    if word == "~" || word.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            out.push_str(&home.to_string_lossy());
            i = 1;
        }
    }

    while i < bytes.len() {
        match bytes[i] as char {
            '\'' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(PlushError::Syntax("unterminated single quote".to_string()));
                }
                out.push_str(&word[start..i]);
                i += 1;
            }
            '"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        if let Some(next) = bytes.get(i + 1) {
                            out.push(*next as char);
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else if bytes[i] == b'$' {
                        i = expand_dollar(word, i, env, &mut out)?;
                    } else {
                        out.push(bytes[i] as char);
                        i += 1;
                    }
                }
                if i < bytes.len() && bytes[i] == b'"' {
                    i += 1;
                }
            }
            '$' => {
                i = expand_dollar(word, i, env, &mut out)?;
            }
            '\\' => {
                if let Some(next) = bytes.get(i + 1) {
                    out.push(*next as char);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn expand_dollar(word: &str, start: usize, env: &Env, out: &mut String) -> Result<usize> {
    let bytes = word.as_bytes();
    let Some(next) = bytes.get(start + 1).copied() else {
        out.push('$');
        return Ok(start + 1);
    };

    match next as char {
        '?' => {
            out.push_str(&env.last_status().to_string());
            Ok(start + 2)
        }
        '$' => {
            out.push_str(&std::process::id().to_string());
            Ok(start + 2)
        }
        '(' => {
            let (cmd, end) = read_balanced(word, start + 2, b'(', b')')?;
            let value = run_command_substitution(&cmd)?;
            out.push_str(&value);
            Ok(end + 1)
        }
        '{' => {
            let end = word[start + 2..]
                .find('}')
                .map(|off| start + 2 + off)
                .ok_or_else(|| PlushError::Syntax("unterminated ${...}".to_string()))?;
            let key = &word[start + 2..end];
            out.push_str(env.get(key).unwrap_or(""));
            Ok(end + 1)
        }
        c if is_name_start(c) => {
            let mut end = start + 2;
            while end < bytes.len() && is_name_char(bytes[end] as char) {
                end += 1;
            }
            let key = &word[start + 1..end];
            out.push_str(env.get(key).unwrap_or(""));
            Ok(end)
        }
        _ => {
            out.push('$');
            Ok(start + 1)
        }
    }
}

fn read_balanced(word: &str, mut i: usize, open: u8, close: u8) -> Result<(String, usize)> {
    let bytes = word.as_bytes();
    let mut depth = 1;
    let start = i;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return Ok((word[start..i].to_string(), i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(PlushError::Syntax(
        "unterminated command substitution".to_string(),
    ))
}

fn run_command_substitution(cmd: &str) -> Result<String> {
    let output = Command::new("/bin/sh").arg("-c").arg(cmd).output()?;
    let mut value = String::from_utf8_lossy(&output.stdout).to_string();
    while value.ends_with('\n') {
        value.pop();
    }
    Ok(value)
}

fn expand_glob(word: &str) -> Result<Vec<String>> {
    if !word.chars().any(|c| matches!(c, '*' | '?' | '[')) {
        return Ok(Vec::new());
    }
    let mut matches = Vec::new();
    for path in glob(word).map_err(|err| PlushError::Syntax(err.to_string()))? {
        if let Ok(path) = path {
            matches.push(path_to_string(path));
        }
    }
    matches.sort();
    Ok(matches)
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

fn is_name_start(c: char) -> bool {
    matches!(c, '_' | 'a'..='z' | 'A'..='Z')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_vars_and_quotes() {
        let mut env = Env::new();
        env.set("NAME", "plush");
        assert_eq!(expand_word("\"hi $NAME\"", &env).unwrap(), "hi plush");
        assert_eq!(expand_word("'%s\\n'", &env).unwrap(), "%s\\n");
    }

    #[test]
    fn survives_large_input_linearly() {
        let env = Env::new();
        let input = "x".repeat(1024 * 1024);
        assert_eq!(expand_word(&input, &env).unwrap().len(), input.len());
    }
}
