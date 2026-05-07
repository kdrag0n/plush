use assert_cmd::prelude::*;
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
