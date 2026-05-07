use assert_cmd::prelude::*;
use plush::{Shell, completion::complete_line, config::Config};
use std::collections::BTreeMap;
use std::process::Command;

#[test]
fn runs_simple_command() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "echo hi"])
        .assert()
        .success()
        .stdout("hi\n");
}

#[test]
fn runs_pipeline_and_redirection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out");
    let command = format!(
        "printf '%s\\n' 'hello world' | wc -c > {} && cat {}",
        file.display(),
        file.display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout(predicates::str::contains("12"));
}

#[test]
fn applies_assignment_to_command_environment() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "A=world /bin/sh -c 'echo $A'"])
        .assert()
        .success()
        .stdout("world\n");
}

#[test]
fn validates_bash_compound_syntax() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["--validate", "if true; then echo ok; fi"])
        .assert()
        .success();
}

#[test]
fn runs_valid_bash_compound_syntax_through_compat_path() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "if true; then echo ok; fi"])
        .assert()
        .success()
        .stdout("ok\n");
}

#[test]
fn loads_local_autoenv_on_cd_without_executing_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "PLUSH_AUTOENV_TEST=meow\n").unwrap();
    let command = format!(
        "cd {} && /bin/sh -c 'echo $PLUSH_AUTOENV_TEST'",
        dir.path().display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout("meow\n");
}

#[test]
fn tracks_background_jobs_in_command_mode() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "sleep 0.2 & jobs"])
        .assert()
        .success()
        .stdout(predicates::str::contains("running sleep 0.2"));
}

#[test]
fn reports_command_not_found_like_a_shell() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "plush-definitely-missing-command"])
        .assert()
        .failure()
        .stderr("plush: command not found: plush-definitely-missing-command\n");
}

#[test]
fn exposes_completion_inspection_cli() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["--complete", "git ch", "6"])
        .assert()
        .success()
        .stdout(predicates::str::contains("checkout"));
}

#[test]
fn survives_accidental_megabyte_line() {
    let mut shell = Shell::new(Config::default());
    let err = shell.run_line(&"x".repeat(1024 * 1024)).unwrap_err();
    assert!(err.to_string().contains("input is too large"));
}

#[test]
fn completion_survives_accidental_megabyte_line() {
    let suggestions = complete_line(BTreeMap::new(), &"x".repeat(1024 * 1024), 1024 * 1024);
    assert!(suggestions.is_empty());
}
