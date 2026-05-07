use crate::error::{PlushError, Result};
use crate::expand::{expand_assignment, expand_word, expand_words};
use crate::parser::{Command, Connector, Pipeline, Redirect, Script};
use crate::shell::Shell;
use nix::sys::signal::{self, SigHandler, Signal};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
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
    configure_child_process_group(&mut process, None);
    let child = process.spawn()?;
    let pgid = Pid::from_raw(child.id() as i32);
    let _ = nix::unistd::setpgid(pgid, pgid);
    let status = wait_foreground_job(Some(pgid), line, vec![child], shell)?;
    crate::terminal::repair_terminal();
    Ok(status)
}

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
        apply_redirects(&mut process, cmd, shell)?;
        configure_child_process_group(&mut process, pgid);

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
        for (name, value) in &cmd.assignments {
            let value = expand_assignment(value, &shell.env)?;
            shell.env.set(name, value);
        }
        return Ok(Some(0));
    };

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
        _ => return Ok(None),
    };
    Ok(Some(status))
}

fn apply_redirects(process: &mut ProcessCommand, cmd: &Command, shell: &Shell) -> Result<()> {
    for redirect in &cmd.redirects {
        match redirect {
            Redirect::Read { fd: 0, target } => {
                process.stdin(Stdio::from(File::open(expand_word(target, &shell.env)?)?));
            }
            Redirect::Write {
                fd: 1,
                target,
                append,
            } => {
                process.stdout(Stdio::from(open_write(
                    expand_word(target, &shell.env)?,
                    *append,
                )?));
            }
            Redirect::Write {
                fd: 2,
                target,
                append,
            } => {
                process.stderr(Stdio::from(open_write(
                    expand_word(target, &shell.env)?,
                    *append,
                )?));
            }
            Redirect::Duplicate { fd: 2, target: 1 } => {
                process.stderr(Stdio::inherit());
            }
            Redirect::Close { .. } => {}
            other => {
                return Err(PlushError::Unsupported(format!("redirection {other:?}")));
            }
        }
    }
    Ok(())
}

fn expand_assignments(cmd: &Command, shell: &Shell) -> Result<Vec<(String, String)>> {
    cmd.assignments
        .iter()
        .map(|(name, value)| Ok((name.clone(), expand_assignment(value, &shell.env)?)))
        .collect()
}

fn open_write(path: String, append: bool) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(path)
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

fn configure_child_process_group(process: &mut ProcessCommand, pgid: Option<Pid>) {
    unsafe {
        process.pre_exec(move || {
            let pid = libc::getpid();
            let group = pgid.map_or(pid, |pgid| pgid.as_raw());
            if libc::setpgid(0, group) == -1 {
                return Err(io::Error::last_os_error());
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
