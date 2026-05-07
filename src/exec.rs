use crate::error::{PlushError, Result};
use crate::expand::{expand_assignment, expand_word, expand_words};
use crate::parser::{Command, Connector, Pipeline, Redirect, Script};
use crate::shell::Shell;
use nix::sys::signal::{self, SigHandler, Signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command as ProcessCommand, Stdio};

#[derive(Debug)]
pub struct Job {
    pub id: usize,
    pub command: String,
    pub children: Vec<Child>,
    pub done: bool,
    pub stopped: bool,
    pub last_status: i32,
}

pub fn run_script(shell: &mut Shell, script: &Script) -> Result<i32> {
    let mut last = 0;
    for item in &script.items {
        match item.connector {
            Connector::Always => {}
            Connector::And if last != 0 => continue,
            Connector::Or if last == 0 => continue,
            Connector::And | Connector::Or => {}
        }
        last = run_pipeline(shell, &item.pipeline)?;
        shell.env.set_last_status(last);
    }
    Ok(last)
}

pub fn run_bash_compat(shell: &mut Shell, line: &str) -> Result<i32> {
    let mut process = ProcessCommand::new("/bin/bash");
    process.arg("-c").arg(line);
    process.env_clear();
    for (k, v) in shell.env.iter() {
        process.env(k, v);
    }
    configure_child_process(&mut process, None, Vec::new());
    let child = process.spawn()?;
    let pgid = Pid::from_raw(child.id() as i32);
    let _ = nix::unistd::setpgid(pgid, pgid);
    let status = wait_foreground_job(Some(pgid), line, vec![child], shell)?;
    crate::terminal::repair_terminal();
    Ok(status)
}

const BUILTIN_NAMES: &[&str] = &[
    ":", "true", "false", "cd", "z", "pwd", "exit", "export", "unset", "alias", "reload", "source",
    ".", "history", "hash", "jobs", "fg", "bg", "disown", "pushd", "popd", "dirs", "type",
    "command", "which", "mkc", "su-user", "fp", "wttr", "notify", "kp", "skp", "ks", "sks",
];

fn run_pipeline(shell: &mut Shell, pipeline: &Pipeline) -> Result<i32> {
    if pipeline.commands.len() == 1 && !pipeline.background {
        if let Some(status) = try_builtin(shell, &pipeline.commands[0])? {
            return Ok(status);
        }
    }

    let mut previous_stdout: Option<ChildStdout> = None;
    let mut children = Vec::new();
    let mut pgid: Option<Pid> = None;
    let command_text = pipeline
        .commands
        .iter()
        .map(|cmd| cmd.words.join(" "))
        .collect::<Vec<_>>()
        .join(" | ");

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let argv = expand_words(&cmd.words, &shell.env)?;
        if argv.is_empty() {
            continue;
        }
        let assignments = expand_assignments(cmd, shell)?;
        let lookup_path = assignments
            .iter()
            .rev()
            .find_map(|(name, value)| (name == "PATH").then_some(value.clone()))
            .or_else(|| shell.env.get("PATH").map(str::to_string));
        let Some(program) = shell.resolve_program_with_path(&argv[0], lookup_path.as_deref())
        else {
            return Err(PlushError::msg(format!("command not found: {}", argv[0])));
        };
        let mut process = ProcessCommand::new(&program);
        process.args(&argv[1..]);
        process.env_clear();
        for (k, v) in shell.env.iter() {
            process.env(k, v);
        }
        for (name, value) in assignments {
            process.env(name, value);
        }

        if let Some(stdout) = previous_stdout.take() {
            process.stdin(Stdio::from(stdout));
        }
        if idx < pipeline.commands.len() - 1 {
            process.stdout(Stdio::piped());
        }
        let redirections = redirection_actions(cmd, shell)?;
        configure_child_process(&mut process, pgid, redirections);

        let mut child = process.spawn().map_err(|err| spawn_error(&argv[0], err))?;
        let child_pid = Pid::from_raw(child.id() as i32);
        if pgid.is_none() {
            pgid = Some(child_pid);
        }
        let group = pgid.unwrap_or(child_pid);
        let _ = nix::unistd::setpgid(child_pid, group);
        previous_stdout = child.stdout.take();
        children.push(child);
    }

    if pipeline.background {
        let id = shell.jobs.len() + 1;
        println!("[{id}] started {command_text}");
        shell.jobs.push(Job {
            id,
            command: command_text,
            children,
            done: false,
            stopped: false,
            last_status: 0,
        });
        return Ok(0);
    }

    let status = wait_foreground_job(pgid, &command_text, children, shell)?;
    crate::terminal::repair_terminal();
    Ok(status)
}

