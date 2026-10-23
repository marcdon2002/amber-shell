use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn fake_codex(root: &Path) -> std::path::PathBuf {
    let program = root.join("codex");
    fs::write(&program, r#"#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
root=Path(__file__).parent
home=Path(os.environ['CODEX_HOME'])
args=sys.argv[1:]
mask=os.umask(0o077)
with (root/'calls.jsonl').open('a') as log:
 log.write(json.dumps({'args':args,'home':str(home),'umask':mask,'api_environment':any(os.environ.get(k) for k in ['OPENAI_API_KEY','CODEX_API_KEY','OPENAI_BASE_URL','CODEX_ACCESS_TOKEN'])})+'\n')
if '--version' in args: print('codex-cli synthetic'); sys.exit(0)
auth=home/'auth.json'
if 'logout' in args: auth.unlink(missing_ok=True); print('Logged out'); sys.exit(0)
if args[-2:]==['login','status']:
 print('Logged in using ChatGPT' if auth.exists() else 'Not logged in')
 sys.exit(0 if auth.exists() else 3)
assert 'login' in args
auth.write_text('{"synthetic_login":true}')
print('Synthetic browser login')
"#).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    program
}

fn cli(root: &Path, program: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_amber-shell"))
        .args(args)
        .env("AMBER_CODEX_BIN", program)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("CODEX_HOME", root.join("personal-codex"))
        .env("OPENAI_API_KEY", "synthetic-unused-key")
        .env("CODEX_API_KEY", "synthetic-unused-key")
        .env("CODEX_ACCESS_TOKEN", "synthetic-unused-token")
        .env("OPENAI_BASE_URL", "https://unused.invalid")
        .output()
        .unwrap()
}

#[test]
fn help_is_available_without_a_terminal_or_codex_for_every_command() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["--help"],
        vec!["login", "--help"],
        vec!["logout", "--help"],
        vec!["doctor", "--help"],
        vec!["ask", "--help"],
        vec!["session", "--help"],
    ] {
        let output = cli(temp.path(), Path::new("/nonexistent/codex"), &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    assert!(!temp.path().join("config").exists());
}

#[test]
fn browser_login_status_and_logout_use_only_ambers_private_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let program = fake_codex(root);
    fs::create_dir(root.join("personal-codex")).unwrap();
    let personal = root.join("personal-codex/auth.json");
    fs::write(&personal, "synthetic personal login: preserve me").unwrap();
    assert!(cli(root, &program, &["login"]).status.success());
    let home = root.join("config/amber-shell/codex");
    assert!(home.join("auth.json").exists());
    assert_eq!(
        fs::metadata(&home).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(home.join("auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(cli(root, &program, &["login", "status"]).status.success());
    assert!(cli(root, &program, &["doctor"]).status.success());
    assert!(cli(root, &program, &["logout"]).status.success());
    assert!(!home.join("auth.json").exists());
    assert_eq!(
        cli(root, &program, &["login", "status"]).status.code(),
        Some(3)
    );
    assert_eq!(
        fs::read_to_string(personal).unwrap(),
        "synthetic personal login: preserve me"
    );
    let calls = fs::read_to_string(root.join("calls.jsonl")).unwrap();
    for line in calls.lines() {
        let call: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(call["home"], home.to_str().unwrap());
        assert_eq!(call["umask"], 0o077);
        assert_eq!(call["api_environment"], false);
        let args = call["args"].as_array().unwrap();
        assert!(args.iter().any(|a| a == "forced_login_method=\"chatgpt\""));
        assert!(
            args.iter()
                .any(|a| a == "cli_auth_credentials_store=\"file\"")
        );
    }
}

#[test]
fn device_login_and_invalid_auth_arguments_never_fall_back_to_the_tui() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let program = fake_codex(root);
    assert!(
        cli(root, &program, &["login", "--device-auth"])
            .status
            .success()
    );
    let before = fs::read(root.join("calls.jsonl")).unwrap();
    assert!(String::from_utf8_lossy(&before).contains("--device-auth"));
    for args in [
        vec!["login", "unexpected"],
        vec!["login", "--with-api-key"],
        vec!["logout", "--all"],
        vec!["doctor", "unexpected"],
    ] {
        assert!(!cli(root, &program, &args).status.success(), "{args:?}");
        assert_eq!(fs::read(root.join("calls.jsonl")).unwrap(), before);
    }
}

#[test]
fn unsafe_auth_directory_is_rejected_before_codex_starts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let program = fake_codex(root);
    fs::create_dir_all(root.join("config/amber-shell")).unwrap();
    fs::create_dir(root.join("personal-codex")).unwrap();
    std::os::unix::fs::symlink(
        root.join("personal-codex"),
        root.join("config/amber-shell/codex"),
    )
    .unwrap();
    assert!(!cli(root, &program, &["login"]).status.success());
    assert!(!root.join("calls.jsonl").exists());
}

#[test]
fn linked_auth_files_cannot_reach_another_login() {
    for hard_link in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let program = fake_codex(root);
        let home = root.join("config/amber-shell/codex");
        fs::create_dir_all(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let personal = root.join("personal-auth.json");
        fs::write(&personal, "synthetic personal login").unwrap();
        fs::set_permissions(&personal, fs::Permissions::from_mode(0o600)).unwrap();
        if hard_link {
            fs::hard_link(&personal, home.join("auth.json")).unwrap();
        } else {
            std::os::unix::fs::symlink(&personal, home.join("auth.json")).unwrap();
        }
        assert!(!cli(root, &program, &["logout"]).status.success());
        assert!(!root.join("calls.jsonl").exists());
        assert_eq!(
            fs::read_to_string(&personal).unwrap(),
            "synthetic personal login"
        );
    }
}

#[test]
fn version_does_not_require_a_terminal_or_codex() {
    let output = Command::new(env!("CARGO_BIN_EXE_amber-shell"))
        .arg("--version")
        .env("AMBER_CODEX_BIN", "/nonexistent/codex")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("amber-shell {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}
