use std::io::{self, IsTerminal, Write};
use std::os::fd::AsRawFd;

pub fn setup_interactive_job_control() {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return;
    }
    unsafe {
        let pid = libc::getpid();
        let _ = libc::setpgid(pid, pid);
        let _ = libc::tcsetpgrp(stdin.as_raw_fd(), pid);
        let _ = libc::signal(libc::SIGINT, libc::SIG_IGN);
        let _ = libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        let _ = libc::signal(libc::SIGTSTP, libc::SIG_IGN);
        let _ = libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        let _ = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
}

pub fn repair_terminal() {
    if !io::stdout().is_terminal() {
        return;
    }
    let mut out = io::stdout();
    let _ = write!(
        out,
        "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1004l\x1b[?2004l\x1b[?25h\x1b[?1049l\x1b[?47l\x1b[?1047l"
    );
    let _ = out.flush();
}
