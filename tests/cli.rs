use std::process::Command;

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
