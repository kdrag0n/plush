use crate::RunOutcome;
use crossterm::style::Color as CrosstermColor;
use nu_ansi_term::{Color, Style};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;
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
    git_pending_request: Option<u64>,
    git_pending_cwd: Option<PathBuf>,
    git_applied_request: u64,
    git_redraw_signal: Option<Arc<AtomicBool>>,
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

#[derive(Clone)]
struct GitResult {
    id: u64,
    cwd: PathBuf,
    info: Option<GitInfo>,
}

struct GitRequest {
    id: u64,
    cwd: PathBuf,
    redraw_signal: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
struct GitWorkerState {
    latest_requested: u64,
    completed: Option<GitResult>,
}

struct GitStatusWorker {
    tx: mpsc::Sender<GitRequest>,
    state: Arc<Mutex<GitWorkerState>>,
    next_id: AtomicU64,
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
            git_pending_request: None,
            git_pending_cwd: None,
            git_applied_request: 0,
            git_redraw_signal: None,
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
        self.apply_git_result();

        let cwd_changed = self.git_cache_cwd.as_ref() != Some(&self.cwd);
        if cwd_changed {
            self.git = None;
            self.git_cache_cwd = Some(self.cwd.clone());
            self.git_last_refresh = None;
        }

        let stale = self
            .git_last_refresh
            .is_none_or(|last| last.elapsed() > Duration::from_secs(10));
        if !cwd_changed && !command_finished && !stale && self.git_last_refresh.is_some() {
            return;
        }
        if self.git_pending_cwd.as_ref() == Some(&self.cwd) && !command_finished && !stale {
            return;
        }

        let request_id = git_worker().request(self.cwd.clone(), self.git_redraw_signal.clone());
        self.git_pending_request = Some(request_id);
        self.git_pending_cwd = Some(self.cwd.clone());
        self.git_last_refresh = Some(Instant::now());
    }

    fn apply_git_result(&mut self) {
        let Some(result) = git_worker().completed_after(self.git_applied_request) else {
            return;
        };
        self.git_applied_request = result.id;
        if self.git_pending_request == Some(result.id) {
            self.git_pending_request = None;
            self.git_pending_cwd = None;
        }
        if result.cwd != self.cwd {
            return;
        }
        self.git = result.info;
        self.git_cache_cwd = Some(result.cwd);
        self.git_last_refresh = Some(Instant::now());
        if let Some(signal) = &self.git_redraw_signal {
            signal.store(false, Ordering::Relaxed);
        }
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

    pub fn with_git_redraw_signal(redraw_signal: Arc<AtomicBool>) -> Self {
        let mut prompt = Self::new();
        prompt.state.git_redraw_signal = Some(redraw_signal);
        prompt
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
    let repo = gix::discover(cwd).ok()?;
    let branch = repo
        .head_name()
        .ok()
        .flatten()
        .map(|name| name.shorten().to_string())
        .or_else(|| repo.head_id().ok().map(|id| id.shorten_or_id().to_string()))?;
    let dirty = repo.is_dirty().unwrap_or(false);
    let (ahead, behind) = ahead_behind(&repo).unwrap_or((false, false));
    Some(GitInfo {
        branch,
        dirty,
        ahead,
        behind,
    })
}

fn ahead_behind(repo: &gix::Repository) -> Option<(bool, bool)> {
    let head = repo.head_id().ok()?.detach();
    let upstream = repo.rev_parse_single("@{upstream}").ok()?.detach();
    let ahead = repo
        .rev_walk([head])
        .with_hidden([upstream])
        .all()
        .ok()?
        .next()
        .is_some();
    let behind = repo
        .rev_walk([upstream])
        .with_hidden([head])
        .all()
        .ok()?
        .next()
        .is_some();
    Some((ahead, behind))
}

fn git_worker() -> &'static GitStatusWorker {
    static WORKER: OnceLock<GitStatusWorker> = OnceLock::new();
    WORKER.get_or_init(GitStatusWorker::start)
}

impl GitStatusWorker {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<GitRequest>();
        let state = Arc::new(Mutex::new(GitWorkerState::default()));
        let worker_state = Arc::clone(&state);
        thread::Builder::new()
            .name("plush-git-status".to_string())
            .spawn(move || run_git_worker(rx, worker_state))
            .expect("spawn git status worker");
        Self {
            tx,
            state,
            next_id: AtomicU64::new(1),
        }
    }

    fn request(&self, cwd: PathBuf, redraw_signal: Option<Arc<AtomicBool>>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut state) = self.state.lock() {
            state.latest_requested = id;
        }
        let _ = self.tx.send(GitRequest {
            id,
            cwd,
            redraw_signal,
        });
        id
    }

    fn completed_after(&self, applied_request: u64) -> Option<GitResult> {
        let state = self.state.lock().ok()?;
        let result = state.completed.as_ref()?;
        (result.id > applied_request).then(|| result.clone())
    }
}

fn run_git_worker(rx: mpsc::Receiver<GitRequest>, state: Arc<Mutex<GitWorkerState>>) {
    while let Ok(mut request) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            request = newer;
        }

        let info = git_info(&request.cwd);
        let current = if let Ok(mut state) = state.lock() {
            if state.latest_requested == request.id {
                state.completed = Some(GitResult {
                    id: request.id,
                    cwd: request.cwd,
                    info,
                });
                true
            } else {
                false
            }
        } else {
            false
        };

        if current {
            if let Some(signal) = request.redraw_signal {
                signal.store(true, Ordering::Relaxed);
            }
        }
    }
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

    #[test]
    fn git_info_reads_current_repo_without_shelling_out() {
        let info = git_info(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(!info.branch.is_empty());
    }
}
