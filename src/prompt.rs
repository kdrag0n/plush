use crate::RunOutcome;
use crossterm::style::Color as CrosstermColor;
use nu_ansi_term::{Color, Style};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const PURE_GIT_BRANCH: Color = Color::Fixed(242);
const PURE_GIT_DIRTY: Color = Color::Fixed(218);
const PURE_MUTED: Color = Color::Fixed(242);

#[derive(Debug, Clone)]
pub struct PromptState {
    cwd: PathBuf,
    status: i32,
    duration: Option<Duration>,
    git: Option<GitInfo>,
    git_cache_cwd: Option<PathBuf>,
    git_last_refresh: Option<Instant>,
    git_slow: bool,
    venv: Option<String>,
    ssh: bool,
    user_host: Option<String>,
}

#[derive(Debug, Clone)]
struct GitInfo {
    branch: String,
    dirty: bool,
    ahead: bool,
    behind: bool,
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            status: 0,
            duration: None,
            git: None,
            git_cache_cwd: None,
            git_last_refresh: None,
            git_slow: false,
            venv: std::env::var("VIRTUAL_ENV")
                .ok()
                .or_else(|| std::env::var("CONDA_DEFAULT_ENV").ok())
                .map(short_env_name),
            ssh: is_ssh(),
            user_host: None,
        }
    }
}

impl PromptState {
    pub fn refresh(&mut self, outcome: Option<&RunOutcome>) {
        self.cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Some(outcome) = outcome {
            self.status = outcome.status;
            self.duration = if outcome.duration >= Duration::from_secs(5) {
                Some(outcome.duration)
            } else {
                None
            };
        }
        self.venv = std::env::var("VIRTUAL_ENV")
            .ok()
            .or_else(|| std::env::var("CONDA_DEFAULT_ENV").ok())
            .map(short_env_name);
        self.ssh = is_ssh();
        self.user_host = self.ssh.then(user_host).flatten();
        self.refresh_git(outcome.is_some());
    }

    fn refresh_git(&mut self, command_finished: bool) {
        let cwd_changed = self.git_cache_cwd.as_ref() != Some(&self.cwd);
        let stale = self
            .git_last_refresh
            .is_none_or(|last| last.elapsed() > Duration::from_secs(10));

        if !cwd_changed && !command_finished && !stale {
            return;
        }
        if self.git_slow && !cwd_changed && !command_finished && !stale {
            return;
        }

        let start = Instant::now();
        self.git = git_info(&self.cwd);
        self.git_cache_cwd = Some(self.cwd.clone());
        self.git_last_refresh = Some(Instant::now());
        self.git_slow = start.elapsed() > Duration::from_millis(75);
    }
}

pub struct PurePrompt {
    state: PromptState,
}

impl PurePrompt {
    pub fn new() -> Self {
        Self {
            state: PromptState::default(),
        }
    }

    pub fn refresh(&mut self, outcome: Option<&RunOutcome>) {
        self.state.refresh(outcome);
    }
}

impl Default for PurePrompt {
    fn default() -> Self {
        Self::new()
    }
}

