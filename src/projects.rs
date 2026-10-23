//! Lazy, bounded project discovery. No watcher, source index, model call or command execution.
use crate::context::{
    ContextFile, contains_secret, forbidden_directory, has_unsafe_controls, is_home,
    read_small_text, repository_root,
};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const NAMES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "go.mod",
    "Makefile",
    "justfile",
    "README.md",
    "README",
];
const ENTRIES: usize = 1024;
const DIRECTORIES: usize = 96;
const MANIFESTS: usize = 24;
const INDEX_BYTES: usize = 32 * 1024;
const ATTACH_BYTES: usize = 8 * 1024;
const DEPTH: usize = 4;
const SEARCH_TIME: Duration = Duration::from_millis(150);

/// A cheap intent check: ordinary shell questions never trigger a directory scan.
pub fn relevant(question: &str) -> bool {
    let q = question.to_lowercase();
    [
        "npm",
        "pnpm",
        "yarn",
        "bun",
        "node",
        "web",
        "frontend",
        "backend",
        "projekt",
        "project",
        "package",
        "workspace",
        "monorepo",
        "build",
        "compil",
        "kompil",
        "cargo",
        "rust",
        "python",
        "pytest",
        "pip ",
        "uv ",
        "go run",
        "go test",
        "make",
        "just ",
        "readme",
        "documentation",
        "docs",
        "dokumentation",
        "script",
        "skript",
        "test",
        "start",
        "dev-server",
        "devserver",
        "entwicklungsserver",
    ]
    .iter()
    .any(|s| q.contains(s))
}

