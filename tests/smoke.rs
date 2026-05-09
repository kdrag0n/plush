use assert_cmd::prelude::*;
use plush::{Shell, completion::complete_line, config::Config};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
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
fn accepts_login_command_option_cluster() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-lc", "/bin/sh -c 'echo exit-status-ok; exit 7'"])
        .assert()
        .code(7)
        .stdout("exit-status-ok\n")
        .stderr("");
}

#[test]
fn accepts_split_login_command_options() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-l", "-c", "echo login-command-ok"])
        .assert()
        .success()
        .stdout("login-command-ok\n");
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
fn redirects_builtins() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pwd");
    let command = format!("pwd > {} && cat {}", file.display(), file.display());
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout(predicates::str::contains("plush"));
}

#[test]
fn preserves_redirection_order() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out");
    let command = format!(
        "/bin/sh -c 'echo out; echo err >&2' 2>&1 > {}",
        file.display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout("err\n");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "out\n");
}

#[test]
fn redirects_arbitrary_file_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in");
    let output = dir.path().join("out");
    std::fs::write(&input, "meow\n").unwrap();
    let command = format!(
        "/bin/sh -c 'cat <&3 >&4' 3<{} 4>{}",
        input.display(),
        output.display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(output).unwrap(), "meow\n");
}

#[test]
fn closes_redirected_file_descriptors() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "/bin/sh -c 'echo hidden >&2' 2>&-"])
        .assert()
        .failure()
        .stderr("");
}

#[test]
fn supports_combined_stdout_stderr_redirection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("both");
    let command = format!(
        "/bin/sh -c 'echo out; echo err >&2' &> {} && cat {}",
        file.display(),
        file.display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout(predicates::str::contains("out\n"))
        .stdout(predicates::str::contains("err\n"));
}

#[test]
fn supports_combined_append_redirection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("both");
    std::fs::write(&file, "start\n").unwrap();
    let command = format!(
        "/bin/sh -c 'echo out; echo err >&2' &>> {} && cat {}",
        file.display(),
        file.display()
    );
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout("start\nout\nerr\n");
}

#[test]
fn supports_read_write_redirection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("created");
    let command = format!(": <> {}", file.display());
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success();
    assert!(file.exists());
}

#[test]
fn supports_redirection_only_commands() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("created");
    let command = format!("> {}", file.display());
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success();
    assert!(file.exists());
}

#[test]
fn supports_here_strings_natively() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "cat <<< meow"])
        .assert()
        .success()
        .stdout("meow\n");
}

#[test]
fn runs_heredocs_through_bash_compatibility() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "cat <<EOF\nmeow\nEOF"])
        .assert()
        .success()
        .stdout("meow\n");
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
fn default_aliases_include_curated_git_shortcuts() {
    let config = Config::default();
    assert_eq!(config.aliases.get("g").unwrap(), "git");
    assert_eq!(config.aliases.get("gws").unwrap(), "git status --short");
    assert_eq!(
        config.aliases.get("gpf").unwrap(),
        "git push --force-with-lease"
    );
    assert_eq!(
        config.aliases.get("gSI").unwrap(),
        "git submodule update --init --recursive"
    );
    assert_eq!(
        config.aliases.get("gl").unwrap(),
        "git log --pretty=format:'%C(yellow)%h%Creset %Cgreen%cr%Creset %C(auto)%d%Creset %s'"
    );
    assert!(!config.aliases.contains_key("gFf"));
}

#[test]
fn git_log_alias_quotes_pretty_format() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "gl --max-count=0"])
        .assert()
        .success();
}

#[test]
fn expands_chained_aliases_with_loop_guard() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out");
    let mut config = Config::default();
    config
        .aliases
        .insert("a".to_string(), "b --flag".to_string());
    config
        .aliases
        .insert("b".to_string(), "echo ok".to_string());
    let mut shell = Shell::new(config);

    shell
        .run_line(&format!("a > {}", file.display()))
        .expect("alias command should run");

    assert_eq!(std::fs::read_to_string(file).unwrap(), "ok --flag\n");
}