impl Prompt for PurePrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let mut left = String::new();
        left.push('\n');
        if self.state.ssh {
            if let Some(user_host) = &self.state.user_host {
                left.push_str(&Style::new().fg(PURE_MUTED).paint(user_host).to_string());
                left.push(' ');
            }
        }
        left.push_str(
            &Style::new()
                .fg(Color::Blue)
                .paint(display_cwd(&self.state.cwd))
                .to_string(),
        );
        if let Some(git) = &self.state.git {
            left.push(' ');
            left.push_str(
                &Style::new()
                    .fg(PURE_GIT_BRANCH)
                    .paint(&git.branch)
                    .to_string(),
            );
            if git.dirty {
                left.push_str(&Style::new().fg(PURE_GIT_DIRTY).paint("*").to_string());
            }
            if git.behind {
                left.push(' ');
                left.push_str(&Style::new().fg(Color::Cyan).paint("⇣").to_string());
            }
            if git.ahead {
                if !git.behind {
                    left.push(' ');
                }
                left.push_str(&Style::new().fg(Color::Cyan).paint("⇡").to_string());
            }
        }
        if let Some(duration) = self.state.duration {
            left.push(' ');
            left.push_str(
                &Style::new()
                    .fg(Color::Yellow)
                    .paint(format_duration(duration))
                    .to_string(),
            );
        }
        left.push('\n');
        Cow::Owned(left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        let mut prompt = String::new();
        if let Some(venv) = &self.state.venv {
            prompt.push_str(&Style::new().fg(PURE_MUTED).paint(venv).to_string());
            prompt.push(' ');
        }
        let style = if self.state.status == 0 {
            Style::new().fg(Color::Magenta)
        } else {
            Style::new().fg(Color::Red)
        };
        prompt.push_str(&format!("{} ", style.paint("❯")));
        Cow::Owned(prompt)
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Owned(Style::new().fg(PURE_MUTED).paint("… ").to_string())
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let marker = match history_search.status {
            PromptHistorySearchStatus::Passing => "?",
            PromptHistorySearchStatus::Failing => "!",
        };
        Cow::Owned(format!("{marker}{} ", history_search.term))
    }

    fn get_prompt_color(&self) -> CrosstermColor {
        CrosstermColor::Blue
    }

    fn get_indicator_color(&self) -> CrosstermColor {
        if self.state.status == 0 {
            CrosstermColor::Magenta
        } else {
            CrosstermColor::Red
        }
    }
}

fn display_cwd(cwd: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = cwd.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    cwd.display().to_string()
}

fn git_info(cwd: &Path) -> Option<GitInfo> {
    let _git_dir = output_trimmed(
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--git-dir"]),
    )?;
    let branch = output_trimmed(
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["branch", "--show-current"]),
    )
    .or_else(|| {
        output_trimmed(Command::new("git").arg("-C").arg(cwd).args([
            "rev-parse",
            "--short",
            "HEAD",
        ]))
    })?;
    let status = output_trimmed(Command::new("git").arg("-C").arg(cwd).args([
        "status",
        "--porcelain=v1",
        "--branch",
        "--untracked-files=no",
    ]))
    .unwrap_or_default();
    let dirty = status.lines().any(|line| !line.starts_with("##"));
    let ahead = status
        .lines()
        .next()
        .is_some_and(|line| line.contains("ahead"));
    let behind = status
        .lines()
        .next()
        .is_some_and(|line| line.contains("behind"));
    Some(GitInfo {
        branch,
        dirty,
        ahead,
        behind,
    })
}

fn output_trimmed(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn user_host() -> Option<String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()?;
    let host = std::env::var("HOST")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| output_trimmed(Command::new("hostname").arg("-s")))?;
    Some(format!("{user}@{host}"))
}

fn is_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some()
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn short_env_name(value: String) -> String {
    Path::new(&value)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_matches_pure_spacing_and_colors() {
        let mut prompt = PurePrompt::new();
        prompt.state.git = Some(GitInfo {
            branch: "main".to_string(),
            dirty: true,
            ahead: true,
            behind: true,
        });
        prompt.state.duration = Some(Duration::from_secs(6));

        let left = prompt.render_prompt_left();
        assert!(left.starts_with('\n'));
        assert!(left.ends_with('\n'));
        assert!(left.contains("\x1b[38;5;242mmain\x1b[0m"));
        assert!(left.contains("\x1b[38;5;218m*\x1b[0m"));
        assert!(left.contains("\x1b[36m⇣\x1b[0m\x1b[36m⇡\x1b[0m"));
        assert!(left.contains("\x1b[33m6s\x1b[0m"));
    }

    #[test]
    fn prompt_indicator_uses_pure_prompt_line() {
        let mut prompt = PurePrompt::new();
        prompt.state.venv = Some("venv".to_string());
        prompt.state.status = 0;
        assert_eq!(
            prompt.render_prompt_indicator(PromptEditMode::Default),
            "\x1b[38;5;242mvenv\x1b[0m \x1b[35m❯\x1b[0m "
        );

        prompt.state.status = 1;
        assert_eq!(
            prompt.render_prompt_indicator(PromptEditMode::Default),
            "\x1b[38;5;242mvenv\x1b[0m \x1b[31m❯\x1b[0m "
        );
    }
}
