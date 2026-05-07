use reedline::{Completer, Span, Suggestion};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

const MAX_COMPLETION_LINE_BYTES: usize = 128 * 1024;
const MAX_COMPLETION_PREFIX_BYTES: usize = 512;

pub struct PlushCompleter {
    aliases: BTreeMap<String, String>,
    bridge_enabled: bool,
}

impl PlushCompleter {
    pub fn new(aliases: BTreeMap<String, String>) -> Self {
        Self {
            aliases,
            bridge_enabled: true,
        }
    }

    #[cfg(test)]
    fn without_bridge(aliases: BTreeMap<String, String>) -> Self {
        Self {
            aliases,
            bridge_enabled: false,
        }
    }
}

impl Completer for PlushCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let pos = pos.min(line.len());
        if line.len() > MAX_COMPLETION_LINE_BYTES {
            return Vec::new();
        }
        let (start, prefix) = current_word(line, pos);
        if prefix.len() > MAX_COMPLETION_PREFIX_BYTES {
            return Vec::new();
        }
        if prefix.starts_with('$') {
            return env_suggestions(start, prefix);
        }
        if let Some(command) = active_command(line, start) {
            if matches!(command, "ssh" | "scp" | "rsync") {
                return ssh_host_suggestions(start, prefix);
            }
            if command == "cd" {
                return file_suggestions(start, prefix, true);
            }
            if command == "git" {
                return git_suggestions(line, start, prefix);
            }
        }
        let command_position = is_command_position(&line[..start]);
        let mut suggestions = if command_position {
            command_suggestions(start, prefix, &self.aliases)
        } else {
            file_suggestions(start, prefix, false)
        };
        if self.bridge_enabled && should_use_shell_bridge(&suggestions, command_position, prefix) {
            if !command_position {
                suggestions.extend(programmable_bash_completion_bridge(
                    line, pos, start, prefix,
                ));
            }
            suggestions.extend(shell_completion_bridge(start, prefix, command_position));
        }
        dedup(suggestions)
    }
}

pub fn complete_line(aliases: BTreeMap<String, String>, line: &str, pos: usize) -> Vec<Suggestion> {
    let mut completer = PlushCompleter::new(aliases);
    completer.complete(line, pos)
}