fn try_builtin(shell: &mut Shell, cmd: &Command) -> Result<Option<i32>> {
    let argv = expand_words(&cmd.words, &shell.env)?;
    let Some(name) = argv.first().map(String::as_str) else {
        let redirections = redirection_actions(cmd, shell)?;
        let _guard = RedirectionGuard::apply(&redirections)?;
        for (name, value) in &cmd.assignments {
            let value = expand_assignment(value, &shell.env)?;
            shell.env.set(name, value);
        }
        return Ok(Some(0));
    };

    let is_builtin_command =
        is_builtin(name) || (argv.len() == 1 && std::path::Path::new(name).is_dir());
    if !is_builtin_command {
        return Ok(None);
    }

    let redirections = redirection_actions(cmd, shell)?;
    let _guard = RedirectionGuard::apply(&redirections)?;

    let status = match name {
        ":" | "true" => 0,
        "false" => 1,
        "cd" => shell.cd(argv.get(1).map(String::as_str))?,
        "z" => {
            let Some(query) = argv.get(1) else {
                return Err(PlushError::msg("z: missing query"));
            };
            let target = crate::dirs::find(query)?;
            shell.cd(Some(&target.to_string_lossy()))?
        }
        "pwd" => {
            println!("{}", std::env::current_dir()?.display());
            0
        }
        "exit" => {
            let code = argv.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        "export" => {
            for arg in &argv[1..] {
                if let Some((key, value)) = arg.split_once('=') {
                    shell.env.set(key, value);
                } else if shell.env.get(arg).is_none() {
                    shell.env.set(arg, "");
                }
            }
            0
        }
        "unset" => {
            for arg in &argv[1..] {
                shell.env.unset(arg);
            }
            0
        }
        "alias" => {
            if argv.len() == 1 {
                for (k, v) in &shell.aliases {
                    println!("alias {k}='{v}'");
                }
            } else {
                for arg in &argv[1..] {
                    if let Some((k, v)) = arg.split_once('=') {
                        shell.aliases.insert(k.to_string(), v.to_string());
                    }
                }
            }
            0
        }
        "reload" => {
            let config = crate::config::load();
            shell.aliases = config.aliases.clone();
            0
        }
        "source" | "." => {
            let Some(path) = argv.get(1) else {
                return Err(PlushError::msg("source: missing file"));
            };
            let text = std::fs::read_to_string(path)?;
            shell.run_source_text(&text)?
        }
        "history" => {
            eprintln!("history: available in interactive mode");
            0
        }
        "hash" => hash_builtin(shell, &argv[1..])?,
        "type" => type_builtin(shell, &argv[1..])?,
        "which" => which_builtin(shell, &argv[1..])?,
        "command" => command_builtin(shell, cmd, &argv[1..])?,
        "jobs" => {
            shell.reap_background_jobs();
            for job in &shell.jobs {
                let state = if job.done {
                    "done"
                } else if job.stopped {
                    "stopped"
                } else {
                    "running"
                };
                println!("[{}] {} {}", job.id, state, job.command);
            }
            0
        }
        "fg" => fg_job(shell, argv.get(1).map(String::as_str))?,
        "bg" => bg_job(shell, argv.get(1).map(String::as_str))?,
        "disown" => disown_job(shell, argv.get(1).map(String::as_str))?,
        "pushd" => {
            shell.pushd(argv.get(1).map(String::as_str))?;
            print_dirs(shell);
            0
        }
        "popd" => {
            shell.popd()?;
            print_dirs(shell);
            0
        }
        "dirs" => {
            print_dirs(shell);
            0
        }
        "mkc" => {
            let Some(path) = argv.get(1) else {
                return Err(PlushError::msg("mkc: missing directory"));
            };
            std::fs::create_dir_all(path)?;
            shell.cd(Some(path))?
        }
        "su-user" => {
            let Some(user) = argv.get(1) else {
                return Err(PlushError::msg("su-user: missing user"));
            };
            let status = ProcessCommand::new("sudo")
                .args(["sh", "-c"])
                .arg(format!("cd /home/{user}; su -s /bin/bash {user}"))
                .status()?;
            exit_code(status)
        }
        "fp" => {
            let cmd = "loc=$(printf '%s' \"$PATH\" | tr ':' '\\n' | fzf --header='[find:path]') && [ -d \"$loc\" ] && rg --files \"$loc\" | awk -F/ '{print $NF}' | fzf --header=\"[find:exe] => $loc\" >/dev/null";
            let status = ProcessCommand::new("/bin/sh").arg("-c").arg(cmd).status()?;
            exit_code(status)
        }
        "wttr" => {
            let loc = argv.get(1).map(String::as_str).unwrap_or("Stanford");
            let output = ProcessCommand::new("curl")
                .args(["-s", "-H"])
                .arg(format!(
                    "Accept-Language: {}",
                    shell
                        .env
                        .get("LANG")
                        .unwrap_or("en_US")
                        .split('_')
                        .next()
                        .unwrap_or("en")
                ))
                .arg(format!("https://wttr.in/{loc}"))
                .output()?;
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(2).take(5) {
                println!("{line}");
            }
            0
        }
        "notify" => {
            print!(
                "\x1b]99;i=1:d=0;{}\x1b\\",
                argv.get(1).map_or("", String::as_str)
            );
            print!(
                "\x1b]99;i=1:d=1:p=body;{}\x1b\\",
                argv.get(2).map_or("", String::as_str)
            );
            0
        }
        "kp" => fzf_kill(false, false, argv.get(1).map(String::as_str))?,
        "skp" => fzf_kill(true, false, argv.get(1).map(String::as_str))?,
        "ks" => fzf_kill(false, true, argv.get(1).map(String::as_str))?,
        "sks" => fzf_kill(true, true, argv.get(1).map(String::as_str))?,
        _ if argv.len() == 1 && std::path::Path::new(name).is_dir() => shell.cd(Some(name))?,
        _ => unreachable!("builtin candidate checked before applying redirections"),
    };
    Ok(Some(status))
}

fn expand_assignments(cmd: &Command, shell: &Shell) -> Result<Vec<(String, String)>> {
    cmd.assignments
        .iter()
        .map(|(name, value)| Ok((name.clone(), expand_assignment(value, &shell.env)?)))
        .collect()
}

#[derive(Debug)]
enum RedirectionAction {
    Open {
        fd: i32,
        path: CString,
        flags: i32,
        mode: libc::mode_t,
    },
    Dup {
        fd: i32,
        target: i32,
    },
    Close {
        fd: i32,
    },
    HereString {
        fd: i32,
        file: File,
    },
}

fn redirection_actions(cmd: &Command, shell: &Shell) -> Result<Vec<RedirectionAction>> {
    let mut actions = Vec::new();
    for redirect in &cmd.redirects {
        match redirect {
            Redirect::Read { fd, target } => actions.push(RedirectionAction::Open {
                fd: *fd,
                path: c_path(expand_word(target, &shell.env)?)?,
                flags: libc::O_RDONLY,
                mode: 0,
            }),
            Redirect::ReadWrite { fd, target } => actions.push(RedirectionAction::Open {
                fd: *fd,
                path: c_path(expand_word(target, &shell.env)?)?,
                flags: libc::O_RDWR | libc::O_CREAT,
                mode: 0o666,
            }),
            Redirect::Write { fd, target, append } => {
                let mut flags = libc::O_WRONLY | libc::O_CREAT;
                flags |= if *append {
                    libc::O_APPEND
                } else {
                    libc::O_TRUNC
                };
                actions.push(RedirectionAction::Open {
                    fd: *fd,
                    path: c_path(expand_word(target, &shell.env)?)?,
                    flags,
                    mode: 0o666,
                });
            }
            Redirect::WriteBoth { target, append } => {
                let mut flags = libc::O_WRONLY | libc::O_CREAT;
                flags |= if *append {
                    libc::O_APPEND
                } else {
                    libc::O_TRUNC
                };
                actions.push(RedirectionAction::Open {
                    fd: 1,
                    path: c_path(expand_word(target, &shell.env)?)?,
                    flags,
                    mode: 0o666,
                });
                actions.push(RedirectionAction::Dup { fd: 2, target: 1 });
            }
            Redirect::HereString { fd, value } => {
                let mut expanded = expand_word(value, &shell.env)?.into_bytes();
                expanded.push(b'\n');
                actions.push(RedirectionAction::HereString {
                    fd: *fd,
                    file: here_string_file(&expanded)?,
                });
            }
            Redirect::Duplicate { fd, target } => actions.push(RedirectionAction::Dup {
                fd: *fd,
                target: *target,
            }),
            Redirect::Close { fd } => actions.push(RedirectionAction::Close { fd: *fd }),
        }
    }
    Ok(actions)
}

fn c_path(path: String) -> Result<CString> {
    CString::new(Path::new(&path).as_os_str().as_bytes())
        .map_err(|_| PlushError::msg(format!("redirection path contains NUL: {path:?}")))
}

fn here_string_file(data: &[u8]) -> Result<File> {
    let mut last_err = None;
    for attempt in 0..100 {
        let path =
            std::env::temp_dir().join(format!("plush-herestr-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = std::fs::remove_file(&path);
                file.write_all(data)?;
                file.seek(SeekFrom::Start(0))?;
                // The temp fd is only a source for dup2(); the installed target fd
                // must remain inheritable, but this source should not leak across exec.
                set_cloexec(file.as_raw_fd())?;
                return Ok(file);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(err);
            }
            Err(err) => return Err(err.into()),
        }
    }
    Err(last_err
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "temporary file exists"))
        .into())
}