#[derive(Default)]
pub struct ProjectIndex {
    files: Vec<ContextFile>,
    pub limited: bool,
}
impl ProjectIndex {
    pub fn discover(cwd: &Path) -> Self {
        let mut index = Self::default();
        let Ok(current) = cwd.canonicalize() else {
            return index;
        };
        if forbidden_directory(&current) || is_home(&current) || !current.is_dir() {
            return index;
        }
        let repo = repository_root(&current);
        let boundary = repo.as_deref().unwrap_or(&current);
        let deadline = Instant::now() + SEARCH_TIME;
        let Some(rules) = IgnoreRules::new(repo.as_deref(), &current) else {
            index.limited = true;
            return index;
        };
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        // Nearest manifests and workspace ancestors come first. Never ascend beyond Git root.
        for directory in current.ancestors().take(32) {
            if !directory.starts_with(boundary) || is_home(directory) {
                break;
            }
            for name in NAMES {
                candidates.push(directory.join(name));
            }
            if directory == boundary {
                break;
            }
        }
        index.read_candidates(&candidates, &rules, boundary, &mut seen, deadline);
        let mut layer = vec![current.clone()];
        let mut entries = 0;
        let mut directories = 0;
        for depth in 0..=DEPTH {
            let mut next = Vec::new();
            for directory in layer {
                if entries >= ENTRIES
                    || directories >= DIRECTORIES
                    || Instant::now() >= deadline
                    || index.files.len() >= MANIFESTS
                {
                    index.limited = true;
                    break;
                }
                directories += 1;
                let Ok(items) = fs::read_dir(&directory) else {
                    continue;
                };
                for item in items {
                    if entries >= ENTRIES || Instant::now() >= deadline {
                        index.limited = true;
                        break;
                    }
                    entries += 1;
                    let Ok(item) = item else {
                        continue;
                    };
                    let Ok(kind) = item.file_type() else {
                        continue;
                    };
                    if !kind.is_dir() {
                        continue;
                    }
                    let path = item.path();
                    if forbidden_directory(&path)
                        || item.file_name().to_string_lossy().starts_with('.')
                    {
                        continue;
                    }
                    if depth == DEPTH {
                        index.limited = true;
                        continue;
                    }
                    // Nested repositories are separate projects, not implicit context.
                    if path.join(".git").exists() {
                        continue;
                    }
                    next.push(path);
                }
            }
            next.sort();
            let allowed = allowed_paths(&next, &rules, deadline);
            next.retain(|p| allowed.contains(p));
            let remaining = DIRECTORIES.saturating_sub(directories);
            if next.len() > remaining {
                next.truncate(remaining);
                index.limited = true;
            }
            if next.is_empty() {
                break;
            }
            let candidates: Vec<_> = next
                .iter()
                .flat_map(|p| NAMES.iter().map(move |n| p.join(n)))
                .collect();
            index.read_candidates(&candidates, &rules, boundary, &mut seen, deadline);
            layer = next;
        }
        index.limited |= Instant::now() >= deadline;
        index
    }
    fn read_candidates(
        &mut self,
        candidates: &[PathBuf],
        rules: &IgnoreRules,
        boundary: &Path,
        seen: &mut HashSet<(u64, u64)>,
        deadline: Instant,
    ) {
        let existing: Vec<_> = candidates
            .iter()
            .take_while(|_| Instant::now() < deadline)
            .filter(|p| {
                fs::symlink_metadata(p)
                    .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 8192)
            })
            .cloned()
            .collect();
        let allowed = allowed_paths(&existing, rules, deadline);
        for path in existing {
            if Instant::now() >= deadline || self.files.len() >= MANIFESTS {
                self.limited = true;
                break;
            }
            if !allowed.contains(&path) {
                continue;
            }
            use std::os::unix::fs::MetadataExt;
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            let Some(raw) = read_small_text(&path) else {
                continue;
            };
            if contains_secret(&raw) || has_unsafe_controls(&raw) {
                continue;
            }
            let content = if path.file_name().is_some_and(|n| n == "package.json") {
                let Some(content) = compact_package(&path, &raw) else {
                    continue;
                };
                content
            } else {
                format!(
                    "File excerpt (up to 2048 characters):\n{}",
                    raw.chars().take(2048).collect::<String>()
                )
            };
            if self.files.iter().map(|f| f.content.len()).sum::<usize>() + content.len()
                > INDEX_BYTES
            {
                self.limited = true;
                continue;
            }
            let label = path
                .strip_prefix(boundary)
                .unwrap_or(&path)
                .to_string_lossy();
            if has_unsafe_controls(&label) || contains_secret(&label) {
                continue;
            }
            self.files.push(ContextFile {
                path: label.into_owned(),
                content,
            });
        }
    }
    /// Rank locally; only a few useful manifests, never the complete directory index, are sent.
    pub fn select(&self, question: &str) -> Vec<ContextFile> {
        let q = question.to_lowercase();
        let javascript = [
            "npm",
            "pnpm",
            "yarn",
            "bun",
            "node",
            "javascript",
            "typescript",
            "react",
            "vite",
        ]
        .iter()
        .any(|s| q.contains(s));
        let readme = q.split_whitespace().any(|w| {
            matches!(
                w.trim_matches(|c: char| c == '?' || c == ',' || c == '.'),
                "readme" | "readme.md" | "readme.txt"
            ) || w.ends_with("/readme.md")
        }) || ["documentation", "docs", "dokumentation"]
            .iter()
            .any(|word| q.contains(word));
        let words: Vec<_> = q
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .filter(|w| w.len() >= 3)
            .collect();
        let mut ranked: Vec<_> = self
            .files
            .iter()
            .filter(|f| {
                (!javascript
                    || f.path.ends_with("package.json")
                    || (readme
                        && Path::new(&f.path).file_name().is_some_and(|n| {
                            n.to_string_lossy()
                                .to_ascii_lowercase()
                                .starts_with("readme")
                        })))
                    && (readme
                        || !Path::new(&f.path).file_name().is_some_and(|n| {
                            n.to_string_lossy()
                                .to_ascii_lowercase()
                                .starts_with("readme")
                        }))
            })
            .map(|f| {
                let label = f.path.to_lowercase();
                let content = f.content.to_lowercase();
                let score: usize = words
                    .iter()
                    .map(|w| {
                        if label.contains(w) {
                            20
                        } else if content.contains(w) {
                            3
                        } else {
                            0
                        }
                    })
                    .sum();
                (score, f)
            })
            .collect();
        ranked.sort_by(|(sa, a), (sb, b)| {
            sb.cmp(sa)
                .then(
                    a.path
                        .matches('/')
                        .count()
                        .cmp(&b.path.matches('/').count()),
                )
                .then(a.path.cmp(&b.path))
        });
        let mut selected = Vec::new();
        let mut bytes = 0;
        // Keep the workspace's package manager alongside a matching child package.
        if let Some((_, root)) = ranked.iter().find(|(_, f)| f.path == "package.json") {
            let size = encoded_size(root);
            if size <= ATTACH_BYTES {
                selected.push((*root).clone());
                bytes += size;
            }
        }
        for (_, file) in ranked {
            if selected.len() >= 4 {
                break;
            }
            if selected.iter().any(|f| f.path == file.path) {
                continue;
            }
            let size = encoded_size(file);
            if bytes + size > ATTACH_BYTES {
                continue;
            }
            selected.push(file.clone());
            bytes += size;
        }
        selected
    }
}

