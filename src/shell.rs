use crate::config::Config;
use crate::error::{PlushError, Result};
use crate::exec::{self, Job};
use crate::expand::Env;
use crate::parser;
use crate::path_cache::PathCache;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub status: i32,
    pub duration: Duration,
}

pub struct Shell {
    pub env: Env,
    pub aliases: BTreeMap<String, String>,
    pub jobs: Vec<Job>,
    path_cache: PathCache,
    dir_stack: Vec<PathBuf>,
    previous_dir: Option<PathBuf>,
    config: Config,
}

impl Shell {
    pub fn new(config: Config) -> Self {
        let mut shell = Self {
            env: Env::new(),
            aliases: config.aliases.clone(),
            jobs: Vec::new(),
            path_cache: PathCache::default(),
            dir_stack: Vec::new(),
            previous_dir: None,
            config,
        };
        shell.install_zshrc_defaults();
        shell.load_env_file();
        shell.load_local_autoenv();
        shell
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn run_line(&mut self, line: &str) -> Result<RunOutcome> {
        let start = std::time::Instant::now();
        if line.len() > self.config.max_command_bytes {
            return Err(PlushError::msg(format!(
                "input is too large to execute ({} bytes, limit {} bytes)",
                line.len(),
                self.config.max_command_bytes
            )));
        }
        let expanded_alias = self.expand_alias(line)?;
        let status = match parser::parse(&expanded_alias) {
            Ok(script) => exec::run_script(self, &script)?,
            Err(err) => {
                if parser::validate_with_brush(&expanded_alias).is_ok() {
                    exec::run_bash_compat(self, &expanded_alias)?
                } else {
                    return Err(err);
                }
            }
        };
        self.env.set_last_status(status);
        self.reap_background_jobs();
        Ok(RunOutcome {
            status,
            duration: start.elapsed(),
        })
    }

    pub fn run_source_text(&mut self, text: &str) -> Result<i32> {
        let script = parser::parse(text)?;
        exec::run_script(self, &script)
    }

    pub fn cd(&mut self, target: Option<&str>) -> Result<i32> {
        let dest = match target {
            Some("-") => self
                .previous_dir
                .clone()
                .ok_or_else(|| PlushError::msg("cd: OLDPWD not set"))?,
            Some(path) => expand_cd_target(path, &self.env)?,
            None => dirs::home_dir().ok_or_else(|| PlushError::msg("cd: HOME not set"))?,
        };
        self.change_dir(dest)
    }

    pub fn pushd(&mut self, target: Option<&str>) -> Result<i32> {
        let current = std::env::current_dir()?;
        match target {
            Some(target) => {
                let dest = expand_cd_target(target, &self.env)?;
                self.change_dir(dest)?;
                self.dir_stack.push(current);
            }
            None => {
                let Some(target) = self.dir_stack.last().cloned() else {
                    return Err(PlushError::msg("pushd: directory stack empty"));
                };
                self.change_dir(target)?;
                if let Some(top) = self.dir_stack.last_mut() {
                    *top = current;
                }
            }
        }
        Ok(0)
    }

    pub fn popd(&mut self) -> Result<i32> {
        let Some(target) = self.dir_stack.last().cloned() else {
            return Err(PlushError::msg("popd: directory stack empty"));
        };
        self.change_dir(target)?;
        self.dir_stack.pop();
        Ok(0)
    }

    pub fn dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))];
        dirs.extend(self.dir_stack.iter().rev().cloned());
        dirs
    }

    pub fn reap_background_jobs(&mut self) {
        for job in &mut self.jobs {
            if job.done || job.stopped {
                continue;
            }
            let mut any_running = false;
            for child in &mut job.children {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        job.last_status = status.code().unwrap_or(128);
                    }
                    Ok(None) => any_running = true,
                    Err(_) => {}
                }
            }
            job.done = !any_running;
        }
    }

    pub fn resolve_program(&mut self, name: &str) -> Option<PathBuf> {
        let path = self.env.get("PATH").map(str::to_string);
        self.resolve_program_with_path(name, path.as_deref())
    }

    pub fn resolve_program_with_path(&mut self, name: &str, path: Option<&str>) -> Option<PathBuf> {
        if name.contains('/') {
            return Some(PathBuf::from(name));
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.path_cache.resolve(name, path, &cwd)
    }

    pub fn clear_path_cache(&mut self) {
        self.path_cache.clear();
    }

    pub fn path_cache_entries(&self) -> Vec<(String, PathBuf)> {
        self.path_cache
            .entries()
            .map(|(name, path)| (name.clone(), path.clone()))
            .collect()
    }

    fn expand_alias(&self, line: &str) -> Result<String> {
        let mut out = String::with_capacity(line.len());
        let mut command_position = true;
        let mut chars = line.char_indices().peekable();

        while let Some((idx, ch)) = chars.next() {
            if ch.is_ascii_whitespace() {
                out.push(ch);
                continue;
            }

            if matches!(ch, ';' | '|' | '&') {
                out.push(ch);
                if let Some((_, next)) = chars.peek().copied() {
                    if (ch == '&' && next == '&') || (ch == '|' && next == '|') {
                        out.push(next);
                        chars.next();
                    }
                }
                command_position = true;
                continue;
            }

            if command_position {
                let start = idx;
                let mut end = idx + ch.len_utf8();
                while let Some((next_idx, next)) = chars.peek().copied() {
                    if next.is_ascii_whitespace() || matches!(next, ';' | '|' | '&' | '<' | '>') {
                        break;
                    }
                    end = next_idx + next.len_utf8();
                    chars.next();
                }
                let word = &line[start..end];
                if is_plain_alias_word(word) {
                    out.push_str(&self.expand_alias_word(word));
                } else {
                    out.push_str(word);
                }
                command_position = false;
                continue;
            }

            out.push(ch);
            command_position = false;
        }

        Ok(out)
    }

    fn expand_alias_word(&self, word: &str) -> String {
        let mut current = word.to_string();
        let mut seen = BTreeSet::new();
        while let Some((first, rest)) = split_first_alias_word(&current) {
            if !seen.insert(first.to_string()) {
                break;
            }
            let Some(alias) = self.aliases.get(first) else {
                break;
            };
            current = if rest.is_empty() {
                alias.clone()
            } else {
                format!("{alias} {rest}")
            };
        }
        current
    }

    fn load_env_file(&mut self) {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        self.load_env_vars_from_file(&home.join(".env"));
    }

    fn install_zshrc_defaults(&mut self) {
        self.env.set_default("VEDITOR", "code");
        self.env.set_default("EDITOR", "code");
        self.env.set_default("LESS", "-R ");
        if let Some(home) = dirs::home_dir() {
            let git_fuzzy = home.join(".cache/zsh-plugins/git-fuzzy/bin");
            if git_fuzzy.is_dir() {
                self.env.prepend_path(git_fuzzy);
            }
        }
    }

    fn load_local_autoenv(&mut self) {
        if !self.config.autoenv {
            return;
        }
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        self.load_env_vars_from_file(&cwd.join(".env"));
        self.load_env_vars_from_file(&cwd.join(".plushenv"));
    }

    fn change_dir(&mut self, dest: PathBuf) -> Result<i32> {
        let old = std::env::current_dir()?;
        std::env::set_current_dir(&dest)?;
        self.previous_dir = Some(old.clone());
        self.env.set("OLDPWD", old.to_string_lossy());
        self.env
            .set("PWD", std::env::current_dir()?.to_string_lossy());
        crate::dirs::record(&std::env::current_dir()?);
        self.load_local_autoenv();
        Ok(0)
    }

    fn load_env_vars_from_file(&mut self, path: &std::path::Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim_matches('"').trim_matches('\'');
                let expanded = crate::expand::expand_word(value, &self.env)
                    .unwrap_or_else(|_| value.to_string());
                self.env.set(key.trim(), expanded);
            }
        }
    }
}

fn is_plain_alias_word(word: &str) -> bool {
    !word.contains(['\'', '"', '\\', '$', '`'])
}

fn split_first_alias_word(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    let end = trimmed
        .find(|c: char| c.is_ascii_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>'))
        .unwrap_or(trimmed.len());
    if end == 0 {
        None
    } else {
        Some((&trimmed[..end], trimmed[end..].trim_start()))
    }
}

fn expand_cd_target(path: &str, env: &Env) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") {
        let home = dirs::home_dir().ok_or_else(|| PlushError::msg("cd: HOME not set"))?;
        if path == "~" {
            return Ok(home);
        }
        return Ok(home.join(&path[2..]));
    }
    if let Some(var) = path.strip_prefix('$') {
        if let Some(value) = env.get(var) {
            return Ok(PathBuf::from(value));
        }
    }
    Ok(PathBuf::from(path))
}