#[test]
fn expands_aliases_after_command_connectors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out");
    let mut config = Config::default();
    config
        .aliases
        .insert("m".to_string(), "echo meow".to_string());
    let mut shell = Shell::new(config);

    shell
        .run_line(&format!("true && m > {}", file.display()))
        .expect("alias command should run after connector");

    assert_eq!(std::fs::read_to_string(file).unwrap(), "meow\n");
}

#[test]
fn hash_r_clears_negative_path_cache() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let exe = dir.path().join("meowcmd");
    let mut config = Config::default();
    config.aliases.clear();
    let mut shell = Shell::new(config);
    shell.env.set("PATH", dir.path().to_string_lossy());

    assert!(shell.run_line("meowcmd").is_err());

    std::fs::write(
        &exe,
        format!("#!/bin/sh\nprintf meow > {}\n", out.display()),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(shell.run_line("meowcmd").is_err());
    shell.run_line("hash -r").unwrap();
    shell.run_line("meowcmd").unwrap();

    assert_eq!(std::fs::read_to_string(out).unwrap(), "meow");
}

#[test]
fn path_assignment_affects_command_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let exe = dir.path().join("meowcmd");
    std::fs::write(
        &exe,
        format!("#!/bin/sh\nprintf meow > {}\n", out.display()),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut config = Config::default();
    config.aliases.clear();
    let mut shell = Shell::new(config);
    shell.env.set("PATH", "");

    shell
        .run_line(&format!("PATH={} meowcmd", dir.path().display()))
        .unwrap();

    assert_eq!(std::fs::read_to_string(out).unwrap(), "meow");
}

#[test]
fn shell_introspection_builtins_report_commands() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "type cd && type exec && command -v cd && which sh"])
        .assert()
        .success()
        .stdout(predicates::str::contains("cd is a shell builtin"))
        .stdout(predicates::str::contains("exec is a shell builtin"))
        .stdout(predicates::str::contains("\ncd\n"))
        .stdout(predicates::str::contains("/sh"));
}

#[test]
fn command_builtin_executes_external_command() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "command echo meow"])
        .assert()
        .success()
        .stdout("meow\n");
}

#[test]
fn exec_builtin_replaces_process() {
    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", "exec sh -c 'printf meow'"])
        .assert()
        .success()
        .stdout("meow");
}

#[test]
fn exec_without_command_keeps_redirections() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out");
    let command = format!("exec > {} ; echo meow", file.display());

    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout("");

    assert_eq!(std::fs::read_to_string(file).unwrap(), "meow\n");
}

#[test]
fn z_without_query_lists_recorded_dirs() {
    let state = tempfile::NamedTempFile::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let command = format!("cd {} && z", dir.path().display());

    Command::cargo_bin("plush")
        .unwrap()
        .env("PLUSH_Z_DATA", state.path())
        .args(["-c", &command])
        .assert()
        .success()
        .stdout(predicates::str::contains(dir.path().display().to_string()));
}

#[test]
fn pushd_popd_and_dirs_track_directory_stack() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let command = format!(
        "cd {} && pushd {} && pwd && popd && pwd",
        first.path().display(),
        second.path().display()
    );

    Command::cargo_bin("plush")
        .unwrap()
        .args(["-c", &command])
        .assert()
        .success()
        .stdout(predicates::str::contains(second.path().to_string_lossy()))
        .stdout(predicates::str::contains(first.path().to_string_lossy()));
}

#[test]
fn startup_profile_reports_checkpoints_when_enabled() {
    Command::cargo_bin("plush")
        .unwrap()
        .env("PLUSH_PROFILE_STARTUP", "1")
        .args(["-c", "true"])
        .assert()
        .success()
        .stderr(predicates::str::contains("plush startup:"))
        .stderr(predicates::str::contains("config loaded"))
        .stderr(predicates::str::contains("shell initialized"));
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
