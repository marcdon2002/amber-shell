use amber_shell::projects::ProjectIndex;
use std::{fs, path::Path, process::Command};
fn put(root: &Path, name: &str, content: &str) {
    let file = root.join(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, content).unwrap();
}
fn init(root: &Path) {
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}
fn text(index: &ProjectIndex, q: &str) -> String {
    index
        .select(q)
        .iter()
        .map(|f| format!("{}\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn english_documentation_questions_include_readme_context() {
    let temp = tempfile::tempdir().unwrap();
    put(
        temp.path(),
        "README.md",
        "Generate documentation with npm run docs.",
    );
    put(
        temp.path(),
        "package.json",
        r#"{"scripts":{"docs":"typedoc"}}"#,
    );
    let index = ProjectIndex::discover(temp.path());
    for question in [
        "How do I generate documentation?",
        "How do I generate the npm documentation?",
        "How do I build the docs?",
    ] {
        assert!(amber_shell::projects::relevant(question), "{question}");
        assert!(text(&index, question).contains("README.md"), "{question}");
    }
    assert!(!amber_shell::projects::relevant("How do I list folders?"));
    assert!(!text(&index, "npm dev").contains("README.md"));
}
#[test]
fn ignores_tracked_files_ignored_subtrees_and_generated_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    init(root);
    put(root, "hidden/package.json", r#"{"name":"IGNORED_PROJECT"}"#);
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "hidden/package.json"])
            .status()
            .unwrap()
            .success()
    );
    put(root, ".gitignore", "hidden/\nprivate.json\n");
    put(
        root,
        "apps/site/package.json",
        r#"{"name":"VISIBLE_PROJECT","scripts":{"dev":"vite"}}"#,
    );
    put(
        root,
        "node_modules/dependency/package.json",
        r#"{"name":"DEPENDENCY_PROJECT"}"#,
    );
    let index = ProjectIndex::discover(root);
    let body = text(&index, "npm site dev");
    assert!(body.contains("VISIBLE_PROJECT"), "{body}");
    assert!(!body.contains("IGNORED_PROJECT"));
    assert!(!body.contains("DEPENDENCY_PROJECT"));
}
#[test]
fn skips_secrets_symlinks_nested_repositories_and_non_repo_ignore_files() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "secrets/package.json",
        r#"{"scripts":{"dev":"API_KEY=synthetic-value run"}}"#,
    );
    put(
        root,
        "outside/package.json",
        r#"{"name":"SYMLINK_PRIVATE"}"#,
    );
    fs::create_dir(root.join("project")).unwrap();
    symlink(root.join("outside"), root.join("project/link")).unwrap();
    let index = ProjectIndex::discover(&root.join("project"));
    assert!(index.select("npm").is_empty());
    init(&root.join("outside"));
    let index = ProjectIndex::discover(root);
    assert!(!text(&index, "npm").contains("SYMLINK_PRIVATE"));
    assert!(!text(&index, "npm").contains("synthetic-value"));
    put(root, "project/.gitignore", "package.json\n");
    put(
        root,
        "project/package.json",
        r#"{"name":"PRIVATE_IGNORED"}"#,
    );
    assert!(
        ProjectIndex::discover(&root.join("project"))
            .select("npm")
            .is_empty()
    );
}
#[test]
fn selects_named_child_and_workspace_manager_with_a_small_send_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "package.json",
        r#"{"packageManager":"pnpm@10.0.0","workspaces":["apps/*"]}"#,
    );
    for n in 0..10 {
        put(
            root,
            &format!("apps/service{n}/package.json"),
            &format!(r#"{{"name":"service{n}","scripts":{{"dev":"run-service{n}"}}}}"#),
        );
    }
    put(
        root,
        "apps/customer-web/package.json",
        r#"{"name":"customer-web","scripts":{"dev":"vite"},"description":"DO_NOT_SEND","dependencies":{"vite":"1"}}"#,
    );
    put(root, "pnpm-lock.yaml", "PRIVATE_LOCK_CONTENT");
    let index = ProjectIndex::discover(root);
    let files = index.select("npm customer-web dev");
    let body = text(&index, "npm customer-web dev");
    assert!(
        body.contains("apps/customer-web/package.json"),
        "search limited={}; selected context:\n{body}",
        index.limited
    );
    assert!(body.contains("pnpm@10.0.0"));
    assert!(body.contains("pnpm-lock.yaml"));
    assert!(!body.contains("DO_NOT_SEND"));
    assert!(!body.contains("PRIVATE_LOCK_CONTENT"));
    assert!(files.len() <= 4);
    assert!(serde_json::to_string(&body).unwrap().len() < 8192);
}
#[test]
fn search_stops_at_depth_and_entry_limits_and_signals_partial_results() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "apps/web/package.json",
        r#"{"scripts":{"dev":"vite"}}"#,
    );
    put(root, "a/b/c/d/e/package.json", r#"{"name":"TOO_DEEP"}"#);
    let index = ProjectIndex::discover(root);
    assert!(index.limited);
    assert!(!text(&index, "npm").contains("TOO_DEEP"));
    for n in 0..1100 {
        put(root, &format!("junk-{n}.txt"), "unread");
    }
    assert!(ProjectIndex::discover(root).limited);
}
#[test]
fn nearest_repository_ancestors_are_available_but_outside_files_are_not() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, "package.json", r#"{"name":"OUTSIDE_PROJECT"}"#);
    fs::create_dir(root.join("repo")).unwrap();
    init(&root.join("repo"));
    put(
        root,
        "repo/package.json",
        r#"{"name":"WORKSPACE_PROJECT","packageManager":"yarn@4"}"#,
    );
    put(
        root,
        "repo/apps/web/package.json",
        r#"{"name":"web","scripts":{"dev":"vite"}}"#,
    );
    fs::create_dir(root.join("repo/apps/web/src")).unwrap();
    let index = ProjectIndex::discover(&root.join("repo/apps/web/src"));
    let body = text(&index, "npm dev");
    assert!(body.contains("WORKSPACE_PROJECT"));
    assert!(body.contains("vite"));
    assert!(!body.contains("OUTSIDE_PROJECT"));
}