fn set_cloexec(fd: i32) -> io::Result<()> {
    set_fd_cloexec(fd, true)
}

fn clear_cloexec(fd: i32) -> io::Result<()> {
    set_fd_cloexec(fd, false)
}

fn set_fd_cloexec(fd: i32, enabled: bool) -> io::Result<()> {
    unsafe {
        let mut flags = libc::fcntl(fd, libc::F_GETFD);
        if flags == -1 {
            return Err(io::Error::last_os_error());
        }
        if enabled {
            flags |= libc::FD_CLOEXEC;
        } else {
            flags &= !libc::FD_CLOEXEC;
        }
        if libc::fcntl(fd, libc::F_SETFD, flags) == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

struct RedirectionGuard {
    saved: Vec<SavedFd>,
}

struct SavedFd {
    fd: i32,
    saved: Option<i32>,
}

impl RedirectionGuard {
    fn apply(actions: &[RedirectionAction]) -> Result<Self> {
        let mut saved = Vec::new();
        for action in actions {
            for fd in action.touched_fds() {
                if saved.iter().any(|saved: &SavedFd| saved.fd == fd) {
                    continue;
                }
                saved.push(SavedFd {
                    fd,
                    saved: dup_cloexec(fd)?,
                });
            }
            apply_redirection_action(action)?;
        }
        Ok(Self { saved })
    }
}

impl Drop for RedirectionGuard {
    fn drop(&mut self) {
        for saved in self.saved.iter().rev() {
            unsafe {
                match saved.saved {
                    Some(saved_fd) => {
                        let _ = libc::dup2(saved_fd, saved.fd);
                        let _ = libc::close(saved_fd);
                    }
                    None => {
                        let _ = libc::close(saved.fd);
                    }
                }
            }
        }
    }
}

impl RedirectionAction {
    fn touched_fds(&self) -> Vec<i32> {
        match self {
            RedirectionAction::Open { fd, .. }
            | RedirectionAction::Dup { fd, .. }
            | RedirectionAction::Close { fd }
            | RedirectionAction::HereString { fd, .. } => vec![*fd],
        }
    }
}

fn dup_cloexec(fd: i32) -> Result<Option<i32>> {
    unsafe {
        if libc::fcntl(fd, libc::F_GETFD) == -1 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EBADF) {
                return Ok(None);
            }
            return Err(err.into());
        }
        // Saved shell fds are restoration handles only. Keep them close-on-exec so
        // builtins that spawn helpers do not leak private copies into children.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let saved = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let saved = {
            let saved = libc::fcntl(fd, libc::F_DUPFD, 10);
            if saved != -1 {
                let _ = libc::fcntl(saved, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            saved
        };
        if saved == -1 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Some(saved))
    }
}

