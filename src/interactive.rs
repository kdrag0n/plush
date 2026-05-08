use crate::completion::PlushCompleter;
use crate::config;
use crate::error::{PlushError, Result};
use crate::highlight::{BashHighlighter, BashValidator};
use crate::prompt::PurePrompt;
use crate::{RunOutcome, Shell};
use crossterm::event::{KeyCode, KeyModifiers};
use nu_ansi_term::{Color, Style};
use reedline::{
    ColumnarMenu, DefaultHinter, EditCommand, Emacs, FileBackedHistory, MenuBuilder, Reedline,
    ReedlineEvent, ReedlineMenu, Signal, default_emacs_keybindings,
};
use std::fs;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub fn run_interactive(shell: &mut Shell) -> Result<i32> {
    crate::terminal::setup_interactive_job_control();

    let history_path = config::history_path();
    if let Some(parent) = history_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let history = Box::new(
        FileBackedHistory::with_file(shell.config().history_size, history_path)
            .map_err(|err| PlushError::msg(format!("history: {err}")))?,
    );

    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('f'),
        ReedlineEvent::Edit(vec![EditCommand::Complete]),
    );
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('c'),
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Esc,
            ReedlineEvent::Repaint,
            ReedlineEvent::CtrlC,
        ]),
    );

    let completion_menu = Box::new(
        ColumnarMenu::default()
            .with_name("completion_menu")
            .with_columns(4)
            .with_column_width(Some(24)),
    );

    let git_redraw_signal = Arc::new(AtomicBool::new(false));

    let mut editor = Reedline::create()
        .use_bracketed_paste(true)
        .with_break_signal(Arc::clone(&git_redraw_signal))
        .with_poll_interval(Duration::from_millis(25))
        .with_history(history)
        .with_hinter(Box::new(
            DefaultHinter::default().with_style(Style::new().fg(Color::DarkGray)),
        ))
        .with_highlighter(Box::new(BashHighlighter::new(
            shell.config().max_interactive_parse_bytes,
        )))
        .with_validator(Box::new(BashValidator::new(
            shell.config().max_interactive_parse_bytes,
        )))
        .with_completer(Box::new(PlushCompleter::new(shell.aliases.clone())))
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_edit_mode(Box::new(Emacs::new(keybindings)));

    let mut prompt = PurePrompt::with_git_redraw_signal(Arc::clone(&git_redraw_signal));
    prompt.refresh(None);
    let mut pending_outcome: Option<RunOutcome> = None;
    let mut print_prompt_gap = true;

    loop {
        let outcome = pending_outcome.take();
        prompt.refresh(outcome.as_ref());
        if print_prompt_gap {
            println!();
            let _ = std::io::stdout().flush();
            crate::terminal::set_prompt_cursor();
        }
        match editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                print_prompt_gap = true;
                if line.trim().is_empty() {
                    continue;
                }
                let _ = crossterm::terminal::disable_raw_mode();
                crate::terminal::reset_cursor_shape();
                match shell.run_line(&line) {
                    Ok(outcome) => pending_outcome = Some(outcome),
                    Err(err) => {
                        eprintln!("plush: {err}");
                        shell.env.set_last_status(2);
                        pending_outcome = Some(RunOutcome {
                            status: 2,
                            duration: std::time::Duration::ZERO,
                        });
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                print_prompt_gap = true;
                shell.env.set_last_status(130);
                pending_outcome = Some(RunOutcome {
                    status: 130,
                    duration: std::time::Duration::ZERO,
                });
            }
            Ok(Signal::CtrlD) => {
                crate::terminal::reset_cursor_shape();
                return Ok(shell.env.last_status());
            }
            Ok(Signal::ExternalBreak(_)) => {
                print_prompt_gap = false;
                git_redraw_signal.store(false, Ordering::Relaxed);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("plush: editor error: {err}");
                return Ok(1);
            }
        }
    }
}
