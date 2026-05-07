use plush::{Shell, config, interactive, parser, terminal};

fn main() {
    let mut args = std::env::args().skip(1);
    let config = config::load();
    let mut shell = Shell::new(config);

    match args.next().as_deref() {
        Some("-c") | Some("--command") => {
            let command = args.next().unwrap_or_default();
            match shell.run_line(&command) {
                Ok(outcome) => std::process::exit(outcome.status),
                Err(err) => {
                    eprintln!("plush: {err}");
                    std::process::exit(2);
                }
            }
        }
        Some("--validate") => {
            let command = args.next().unwrap_or_default();
            match parser::validate_with_brush(&command) {
                Ok(()) => std::process::exit(0),
                Err(err) => {
                    eprintln!("plush: {err}");
                    std::process::exit(2);
                }
            }
        }
        Some("--complete") => {
            let line = args.next().unwrap_or_default();
            let pos = args
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(line.len());
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
        Some("--repair-terminal") => {
            terminal::repair_terminal();
        }
        Some("--help") | Some("-h") => {
            println!("plush - a fast, bash-ish interactive shell");
            println!();
            println!("usage:");
            println!("  plush                 start interactive shell");
            println!("  plush -c 'command'    run a command");
            println!("  plush --validate 'command'");
            println!("  plush --complete 'line' [cursor-byte-pos]");
            println!("  plush --repair-terminal");
        }
        Some(other) => {
            eprintln!("plush: unknown argument: {other}");
            std::process::exit(2);
        }
        None => match interactive::run_interactive(&mut shell) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                eprintln!("plush: {err}");
                std::process::exit(1);
            }
        },
    }
}