fn apply_redirection_action(action: &RedirectionAction) -> io::Result<()> {
    unsafe {
        match action {
            RedirectionAction::Open {
                fd,
                path,
                flags,
                mode,
            } => {
                let opened = libc::open(path.as_ptr(), *flags, *mode as libc::c_uint);
                if opened == -1 {
                    return Err(io::Error::last_os_error());
                }
                if opened != *fd {
                    if libc::dup2(opened, *fd) == -1 {
                        let err = io::Error::last_os_error();
                        let _ = libc::close(opened);
                        return Err(err);
                    }
                    if libc::close(opened) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                }
            }
            RedirectionAction::Dup { fd, target } => {
                if libc::dup2(*target, *fd) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            RedirectionAction::Close { fd } => {
                if libc::close(*fd) == -1 {
                    let err = io::Error::last_os_error();
                    if err.raw_os_error() != Some(libc::EBADF) {
                        return Err(err);
                    }
                }
            }
            RedirectionAction::HereString { fd, file } => {
                let source = file.as_raw_fd();
                if source != *fd {
                    if libc::dup2(source, *fd) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                } else {
                    clear_cloexec(*fd)?;
                }
            }
        }
    }
    Ok(())
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|signal| 128 + signal).unwrap_or(1))
}

fn spawn_error(command: &str, err: io::Error) -> PlushError {
    if err.kind() == io::ErrorKind::NotFound {
        PlushError::msg(format!("command not found: {command}"))
    } else {
        PlushError::msg(format!("{command}: {err}"))
    }
}

fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

fn print_dirs(shell: &Shell) {
    println!(
        "{}",
        shell
            .dirs()
            .into_iter()
            .map(|dir| display_path(&dir))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

fn type_builtin(shell: &mut Shell, args: &[String]) -> Result<i32> {
    let mut all = false;
    let mut names = Vec::new();
    for arg in args {
        if arg == "-a" {
            all = true;
        } else {
            names.push(arg.as_str());
        }
    }
    if names.is_empty() {
        return Err(PlushError::msg("type: missing name"));
    }

    let mut status = 0;
    for name in names {
        if !describe_command(shell, name, all, true) {
            eprintln!("type: {name}: not found");
            status = 1;
        }
    }
    Ok(status)
}

fn which_builtin(shell: &Shell, args: &[String]) -> Result<i32> {
    let mut all = false;
    let mut names = Vec::new();
    for arg in args {
        if arg == "-a" {
            all = true;
        } else if arg.starts_with('-') {
            return Err(PlushError::msg(format!("which: bad option: {arg}")));
        } else {
            names.push(arg.as_str());
        }
    }
    if names.is_empty() {
        return Err(PlushError::msg("which: missing name"));
    }

    let mut status = 0;
    for name in names {
        let paths = path_candidates(name, shell);
        if paths.is_empty() {
            status = 1;
            continue;
        }
        for path in paths.into_iter().take(if all { usize::MAX } else { 1 }) {
            println!("{}", path.display());
        }
    }
    Ok(status)
}

fn command_builtin(shell: &mut Shell, cmd: &Command, args: &[String]) -> Result<i32> {
    if args.is_empty() {
        return Ok(0);
    }

    let mut verbose = false;
    let mut concise = false;
    let mut idx = 0;
    while let Some(arg) = args.get(idx) {
        match arg.as_str() {
            "--" => {
                idx += 1;
                break;
            }
            "-v" => {
                concise = true;
                idx += 1;
            }
            "-V" => {
                verbose = true;
                idx += 1;
            }
            _ if arg.starts_with('-') => {
                return Err(PlushError::msg(format!("command: bad option: {arg}")));
            }
            _ => break,
        }
    }

    let rest = &args[idx..];
    if concise || verbose {
        if rest.is_empty() {
            return Err(PlushError::msg("command: missing name"));
        }
        let mut status = 0;
        for name in rest {
            let found = if verbose {
                describe_command(shell, name, false, true)
            } else {
                describe_command(shell, name, false, false)
            };
            if !found {
                status = 1;
            }
        }
        return Ok(status);
    }

    run_external_single(shell, cmd, rest)
}

fn describe_command(shell: &mut Shell, name: &str, all: bool, verbose: bool) -> bool {
    let mut found = false;
    if let Some(alias) = shell.aliases.get(name) {
        found = true;
        if verbose {
            println!("{name} is an alias for {alias}");
        } else {
            println!("{alias}");
        }
        if !all {
            return true;
        }
    }
    if is_builtin(name) {
        found = true;
        if verbose {
            println!("{name} is a shell builtin");
        } else {
            println!("{name}");
        }
        if !all {
            return true;
        }
    }

    let paths = if all {
        path_candidates(name, shell)
    } else {
        shell.resolve_program(name).into_iter().collect()
    };
    for path in paths {
        found = true;
        if verbose {
            println!("{name} is {}", path.display());
        } else {
            println!("{}", path.display());
        }
        if !all {
            break;
        }
    }
    found
}

fn path_candidates(name: &str, shell: &Shell) -> Vec<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return is_executable_file(&path)
            .then_some(path)
            .into_iter()
            .collect();
    }
    let Some(path) = shell.env.get("PATH") else {
        return Vec::new();
    };
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .filter(|path| is_executable_file(path))
        .collect()
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn run_external_single(shell: &mut Shell, cmd: &Command, argv: &[String]) -> Result<i32> {
    let Some(name) = argv.first() else {
        return Ok(0);
    };
    let assignments = expand_assignments(cmd, shell)?;
    let lookup_path = assignments
        .iter()
        .rev()
        .find_map(|(name, value)| (name == "PATH").then_some(value.clone()))
        .or_else(|| shell.env.get("PATH").map(str::to_string));
    let Some(program) = shell.resolve_program_with_path(name, lookup_path.as_deref()) else {
        return Err(PlushError::msg(format!("command not found: {name}")));
    };

    let mut process = ProcessCommand::new(program);
    process.args(&argv[1..]);
    process.env_clear();
    for (key, value) in shell.env.iter() {
        process.env(key, value);
    }
    for (key, value) in assignments {
        process.env(key, value);
    }
    configure_child_process(&mut process, None, Vec::new());
    let child = process.spawn().map_err(|err| spawn_error(name, err))?;
    let pgid = Pid::from_raw(child.id() as i32);
    let _ = nix::unistd::setpgid(pgid, pgid);
    let status = wait_foreground_job(Some(pgid), &argv.join(" "), vec![child], shell)?;
    crate::terminal::repair_terminal();
    Ok(status)
}

fn hash_builtin(shell: &mut Shell, args: &[String]) -> Result<i32> {
    if args.is_empty() {
        for (name, path) in shell.path_cache_entries() {
            println!("{name}={}", path.display());
        }
        return Ok(0);
    }

    let mut status = 0;
    for arg in args {
        if arg == "-r" {
            shell.clear_path_cache();
            crate::completion::clear_command_cache();
            continue;
        }
        if arg.starts_with('-') {
            return Err(PlushError::msg(format!("hash: bad option: {arg}")));
        }
        if shell.resolve_program(arg).is_none() {
            eprintln!("hash: no such command: {arg}");
            status = 1;
        }
    }
    Ok(status)
}

fn fg_job(shell: &mut Shell, spec: Option<&str>) -> Result<i32> {
    let idx = find_job(shell, spec).ok_or_else(|| PlushError::msg("fg: no such job"))?;
    let mut job = shell.jobs.remove(idx);
    let Some(pgid) = job_group(&job) else {
        return Ok(job.last_status);
    };
    let _ = signal::kill(Pid::from_raw(-pgid.as_raw()), Signal::SIGCONT);
    job.stopped = false;
    println!("{}", job.command);
    wait_foreground_job(Some(pgid), &job.command.clone(), job.children, shell)
}

fn bg_job(shell: &mut Shell, spec: Option<&str>) -> Result<i32> {
    let idx = find_job(shell, spec).ok_or_else(|| PlushError::msg("bg: no such job"))?;
    let job = &mut shell.jobs[idx];
    let Some(pgid) = job_group(job) else {
        return Ok(job.last_status);
    };
    signal::kill(Pid::from_raw(-pgid.as_raw()), Signal::SIGCONT)?;
    job.stopped = false;
    println!("[{}] running {}", job.id, job.command);
    Ok(0)
}

fn disown_job(shell: &mut Shell, spec: Option<&str>) -> Result<i32> {
    let idx = find_job(shell, spec).ok_or_else(|| PlushError::msg("disown: no such job"))?;
    shell.jobs.remove(idx);
    Ok(0)
}

fn find_job(shell: &Shell, spec: Option<&str>) -> Option<usize> {
    let id = spec
        .map(|s| s.trim_start_matches('%').parse::<usize>().ok())
        .unwrap_or(None);
    if let Some(id) = id {
        shell.jobs.iter().position(|job| job.id == id)
    } else {
        shell.jobs.iter().rposition(|job| !job.done)
    }
}

fn job_group(job: &Job) -> Option<Pid> {
    job.children
        .first()
        .map(|child| Pid::from_raw(child.id() as i32))
}

fn configure_child_process(
    process: &mut ProcessCommand,
    pgid: Option<Pid>,
    redirections: Vec<RedirectionAction>,
) {
    unsafe {
        process.pre_exec(move || {
            let pid = libc::getpid();
            let group = pgid.map_or(pid, |pgid| pgid.as_raw());
            if libc::setpgid(0, group) == -1 {
                return Err(io::Error::last_os_error());
            }
            for action in &redirections {
                apply_redirection_action(action)?;
            }
            signal::signal(Signal::SIGINT, SigHandler::SigDfl).map_err(io::Error::other)?;
            signal::signal(Signal::SIGQUIT, SigHandler::SigDfl).map_err(io::Error::other)?;
            signal::signal(Signal::SIGTSTP, SigHandler::SigDfl).map_err(io::Error::other)?;
            signal::signal(Signal::SIGTTIN, SigHandler::SigDfl).map_err(io::Error::other)?;
            signal::signal(Signal::SIGTTOU, SigHandler::SigDfl).map_err(io::Error::other)?;
            Ok(())
        });
    }
}

fn wait_foreground_job(
    pgid: Option<Pid>,
    command_text: &str,
    children: Vec<Child>,
    shell: &mut Shell,
) -> Result<i32> {
    let terminal = io::stdin();
    let terminal_fd = terminal.as_raw_fd();
    let terminal_job_control = terminal.is_terminal() && pgid.is_some();
    let shell_pgid = Pid::from_raw(unsafe { libc::getpgrp() });

    if terminal_job_control {
        ignore_tty_stop_signals()?;
        let _ = nix::unistd::tcsetpgrp(&terminal, pgid.unwrap());
        restore_tty_stop_signals()?;
    }

    let mut status = 0;
    let mut stopped = false;
    let mut live_children = children.len();
    let child_pids = children
        .iter()
        .map(|child| Pid::from_raw(child.id() as i32))
        .collect::<Vec<_>>();

    while live_children > 0 {
        let wait_target = pgid
            .map(|pgid| Pid::from_raw(-pgid.as_raw()))
            .unwrap_or_else(|| Pid::from_raw(-1));
        match waitpid(wait_target, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => {
                status = code;
                live_children -= 1;
            }
            Ok(WaitStatus::Signaled(_, signal, _)) => {
                status = 128 + signal as i32;
                live_children -= 1;
            }
            Ok(WaitStatus::Stopped(_, _)) => {
                stopped = true;
                status = 148;
                break;
            }
            Ok(WaitStatus::StillAlive) => {}
            Ok(_) => {}
            Err(nix::errno::Errno::ECHILD) => break,
            Err(err) => return Err(err.into()),
        }
    }

    if terminal_job_control {
        ignore_tty_stop_signals()?;
        let _ = unsafe { libc::tcsetpgrp(terminal_fd, shell_pgid.as_raw()) };
        restore_tty_stop_signals()?;
    }

    if stopped {
        let id = shell.jobs.len() + 1;
        println!("[{id}] stopped {command_text}");
        shell.jobs.push(Job {
            id,
            command: command_text.to_string(),
            children,
            done: false,
            stopped: true,
            last_status: status,
        });
    } else {
        for pid in child_pids {
            let _ = waitpid(pid, Some(WaitPidFlag::WNOHANG));
        }
    }

    Ok(status)
}

fn ignore_tty_stop_signals() -> Result<()> {
    unsafe {
        signal::signal(Signal::SIGTTOU, SigHandler::SigIgn)?;
        signal::signal(Signal::SIGTTIN, SigHandler::SigIgn)?;
        signal::signal(Signal::SIGTSTP, SigHandler::SigIgn)?;
    }
    Ok(())
}

fn restore_tty_stop_signals() -> Result<()> {
    unsafe {
        signal::signal(Signal::SIGTTOU, SigHandler::SigDfl)?;
        signal::signal(Signal::SIGTTIN, SigHandler::SigDfl)?;
        signal::signal(Signal::SIGTSTP, SigHandler::SigDfl)?;
    }
    Ok(())
}

fn fzf_kill(sudo: bool, sockets: bool, signal: Option<&str>) -> Result<i32> {
    let sig = signal.unwrap_or("9");
    let source = if sockets {
        if sudo {
            "sudo lsof -Pwni | sed 1d | grep -e LISTEN -e '\\*:'"
        } else {
            "lsof -Pwni | sed 1d | grep -e LISTEN -e '\\*:'"
        }
    } else if sudo {
        "sudo ps -ef | sed 1d"
    } else {
        "ps -ef | sed 1d"
    };
    let awk = if sockets { "{print $2}" } else { "{print $2}" };
    let cmd = format!(
        "{source} | fzf -m --header='[kill:{}]' | awk '{}' | xargs -r {}kill -{}",
        if sockets { "tcp" } else { "process" },
        awk,
        if sudo { "sudo " } else { "" },
        sig
    );
    let status = ProcessCommand::new("/bin/sh").arg("-c").arg(cmd).status()?;
    Ok(exit_code(status))
}
