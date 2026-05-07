use crate::config::Config;
use crate::error::{PlushError, Result};
use crate::exec::{self, Job};
use crate::expand::Env;
use crate::parser;
use std::collections::BTreeMap;
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
    previous_dir: Option<PathBuf>,
    config: Config,
}

impl Shell {
    pub fn new(config: Config) -> Self {
        let mut shell = Self {
            env: Env::new(),
            aliases: config.aliases.clone(),
            jobs: Vec::new(),
            previous_dir: None,
            config,
        };
        shell.load_env_file();
        shell
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn run_line(&mut self, line: &str) -> Result<RunOutcome> {
        let start = std::time::Instant::now();
        let expanded_alias = self.expand_alias(line)?;
        let script = parser::parse(&expanded_alias)?;
        let status = exec::run_script(self, &script)?;
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
        let old = std::env::current_dir()?;
        std::env::set_current_dir(&dest)?;
        self.previous_dir = Some(old.clone());
        self.env.set("OLDPWD", old.to_string_lossy());
        self.env
            .set("PWD", std::env::current_dir()?.to_string_lossy());
        crate::dirs::record(&std::env::current_dir()?);
        Ok(0)
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

    fn expand_alias(&self, line: &str) -> Result<String> {
        let trimmed = line.trim_start();
        let leading = &line[..line.len() - trimmed.len()];
        let Some((first, rest)) = split_first_word(trimmed) else {
            return Ok(line.to_string());
        };
        let Some(alias) = self.aliases.get(first) else {
            return Ok(line.to_string());
        };
        let mut out = String::new();
        out.push_str(leading);
        out.push_str(alias);
        if !rest.is_empty() {
            out.push(' ');
            out.push_str(rest);
        }
        Ok(out)
    }

    fn load_env_file(&mut self) {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(home.join(".env")) else {
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

fn split_first_word(input: &str) -> Option<(&str, &str)> {
    let end = input
        .find(|c: char| c.is_ascii_whitespace() || matches!(c, ';' | '|' | '&'))
        .unwrap_or(input.len());
    if end == 0 {
        None
    } else {
        Some((&input[..end], input[end..].trim_start()))
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
