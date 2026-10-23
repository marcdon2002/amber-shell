use amber_shell::context::collect;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn put(root: &Path, name: &str, content: impl AsRef<[u8]>) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn collects_only_allowed_nearby_project_files_and_preserves_contents() {
    let dir = tempdir().unwrap();
    put(dir.path(), "README.md", "# Demo\nÜbersicht\n");
    put(dir.path(), "Cargo.toml", "[package]\nname = \"demo\"\n");
    put(dir.path(), ".env", "PRIVATE_SENTINEL=secret");
    put(dir.path(), "source.rs", "outside allowlist");
    let ctx = collect(dir.path(), &[]);
    assert_eq!(ctx.files.len(), 2);
    assert!(
        ctx.files
            .iter()
            .any(|f| f.path.ends_with("README.md") && f.content == "# Demo\nÜbersicht\n")
    );
    assert!(!ctx.render(true, true).contains("PRIVATE_SENTINEL"));
    assert!(!ctx.render(true, true).contains("outside allowlist"));
}

#[test]
fn does_not_collect_parent_readme_without_repository_boundary() {
    let dir = tempdir().unwrap();
    put(dir.path(), "README.md", "parent-private-marker");
    put(dir.path(), "nested/README.md", "local-marker");
    let ctx = collect(&dir.path().join("nested"), &[]);
    assert!(ctx.render(true, true).contains("local-marker"));
    assert!(!ctx.render(true, true).contains("parent-private-marker"));
}

#[test]
fn collects_parent_manifest_inside_repo_but_never_outside_repo() {
    let dir = tempdir().unwrap();
    put(dir.path(), "README.md", "outside-repo-marker");
    let root = dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    put(&root, "Cargo.toml", "[package]\nname = \"repo-marker\"");
    fs::create_dir(root.join("src")).unwrap();
    let ctx = collect(&root.join("src"), &[]);
    assert!(ctx.render(true, true).contains("repo-marker"));
    assert!(!ctx.render(true, true).contains("outside-repo-marker"));
}

#[test]
fn respects_gitignore_even_for_tracked_allowed_filenames() {
    let dir = tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    put(dir.path(), "README.md", "ignored-private-marker");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    put(dir.path(), ".gitignore", "README.md\n");
    let ctx = collect(dir.path(), &[]);
    assert!(ctx.files.is_empty());
    assert!(ctx.omitted > 0);
}

#[cfg(unix)]
#[test]
fn skips_symlink_and_special_file_without_following_or_blocking() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    put(dir.path(), "outside/secret", "symlink-private-marker");
    fs::create_dir(dir.path().join("project")).unwrap();
    symlink(
        dir.path().join("outside/secret"),
        dir.path().join("project/README.md"),
    )
    .unwrap();
    let ctx = collect(&dir.path().join("project"), &[]);
    assert!(ctx.files.is_empty());
    assert!(ctx.omitted > 0);
}

#[test]
fn excludes_binary_oversize_and_secret_bearing_allowlisted_files() {
    let dir = tempdir().unwrap();
    put(
        dir.path(),
        "README.md",
        "token = \"synthetic-secret-value\"",
    );
    put(dir.path(), "Cargo.toml", [0, 255, 128, 1]);
    put(dir.path(), "package.json", "ü".repeat(5000));
    let ctx = collect(dir.path(), &[]);
    assert!(ctx.files.is_empty());
    assert!(ctx.omitted >= 3);
}