fn encoded_size(file: &ContextFile) -> usize {
    serde_json::to_string(&format!(
        "\n--- {} ---\n{}\n--- End of file ---\n",
        file.path, file.content
    ))
    .map_or(ATTACH_BYTES + 1, |s| s.len())
}

fn compact_package(path: &Path, raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    let mut out = serde_json::Map::new();
    for key in ["name", "packageManager", "scripts", "workspaces", "engines"] {
        if let Some(value) = object.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    let dependencies: Vec<_> = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|k| object.get(*k)?.as_object())
        .flat_map(|d| d.keys())
        .take(64)
        .cloned()
        .collect();
    if !dependencies.is_empty() {
        out.insert("dependencyNames".into(), serde_json::json!(dependencies));
    }
    let directory = path.parent()?;
    let locks: Vec<_> = [
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "package-lock.json",
    ]
    .into_iter()
    .filter(|n| fs::symlink_metadata(directory.join(n)).is_ok_and(|m| m.file_type().is_file()))
    .collect();
    if !locks.is_empty() {
        out.insert("lockfiles".into(), serde_json::json!(locks));
    }
    let content = serde_json::to_string(&out).ok()?;
    if content.len() > 4096 {
        return None;
    }
    Some(format!("Manifest excerpt (selected keys): {content}"))
}

/// Batch Git's complete ignore rules, including tracked files and global excludes.
/// Files back the pipes so neither a full input pipe nor child output can deadlock the UI.
struct IgnoreRules {
    root: PathBuf,
    temporary_git: Option<tempfile::TempDir>,
}
impl IgnoreRules {
    fn new(repo: Option<&Path>, current: &Path) -> Option<Self> {
        if let Some(repo) = repo {
            return Some(Self {
                root: repo.to_owned(),
                temporary_git: None,
            });
        }
        // Git evaluates its full ignore language outside repositories using private,
        // temporary administrative scaffolding. No workspace files, index or logs are written.
        let dir = tempfile::tempdir().ok()?;
        fs::create_dir(dir.path().join("objects")).ok()?;
        fs::create_dir(dir.path().join("refs")).ok()?;
        fs::write(dir.path().join("HEAD"), "ref: refs/heads/amber-context\n").ok()?;
        Some(Self {
            root: current.to_owned(),
            temporary_git: Some(dir),
        })
    }
}
fn allowed_paths(paths: &[PathBuf], rules: &IgnoreRules, deadline: Instant) -> HashSet<PathBuf> {
    if paths.is_empty() || Instant::now() >= deadline {
        return HashSet::new();
    }
    let run = || -> Option<HashSet<PathBuf>> {
        let mut input = tempfile::tempfile().ok()?;
        for path in paths {
            use std::os::unix::ffi::OsStrExt;
            input.write_all(path.as_os_str().as_bytes()).ok()?;
            input.write_all(&[0]).ok()?;
        }
        input.rewind().ok()?;
        let mut output = tempfile::tempfile().ok()?;
        let mut command = Command::new("/usr/bin/git");
        for (name, _) in
            std::env::vars_os().filter(|(n, _)| n.to_string_lossy().starts_with("GIT_"))
        {
            command.env_remove(name);
        }
        command.arg("-C").arg(&rules.root);
        if let Some(dir) = &rules.temporary_git {
            let mut git_dir = std::ffi::OsString::from("--git-dir=");
            git_dir.push(dir.path());
            let mut work_tree = std::ffi::OsString::from("--work-tree=");
            work_tree.push(&rules.root);
            command
                .arg(git_dir)
                .arg(work_tree)
                .args(["-c", "core.bare=false"]);
        }
        let mut child = command
            .args(["check-ignore", "--no-index", "-z", "--stdin"])
            .stdin(input)
            .stdout(output.try_clone().ok()?)
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        if !matches!(status.code(), Some(0 | 1)) {
            return None;
        }
        output.rewind().ok()?;
        let mut bytes = Vec::new();
        output.take(128 * 1024 + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() > 128 * 1024 {
            return None;
        }
        use std::os::unix::ffi::OsStrExt;
        let ignored: HashSet<_> = bytes
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| PathBuf::from(std::ffi::OsStr::from_bytes(b)))
            .collect();
        Some(
            paths
                .iter()
                .filter(|p| !ignored.contains(*p))
                .cloned()
                .collect(),
        )
    };
    run().unwrap_or_default()
}
