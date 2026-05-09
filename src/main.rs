use plush::{Shell, config, interactive, parser, profile::StartupProfile, terminal};

enum Invocation {
    Interactive,
    Command(String),
    Validate(String),
    Complete { line: String, pos: Option<usize> },
    RepairTerminal,
    Version,
    Help,
    Unknown(String),
}

fn main() {
    let mut profile = StartupProfile::from_env();
    profile.mark("start");
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let config = config::load();
    profile.mark("config loaded");
    let mut shell = Shell::new(config);
    profile.mark("shell initialized");

    match parse_invocation(&args) {
        Invocation::Command(command) => match shell.run_line(&command) {
            Ok(outcome) => std::process::exit(outcome.status),
            Err(err) => {
                eprintln!("plush: {err}");
                std::process::exit(2);
            }
        },
        Invocation::Validate(command) => match parser::validate_with_brush(&command) {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                eprintln!("plush: {err}");
                std::process::exit(2);
            }
        },
        Invocation::Complete { line, pos } => {
            let pos = pos.unwrap_or(line.len());
            for suggestion in
                plush::completion::complete_line(shell.aliases.clone(), &line, pos.min(line.len()))
            {
                if let Some(description) = suggestion.description {
                    println!("{}\t{}", suggestion.value, description);
                } else {
                    println!("{}", suggestion.value);
                }
            }
        }
        Invocation::RepairTerminal => {
            terminal::repair_terminal();
        }
        Invocation::Version => {
            println!("plush {}", env!("CARGO_PKG_VERSION"));
        }
        Invocation::Help => {
            println!("plush - Soft comfy bash-compatible shell");
            println!();
            println!("usage:");
            println!("  plush                 start interactive shell");
            println!("  plush -l              start interactive login shell");
            println!("  plush -c 'command'    run a command");
            println!("  plush -lc 'command'   run a command as a login shell");
            println!("  plush --validate 'command'");
            println!("  plush --complete 'line' [cursor-byte-pos]");
            println!("  plush --repair-terminal");
            println!("  plush --version");
        }
        Invocation::Unknown(other) => {
            eprintln!("plush: unknown argument: {other}");
            std::process::exit(2);
        }
        Invocation::Interactive => {
            profile.mark("entering interactive mode");
            match interactive::run_interactive(&mut shell) {
                Ok(code) => std::process::exit(code),
                Err(err) => {
                    eprintln!("plush: {err}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn parse_invocation(args: &[String]) -> Invocation {
    let mut idx = 0;

    while let Some(arg) = args.get(idx) {
        match arg.as_str() {
            "-l" | "--login" => {
                idx += 1;
            }
            "-c" | "--command" => {
                return Invocation::Command(args.get(idx + 1).cloned().unwrap_or_default());
            }
            "--validate" => {
                return Invocation::Validate(args.get(idx + 1).cloned().unwrap_or_default());
            }
            "--complete" => {
                let line = args.get(idx + 1).cloned().unwrap_or_default();
                let pos = args
                    .get(idx + 2)
                    .and_then(|value| value.parse::<usize>().ok());
                return Invocation::Complete { line, pos };
            }
            "--repair-terminal" => return Invocation::RepairTerminal,
            "--version" | "-V" => return Invocation::Version,
            "--help" | "-h" => return Invocation::Help,
            _ if arg.starts_with('-') => {
                if let Some(invocation) = parse_short_options(arg, args.get(idx + 1)) {
                    return invocation;
                }
                return Invocation::Unknown(arg.clone());
            }
            _ => return Invocation::Unknown(arg.clone()),
        }
    }

    Invocation::Interactive
}

fn parse_short_options(arg: &str, next: Option<&String>) -> Option<Invocation> {
    let mut login = false;
    let mut chars = arg.strip_prefix('-')?.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            'l' => login = true,
            'c' if chars.peek().is_none() => {
                return Some(Invocation::Command(next.cloned().unwrap_or_default()));
            }
            _ => return None,
        }
    }

    login.then_some(Invocation::Interactive)
}
