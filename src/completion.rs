use reedline::{Completer, Span, Suggestion};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct PlushCompleter {
    aliases: BTreeMap<String, String>,
}

impl PlushCompleter {
    pub fn new(aliases: BTreeMap<String, String>) -> Self {
        Self { aliases }
    }
}

impl Completer for PlushCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let pos = pos.min(line.len());
        let (start, prefix) = current_word(line, pos);
        if prefix.starts_with('$') {
            return env_suggestions(start, prefix);
        }
        let command_position = is_command_position(&line[..start]);
        let mut suggestions = if command_position {
            command_suggestions(start, prefix, &self.aliases)
        } else {
            file_suggestions(start, prefix, false)
        };
        suggestions.extend(shell_completion_bridge(start, prefix, command_position));
        dedup(suggestions)
    }
}

fn current_word(line: &str, pos: usize) -> (usize, &str) {
    let mut start = pos;
    for (idx, ch) in line[..pos].char_indices().rev() {
        if ch.is_ascii_whitespace() || matches!(ch, '|' | '&' | ';' | '<' | '>') {
            break;
        }
        start = idx;
    }
    (start, &line[start..pos])
}

fn is_command_position(prefix: &str) -> bool {
    let trimmed = prefix.trim_end();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.ends_with('|')
        || trimmed.ends_with(';')
        || trimmed.ends_with("&&")
        || trimmed.ends_with("||")
}

fn command_suggestions(
    start: usize,
    prefix: &str,
    aliases: &BTreeMap<String, String>,
) -> Vec<Suggestion> {
    let mut values = BTreeSet::new();
    values.extend(
        aliases
            .keys()
            .filter(|name| name.starts_with(prefix))
            .cloned(),
    );
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(prefix) {
                        values.insert(name);
                    }
                }
            }
        }
    }
    values
        .into_iter()
        .take(200)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            description: aliases.get(&value).cloned(),
            value,
            append_whitespace: true,
            ..Suggestion::default()
        })
        .collect()
}

fn file_suggestions(start: usize, prefix: &str, dirs_only: bool) -> Vec<Suggestion> {
    let (dir, stem) = split_path_prefix(prefix);
    let read_dir = if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        expand_tilde_path(&dir)
    };
    let display_dir = dir.to_string_lossy();
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(read_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if dirs_only && !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(stem) {
                continue;
            }
            let suffix = if path.is_dir() { "/" } else { "" };
            let value = format!("{display_dir}{name}{suffix}");
            out.push(Suggestion {
                span: Span::new(start, start + prefix.len()),
                value,
                description: if path.is_dir() {
                    Some("dir".to_string())
                } else {
                    None
                },
                append_whitespace: !path.is_dir(),
                ..Suggestion::default()
            });
            if out.len() >= 200 {
                break;
            }
        }
    }
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out
}

fn env_suggestions(start: usize, prefix: &str) -> Vec<Suggestion> {
    let needle = prefix.trim_start_matches('$');
    std::env::vars()
        .filter_map(|(key, _)| {
            key.starts_with(needle).then(|| Suggestion {
                span: Span::new(start, start + prefix.len()),
                value: format!("${key}"),
                description: Some("env".to_string()),
                append_whitespace: false,
                ..Suggestion::default()
            })
        })
        .take(200)
        .collect()
}

fn shell_completion_bridge(start: usize, prefix: &str, command_position: bool) -> Vec<Suggestion> {
    if prefix.len() > 128 {
        return Vec::new();
    }
    let action = if command_position { "-A command" } else { "-f" };
    let script = format!("compgen {action} -- {}", shell_quote(prefix));
    let Ok(output) = Command::new("bash").arg("-lc").arg(script).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .take(100)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            value: value.to_string(),
            description: Some("bash".to_string()),
            append_whitespace: command_position,
            ..Suggestion::default()
        })
        .collect()
}

fn dedup(suggestions: Vec<Suggestion>) -> Vec<Suggestion> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for suggestion in suggestions {
        if seen.insert(suggestion.value.clone()) {
            out.push(suggestion);
        }
    }
    out
}

fn split_path_prefix(prefix: &str) -> (PathBuf, &str) {
    let path = Path::new(prefix);
    match path.parent() {
        Some(parent) if parent != Path::new("") => {
            let mut dir = parent.to_path_buf();
            dir.push("");
            (dir, path.file_name().and_then(|s| s.to_str()).unwrap_or(""))
        }
        _ => (PathBuf::new(), prefix),
    }
}

fn expand_tilde_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~/" || text.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(text.trim_start_matches("~/"));
        }
    }
    path.to_path_buf()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_command_position() {
        assert!(is_command_position(""));
        assert!(is_command_position("echo hi | "));
        assert!(!is_command_position("echo "));
    }
}