#[test]
fn keeps_recent_history_order_but_omits_secrets_and_controls() {
    let dir = tempdir().unwrap();
    let history = vec![
        "pwd",
        "export API_KEY=synthetic-secret",
        "curl -H 'Authorization: Bearer synthetic-secret' example.test",
        "git clone https://user:synthetic-secret@example.test/repo",
        "git status",
        "printf '\u{1b}]52;c;evil\u{7}'",
        "echo TOKEN=synthetic-secret",
        "sshpass -p synthetic-secret ssh example.test",
        "curl --user user:synthetic-secret example.test",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let ctx = collect(dir.path(), &history);
    assert_eq!(ctx.history, vec!["pwd", "git status"]);
    assert!(!ctx.render(true, true).contains("synthetic-secret"));
    assert!(ctx.omitted >= 7);
}

#[test]
fn history_uses_last_twenty_input_entries_and_does_not_backfill_secrets() {
    let dir = tempdir().unwrap();
    let mut history = (0..22).map(|n| format!("echo {n}")).collect::<Vec<_>>();
    history.push("TOKEN=synthetic-secret".into());
    let ctx = collect(dir.path(), &history);
    assert_eq!(ctx.history.len(), 19);
    assert_eq!(ctx.history.first().unwrap(), "echo 3");
    assert_eq!(ctx.history.last().unwrap(), "echo 21");
}

#[test]
fn history_budget_omits_whole_entries_and_prioritizes_recent() {
    let dir = tempdir().unwrap();
    let history = vec![
        format!("echo {}", "a".repeat(5000)),
        format!("echo {}", "b".repeat(5000)),
        "pwd".into(),
    ];
    let ctx = collect(dir.path(), &history);
    assert_eq!(ctx.history, vec![history[1].clone(), "pwd".into()]);
    assert!(ctx.history.iter().map(String::len).sum::<usize>() <= 8192);
    assert!(ctx.omitted > 0);
}

#[test]
fn preview_toggles_remove_actual_data_and_state_origin_without_claiming_outputs() {
    let dir = tempdir().unwrap();
    put(dir.path(), "README.md", "project-content-marker");
    let ctx = collect(dir.path(), &["echo history-content-marker".into()]);
    let none = ctx.render(false, false);
    assert!(!none.contains("project-content-marker"));
    assert!(!none.contains("history-content-marker"));
    assert!(ctx.render(true, false).contains("project-content-marker"));
    assert!(!ctx.render(true, false).contains("history-content-marker"));
    assert!(ctx.render(false, true).contains("history-content-marker"));
    assert!(!ctx.render(false, true).contains("project-content-marker"));
}

#[test]
fn file_count_total_budget_and_render_budget_are_bounded() {
    let dir = tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let names = [
        "README.md",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];
    for name in names {
        put(dir.path(), name, "x".repeat(8000));
    }
    fs::create_dir(dir.path().join("child")).unwrap();
    for name in names {
        put(&dir.path().join("child"), name, "y".repeat(8000));
    }
    let ctx = collect(&dir.path().join("child"), &[]);
    assert!(!ctx.files.is_empty());
    assert!(ctx.files.len() <= 8);
    assert!(ctx.files.iter().map(|f| f.content.len()).sum::<usize>() <= 32768);
    assert!(ctx.render(true, true).len() <= 49152);
    assert!(ctx.omitted > 0);
}

#[test]
fn no_raw_terminal_controls_escape_from_files_or_paths() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().join("terminal\u{1b}]0;spoof\u{7}");
    fs::create_dir(&cwd).unwrap();
    put(&cwd, "README.md", "benign\n\u{1b}]52;c;clipboard\u{7}");
    let ctx = collect(&cwd, &[]);
    let rendered = ctx.render(true, true);
    assert!(rendered.contains("Working directory"));
    assert!(
        rendered
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t')
    );
    assert!(!rendered.contains('\u{1b}'));
}

#[test]
fn caps_small_files_at_eight_and_preserves_nearest_first() {
    let dir = tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let child = dir.path().join("child");
    let names = [
        "README.md",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
    ];
    for name in names {
        put(dir.path(), name, "root-marker");
        put(&child, name, "child-marker");
    }
    let ctx = collect(&child, &[]);
    assert_eq!(ctx.files.len(), 8);
    assert!(ctx.files[..5].iter().all(|f| f.content == "child-marker"));
    assert_eq!(ctx.omitted, 2);
}

#[test]
fn sensitive_history_identifier_suffixes_and_credential_paths_are_excluded() {
    let dir = tempdir().unwrap();
    let history = [
        "MY_SECRET_VALUE=synthetic-value",
        "cat ~/.aws/credentials",
        "cat .env.local",
        "curl -uuser:synthetic-value example.test",
        "echo safe",
    ]
    .map(str::to_owned);
    let ctx = collect(dir.path(), &history);
    assert_eq!(ctx.history, vec!["echo safe"]);
}

#[test]
fn ignore_rules_without_repository_fail_closed() {
    let dir = tempdir().unwrap();
    put(dir.path(), ".gitignore", "README.md\n");
    put(dir.path(), "README.md", "non-repo-private-marker");
    assert!(collect(dir.path(), &[]).files.is_empty());
}

#[test]
fn serialized_context_budget_includes_json_escaping() {
    let dir = tempdir().unwrap();
    for name in ["README.md", "Cargo.toml", "package.json", "pyproject.toml"] {
        put(dir.path(), name, "\"\\\t".repeat(2600));
    }
    let ctx = collect(dir.path(), &[]);
    let text = ctx.render(true, true);
    assert!(!ctx.files.is_empty());
    assert!(serde_json::to_string(&text).unwrap().len() <= 49152);
}

#[test]
fn attachment_details_list_only_entries_that_survive_the_send_budget() {
    let context = amber_shell::context::Context {
        outputs: vec![],
        cwd: "/synthetic/project".into(),
        files: (0..8)
            .map(|i| amber_shell::context::ContextFile {
                path: format!("file-{i}.txt"),
                content: "\\".repeat(8000),
            })
            .collect(),
        history: vec!["printf harmless".into()],
        omitted: 0,
    };
    let payload = context.payload(true, true);
    assert!(payload.files.len() < context.files.len());
    for file in &context.files {
        assert_eq!(
            payload.files.contains(&file.path),
            payload.text.contains(&format!("--- {} ---", file.path))
        );
        assert_eq!(
            payload.files.contains(&file.path),
            payload.details.contains(&file.path)
        );
    }
    assert_eq!(
        payload.commands.contains(&"printf harmless".into()),
        payload.text.contains("printf harmless")
    );
    let off = context.payload(false, false);
    assert!(off.files.is_empty() && off.commands.is_empty());
    assert!(!off.text.contains("printf harmless"));
    assert!(off.details.contains("Project files: off"));
}
