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

    let mut editor = Reedline::create()
        .use_bracketed_paste(true)
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

    let mut prompt = PurePrompt::new();
    prompt.refresh(None);
    let mut last_outcome: Option<RunOutcome> = None;

    loop {
        prompt.refresh(last_outcome.as_ref());
        match editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                if line.trim().is_empty() {
                    last_outcome = None;
                    continue;
                }
                let _ = crossterm::terminal::disable_raw_mode();
                match shell.run_line(&line) {
                    Ok(outcome) => last_outcome = Some(outcome),
                    Err(err) => {
                        eprintln!("plush: {err}");
                        shell.env.set_last_status(2);
                        last_outcome = Some(RunOutcome {
                            status: 2,
                            duration: std::time::Duration::ZERO,
                        });
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                println!("^C");
                shell.env.set_last_status(130);
                last_outcome = Some(RunOutcome {
                    status: 130,
                    duration: std::time::Duration::ZERO,
                });
            }
            Ok(Signal::CtrlD) => {
                println!("exit");
                return Ok(shell.env.last_status());
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("plush: editor error: {err}");
                return Ok(1);
            }
        }
    }
}