#[test]
fn python_web_questions_and_readme_named_package_folders_keep_the_actual_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "pyproject.toml",
        "[project]\nname = \"python-web-service\"\n",
    );
    put(
        root,
        "apps/readme-web/package.json",
        r#"{"name":"web","scripts":{"dev":"vite"}}"#,
    );
    let index = ProjectIndex::discover(root);
    assert!(text(&index, "Python Webserver starten").contains("pyproject.toml"));
    assert!(text(&index, "npm vite dev").contains("apps/readme-web/package.json"));
    assert!(!text(&index, "npm readme-web dev").contains("pyproject.toml"));
}

#[test]
fn root_and_child_use_the_same_encoded_project_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(
        root,
        "package.json",
        &serde_json::json!({"scripts":{"dev":"\\".repeat(1100)}}).to_string(),
    );
    put(
        root,
        "web/package.json",
        &serde_json::json!({"scripts":{"dev":"\\".repeat(1000)}}).to_string(),
    );
    let selected = ProjectIndex::discover(root).select("npm dev");
    let body = selected
        .iter()
        .map(|f| format!("\n--- {} ---\n{}\n--- End of file ---\n", f.path, f.content))
        .collect::<String>();
    assert!(serde_json::to_string(&body).unwrap().len() <= 8192);
}

#[test]
fn projects_without_git_still_read_visible_manifests_and_honor_nested_ignore_rules() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, ".gitignore", "node_modules/\ndist/\nprivate/\n");
    put(
        root,
        "package.json",
        r#"{"name":"VISIBLE_ROOT","scripts":{"dev":"vite"}}"#,
    );
    put(root, "apps/.gitignore", "hidden/\n");
    put(
        root,
        "apps/hidden/package.json",
        r#"{"name":"HIDDEN_CHILD"}"#,
    );
    put(
        root,
        "apps/site/package.json",
        r#"{"name":"VISIBLE_CHILD"}"#,
    );
    let index = ProjectIndex::discover(root);
    let body = text(&index, "npm site dev");
    assert!(body.contains("VISIBLE_ROOT"), "{body}");
    assert!(body.contains("VISIBLE_CHILD"));
    assert!(!body.contains("HIDDEN_CHILD"));
    assert!(!root.join(".git").exists());
}