pub fn clear_command_cache() {
    if let Some(cache) = COMMAND_CACHE.get() {
        if let Ok(mut cache) = cache.lock() {
            cache.clear();
        }
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

fn active_command(line: &str, word_start: usize) -> Option<&str> {
    let prefix = line[..word_start].trim_end();
    let segment_start = prefix
        .rfind(['|', ';', '&'])
        .map(|idx| idx + 1)
        .unwrap_or(0);
    prefix[segment_start..].split_whitespace().next()
}

fn active_words(line: &str, word_start: usize) -> Vec<&str> {
    let prefix = line[..word_start].trim_end();
    let segment_start = prefix
        .rfind(['|', ';', '&'])
        .map(|idx| idx + 1)
        .unwrap_or(0);
    prefix[segment_start..].split_whitespace().collect()
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
    values.extend(
        cached_path_commands()
            .into_iter()
            .filter(|name| name.starts_with(prefix)),
    );
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

#[derive(Default)]
struct CommandCache {
    path: String,
    cwd: PathBuf,
    commands: Vec<String>,
}

impl CommandCache {
    fn clear(&mut self) {
        self.commands.clear();
        self.path.clear();
        self.cwd = PathBuf::new();
    }

    fn commands(&mut self) -> Vec<String> {
        let path = std::env::var("PATH").unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if self.path != path || self.cwd != cwd || self.commands.is_empty() {
            self.path = path;
            self.cwd = cwd;
            self.commands = collect_path_commands(&self.path);
        }
        self.commands.clone()
    }
}

static COMMAND_CACHE: OnceLock<Mutex<CommandCache>> = OnceLock::new();

fn cached_path_commands() -> Vec<String> {
    COMMAND_CACHE
        .get_or_init(|| Mutex::new(CommandCache::default()))
        .lock()
        .map(|mut cache| cache.commands())
        .unwrap_or_default()
}

fn collect_path_commands(path: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    for dir in std::env::split_paths(path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !is_executable_file(&path) {
                    continue;
                }
                values.insert(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    values.into_iter().collect()
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn should_use_shell_bridge(native: &[Suggestion], command_position: bool, prefix: &str) -> bool {
    if prefix.len() > 128 {
        return false;
    }
    command_position || native.is_empty()
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

fn git_suggestions(line: &str, start: usize, prefix: &str) -> Vec<Suggestion> {
    let words = active_words(line, start);
    if words.len() <= 1 {
        return git_subcommand_suggestions(start, prefix);
    }

    let subcommand = words.get(1).copied().unwrap_or("");
    match subcommand {
        "checkout" | "switch" | "merge" | "rebase" | "branch" | "show" | "log" => {
            let mut out = git_refs(start, prefix);
            if matches!(subcommand, "checkout" | "switch" | "show" | "log") {
                out.extend(file_suggestions(start, prefix, false));
            }
            dedup(out)
        }
        "add" | "restore" | "diff" | "status" => file_suggestions(start, prefix, false),
        _ => {
            let mut native = file_suggestions(start, prefix, false);
            if native.is_empty() {
                native.extend(git_subcommand_suggestions(start, prefix));
            }
            native
        }
    }
}

fn git_subcommand_suggestions(start: usize, prefix: &str) -> Vec<Suggestion> {
    [
        "add",
        "bisect",
        "branch",
        "checkout",
        "cherry-pick",
        "clone",
        "commit",
        "diff",
        "fetch",
        "grep",
        "log",
        "merge",
        "pull",
        "push",
        "rebase",
        "remote",
        "reset",
        "restore",
        "revert",
        "show",
        "status",
        "stash",
        "switch",
        "tag",
    ]
    .into_iter()
    .filter(|cmd| cmd.starts_with(prefix))
    .map(|value| Suggestion {
        span: Span::new(start, start + prefix.len()),
        value: value.to_string(),
        description: Some("git".to_string()),
        append_whitespace: true,
        ..Suggestion::default()
    })
    .collect()
}

fn git_refs(start: usize, prefix: &str) -> Vec<Suggestion> {
    let Ok(repo) = gix::discover(".") else {
        return Vec::new();
    };
    let Ok(platform) = repo.references() else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    if let Ok(iter) = platform.local_branches() {
        refs.extend(iter.filter_map(|reference| {
            reference
                .ok()
                .map(|reference| reference.name().shorten().to_string())
        }));
    }
    if let Ok(iter) = platform.tags() {
        refs.extend(iter.filter_map(|reference| {
            reference
                .ok()
                .map(|reference| reference.name().shorten().to_string())
        }));
    }
    refs.into_iter()
        .filter(|name| name.starts_with(prefix))
        .take(200)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            value,
            description: Some("git ref".to_string()),
            append_whitespace: true,
            ..Suggestion::default()
        })
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
    let mut out = String::from_utf8_lossy(&output.stdout)
        .lines()
        .take(100)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            value: value.to_string(),
            description: Some("bash".to_string()),
            append_whitespace: command_position,
            ..Suggestion::default()
        })
        .collect::<Vec<_>>();
    out.extend(zsh_completion_bridge(start, prefix, command_position));
    out
}

fn programmable_bash_completion_bridge(
    line: &str,
    pos: usize,
    start: usize,
    prefix: &str,
) -> Vec<Suggestion> {
    if prefix.len() > 128 || line.len() > 4096 {
        return Vec::new();
    }
    let script = r#"
source /opt/homebrew/etc/profile.d/bash_completion.sh >/dev/null 2>&1 || \
source /usr/local/etc/profile.d/bash_completion.sh >/dev/null 2>&1 || \
source /usr/share/bash-completion/bash_completion >/dev/null 2>&1 || true
COMP_LINE=$PLUSH_COMP_LINE
COMP_POINT=$PLUSH_COMP_POINT
prefix=${COMP_LINE:0:COMP_POINT}
read -r -a COMP_WORDS <<< "$prefix"
COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
if (( COMP_CWORD < 0 )); then exit 0; fi
cmd=${COMP_WORDS[0]}
cur=${COMP_WORDS[$COMP_CWORD]}
if (( COMP_CWORD > 0 )); then prev=${COMP_WORDS[$((COMP_CWORD - 1))]}; else prev=; fi
type _completion_loader >/dev/null 2>&1 && _completion_loader "$cmd" >/dev/null 2>&1 || true
spec=$(complete -p "$cmd" 2>/dev/null) || exit 0
func=
words=($spec)
for ((i=0; i<${#words[@]}; i++)); do
  if [[ ${words[$i]} == -F && $((i + 1)) -lt ${#words[@]} ]]; then
    func=${words[$((i + 1))]}
  fi
done
[[ -n $func ]] || exit 0
COMPREPLY=()
"$func" "$cmd" "$cur" "$prev" >/dev/null 2>&1 || true
printf '%s\n' "${COMPREPLY[@]}"
"#;
    let Ok(output) = Command::new("bash")
        .arg("-lc")
        .arg(script)
        .env("PLUSH_COMP_LINE", line)
        .env("PLUSH_COMP_POINT", pos.to_string())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|value| value.starts_with(prefix))
        .take(100)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            value: value.to_string(),
            description: Some("bash completion".to_string()),
            append_whitespace: true,
            ..Suggestion::default()
        })
        .collect()
}

fn zsh_completion_bridge(start: usize, prefix: &str, command_position: bool) -> Vec<Suggestion> {
    if !command_position || prefix.len() > 128 {
        return Vec::new();
    }
    let script = format!(
        "print -rl -- ${{(k)commands[(I){}*]}}",
        zsh_pattern_quote(prefix)
    );
    let Ok(output) = Command::new("zsh").arg("-fc").arg(script).output() else {
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
            description: Some("zsh".to_string()),
            append_whitespace: true,
            ..Suggestion::default()
        })
        .collect()
}

fn ssh_host_suggestions(start: usize, prefix: &str) -> Vec<Suggestion> {
    let mut hosts = BTreeSet::new();
    collect_known_hosts(&mut hosts);
    collect_ssh_config_hosts(&mut hosts);
    collect_etc_hosts(&mut hosts);
    hosts
        .into_iter()
        .filter(|host| host.starts_with(prefix) && !host.contains('*') && !host.contains('?'))
        .take(200)
        .map(|value| Suggestion {
            span: Span::new(start, start + prefix.len()),
            value,
            description: Some("host".to_string()),
            append_whitespace: true,
            ..Suggestion::default()
        })
        .collect()
}

fn collect_known_hosts(hosts: &mut BTreeSet<String>) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    for file in [
        home.join(".ssh/known_hosts"),
        home.join(".ssh/known_hosts2"),
    ] {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines() {
            if line.starts_with('#') || line.starts_with('|') {
                continue;
            }
            if let Some(names) = line.split_whitespace().next() {
                for host in names.split(',') {
                    let host = host
                        .trim_start_matches('[')
                        .split(']')
                        .next()
                        .unwrap_or(host)
                        .split(':')
                        .next()
                        .unwrap_or(host);
                    if !host.is_empty() {
                        hosts.insert(host.to_string());
                    }
                }
            }
        }
    }
}

fn collect_ssh_config_hosts(hosts: &mut BTreeSet<String>) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let Ok(text) = fs::read_to_string(home.join(".ssh/config")) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Host ") else {
            continue;
        };
        for host in rest.split_whitespace() {
            hosts.insert(host.to_string());
        }
    }
}

fn collect_etc_hosts(hosts: &mut BTreeSet<String>) {
    let Ok(text) = fs::read_to_string("/etc/hosts") else {
        return;
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        let _ip = parts.next();
        for host in parts {
            hosts.insert(host.to_string());
        }
    }
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

fn zsh_pattern_quote(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if matches!(c, '[' | ']' | '*' | '?' | '\\' | '$' | '{' | '}') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
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

    #[test]
    fn identifies_active_command() {
        assert_eq!(active_command("echo hi | ssh ", 14), Some("ssh"));
        assert_eq!(active_command("cd ", 3), Some("cd"));
    }

    #[test]
    fn completes_git_subcommands() {
        let values = git_suggestions("git ch", 6, "ch")
            .into_iter()
            .map(|s| s.value)
            .collect::<Vec<_>>();
        assert!(values.contains(&"checkout".to_string()));
        assert!(values.contains(&"cherry-pick".to_string()));
    }

    #[test]
    fn completes_alias_at_command_position() {
        let mut aliases = BTreeMap::new();
        aliases.insert("gsh".to_string(), "git show".to_string());
        let mut completer = PlushCompleter::without_bridge(aliases);
        let values = completer
            .complete("gs", 2)
            .into_iter()
            .map(|s| s.value)
            .collect::<Vec<_>>();
        assert!(values.contains(&"gsh".to_string()));
    }

    #[test]
    fn avoids_shell_bridge_for_native_file_matches() {
        let native = vec![Suggestion {
            value: "Cargo.toml".to_string(),
            ..Suggestion::default()
        }];
        assert!(!should_use_shell_bridge(&native, false, "Cargo"));
        assert!(should_use_shell_bridge(&native, true, "ca"));
    }

    #[test]
    fn command_cache_collects_only_executable_files() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("plush-exe");
        let data = dir.path().join("plush-data");
        fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&data, "nope").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o644)).unwrap();

        let commands = collect_path_commands(&dir.path().to_string_lossy());

        assert!(commands.contains(&"plush-exe".to_string()));
        assert!(!commands.contains(&"plush-data".to_string()));
    }
}
