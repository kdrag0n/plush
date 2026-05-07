use crate::RunOutcome;
use crossterm::style::Color as CrosstermColor;
use nu_ansi_term::{Color, Style};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PromptState {
    cwd: PathBuf,
    status: i32,
    duration: Option<Duration>,
    git: Option<GitInfo>,
    venv: Option<String>,
    ssh: bool,
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
            venv: std::env::var("VIRTUAL_ENV")
                .ok()
                .or_else(|| std::env::var("CONDA_DEFAULT_ENV").ok())
                .map(short_env_name),
            ssh: std::env::var_os("SSH_CONNECTION").is_some()
                || std::env::var_os("SSH_CLIENT").is_some(),
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
        self.ssh = std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_CLIENT").is_some();
        self.git = git_info(&self.cwd);
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
        if self.state.ssh {
            left.push_str(&Style::new().fg(Color::Purple).paint("ssh ").to_string());
        }
        if let Some(venv) = &self.state.venv {
            left.push_str(
                &Style::new()
                    .fg(Color::Cyan)
                    .paint(format!("({venv}) "))
                    .to_string(),
            );
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
                    .fg(Color::Purple)
                    .paint(&git.branch)
                    .to_string(),
            );
            if git.dirty {
                left.push_str(&Style::new().fg(Color::Yellow).paint(" *").to_string());
            }
            if git.ahead {
                left.push_str(&Style::new().fg(Color::Green).paint(" up").to_string());
            }
            if git.behind {
                left.push_str(&Style::new().fg(Color::Red).paint(" down").to_string());
            }
        }
        if let Some(duration) = self.state.duration {
            left.push(' ');
            left.push_str(
                &Style::new()
                    .fg(Color::LightGray)
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
        let style = if self.state.status == 0 {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::Red)
        };
        Cow::Owned(format!("{} ", style.paint("❯")))
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Owned(Style::new().fg(Color::LightGray).paint("  ").to_string())
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
            CrosstermColor::Cyan
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
