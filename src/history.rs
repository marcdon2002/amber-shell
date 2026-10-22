//! Bounded local history of completed Amber replies, separate from model conversation input.
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_FILE: usize = 128 * 1024;
const MAX_ENTRIES: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryEntry {
    pub question: String,
    pub explanation: String,
    pub command: String,
    pub cwd: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHistory {
    version: u32,
    entries: Vec<HistoryEntry>,
}

/// Returns oldest-to-newest entries for this exact canonical working directory.
/// Reading does not create files and does not add anything to model requests.
pub fn load_for(cwd: &Path) -> Result<Vec<HistoryEntry>, String> {
    load_for_mode(cwd, false)
}
/// Simulation has its own file and never reads real replies.
pub fn load_for_mode(cwd: &Path, simulated: bool) -> Result<Vec<HistoryEntry>, String> {
    load_at_mode(&history_directory()?, cwd, simulated)
}
/// Call only for a validated, completed reply. This function never executes a command.
/// The caller-supplied cwd field is replaced with the canonical cwd argument.
pub fn record(cwd: &Path, entry: HistoryEntry) -> Result<(), String> {
    record_mode(cwd, entry, false)
}
/// Records a completed reply only in the selected real or simulation namespace.
pub fn record_mode(cwd: &Path, entry: HistoryEntry, simulated: bool) -> Result<(), String> {
    record_at_mode(&history_directory()?, cwd, entry, simulated)
}
fn history_directory() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            PathBuf::from(std::env::var_os("HOME").ok_or("HOME fehlt; Verlauf nicht verfügbar.")?)
                .join(".config")
        }
    };
    if !base.is_absolute() {
        return Err("Der Verlaufsordner muss ein absoluter Pfad sein.".into());
    }
    Ok(base.join("amber-shell"))
}
fn canonical_cwd(cwd: &Path) -> Result<String, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|_| "Arbeitsverzeichnis für Verlauf nicht bestimmbar.")?;
    if !cwd.is_dir() {
        return Err("Verlauf benötigt ein Arbeitsverzeichnis.".into());
    }
    let value = cwd
        .to_str()
        .ok_or("Arbeitsverzeichnis für Verlauf ist kein UTF-8.")?;
    if value.len() > 4096 {
        return Err("Arbeitsverzeichnis für Verlauf ist zu lang.".into());
    }
    Ok(value.into())
}
fn validate_entry(entry: &HistoryEntry) -> Result<(), String> {
    if entry.question.trim().is_empty()
        || entry.question.len() > 8192
        || entry.explanation.len() > 16384
        || entry.cwd.len() > 4096
        || !Path::new(&entry.cwd).is_absolute()
        || [&entry.question, &entry.explanation].iter().any(|text| {
            text.chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        })
    {
        return Err("Verlaufseintrag ist ungültig oder überschreitet das Größenlimit.".into());
    }
    crate::model::validate_command(&entry.command)
        .map_err(|_| "Verlaufseintrag enthält keinen gültigen Befehl.".into())
}
fn open_directory(path: &Path, create: bool) -> Result<Option<File>, String> {
    if create {
        fs::create_dir_all(path.parent().ok_or("Ungültiger Verlaufsordner.")?)
            .map_err(|_| "Verlaufsordner konnte nicht erstellt werden.")?;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Verlaufsordner konnte nicht erstellt werden.".into()),
        }
    }
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(_) => return Err("Unsicherer oder nicht lesbarer Verlaufsordner.".into()),
    };
    let metadata = directory
        .metadata()
        .map_err(|_| "Verlaufsordner konnte nicht geprüft werden.")?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(
            "Verlaufsordner muss dem Benutzer gehören und darf nicht fremd beschreibbar sein."
                .into(),
        );
    }
    Ok(Some(directory))
}
fn open_private(directory: &File, name: &CStr, flags: i32) -> Result<Option<File>, String> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        if flags & libc::O_CREAT == 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            return Ok(None);
        }
        return Err("Verlaufsdatei konnte nicht sicher geöffnet werden.".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|_| "Verlaufsdatei konnte nicht geprüft werden.")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_FILE as u64
    {
        return Err("Verlauf benötigt eine private eigene reguläre Datei bis 128 KiB.".into());
    }
    Ok(Some(file))
}
fn history_name(simulated: bool) -> &'static CStr {
    if simulated {
        c"history-simulated.json"
    } else {
        c"history.json"
    }
}
fn lock_name(simulated: bool) -> &'static CStr {
    if simulated {
        c"history-simulated.lock"
    } else {
        c"history.lock"
    }
}
fn read_entries(directory: &File, simulated: bool) -> Result<Vec<HistoryEntry>, String> {
    let Some(file) = open_private(directory, history_name(simulated), libc::O_RDONLY)? else {
        return Ok(vec![]);
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Verlauf konnte nicht gelesen werden.")?;
    if bytes.len() > MAX_FILE {
        return Err("Verlauf überschreitet 128 KiB.".into());
    }
    let history: StoredHistory = serde_json::from_slice(&bytes)
        .map_err(|_| "Gespeicherter Verlauf ist ungültig; Datei bitte prüfen.")?;
    if history.version != 1 || history.entries.len() > MAX_ENTRIES {
        return Err(
            "Gespeicherter Verlauf hat eine ungültige Version oder zu viele Einträge.".into(),
        );
    }
    for entry in &history.entries {
        validate_entry(entry)?;
    }
    Ok(history.entries)
}
#[cfg(test)]
fn load_at(directory: &Path, cwd: &Path) -> Result<Vec<HistoryEntry>, String> {
    load_at_mode(directory, cwd, false)
}
fn load_at_mode(
    directory: &Path,
    cwd: &Path,
    simulated: bool,
) -> Result<Vec<HistoryEntry>, String> {
    let cwd = canonical_cwd(cwd)?;
    let Some(directory) = open_directory(directory, false)? else {
        return Ok(vec![]);
    };
    Ok(read_entries(&directory, simulated)?
        .into_iter()
        .filter(|entry| entry.cwd == cwd)
        .collect())
}
#[cfg(test)]
fn record_at(directory: &Path, cwd: &Path, entry: HistoryEntry) -> Result<(), String> {
    record_at_mode(directory, cwd, entry, false)
}
fn record_at_mode(
    directory: &Path,
    cwd: &Path,
    mut entry: HistoryEntry,
    simulated: bool,
) -> Result<(), String> {
    entry.cwd = canonical_cwd(cwd)?;
    validate_entry(&entry)?;
    let directory = open_directory(directory, true)?.ok_or("Verlaufsordner fehlt.")?;
    // A persistent inode serializes atomic replacements across terminal sessions.
    // Never wait on another writer while the interactive terminal is in raw mode.
    let lock = open_private(
        &directory,
        lock_name(simulated),
        libc::O_RDWR | libc::O_CREAT,
    )?
    .ok_or("Verlaufssperre fehlt.")?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(
            "Verlauf wird gerade in einem anderen Terminal gespeichert; Eintrag nicht gespeichert."
                .into(),
        );
    }
    let mut entries = read_entries(&directory, simulated)?;
    entries.push(entry);
    if entries.len() > MAX_ENTRIES {
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    let mut history = StoredHistory {
        version: 1,
        entries,
    };
    let bytes = loop {
        let bytes = serde_json::to_vec(&history)
            .map_err(|_| "Verlauf konnte nicht serialisiert werden.")?;
        if bytes.len() < MAX_FILE {
            break bytes;
        }
        if history.entries.len() <= 1 {
            return Err("Verlaufseintrag überschreitet das Dateilimit.".into());
        }
        history.entries.remove(0);
    };
    atomic_write(&directory, &bytes, simulated)
}
fn atomic_write(directory: &File, bytes: &[u8], simulated: bool) -> Result<(), String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut temporary = None;
    for _ in 0..64 {
        let name = CString::new(format!(
            ".history.{}.{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            temporary = Some((name, unsafe { File::from_raw_fd(fd) }));
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
            return Err("Temporäre Verlaufsdatei konnte nicht erstellt werden.".into());
        }
    }
    let (name, mut file) =
        temporary.ok_or("Temporäre Verlaufsdatei konnte nicht erstellt werden.")?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|_| "Verlauf konnte nicht gespeichert werden.".to_string())?;
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                name.as_ptr(),
                directory.as_raw_fd(),
                history_name(simulated).as_ptr(),
            )
        } != 0
        {
            return Err("Verlauf konnte nicht atomar ersetzt werden.".into());
        }
        directory
            .sync_all()
            .map_err(|_| "Verlaufsordner konnte nicht synchronisiert werden.".into())
    })();
    unsafe {
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    fn entry(number: usize) -> HistoryEntry {
        HistoryEntry {
            question: format!("Frage {number}"),
            explanation: "Eine kurze Erklärung".into(),
            command: format!("  printf '%s\\n' 'ä e\u{301} 👩‍💻 {number}'\n\n"),
            cwd: "caller cannot select another directory".into(),
        }
    }
    #[test]
    fn absent_history_reads_empty_without_creating_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        assert!(load_at(&directory, temp.path()).unwrap().is_empty());
        assert!(!directory.exists());
    }
    #[test]
    fn simulated_history_is_isolated_from_real_history_and_its_lock() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        record_at_mode(&directory, temp.path(), entry(1), false).unwrap();
        assert!(
            load_at_mode(&directory, temp.path(), true)
                .unwrap()
                .is_empty()
        );
        let dir = open_directory(&directory, false).unwrap().unwrap();
        let lock = open_private(&dir, c"history.lock", libc::O_RDWR)
            .unwrap()
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        record_at_mode(&directory, temp.path(), entry(2), true).unwrap();
        assert_eq!(
            load_at_mode(&directory, temp.path(), false).unwrap()[0].question,
            "Frage 1"
        );
        let simulated = load_at_mode(&directory, temp.path(), true).unwrap();
        assert_eq!(simulated.len(), 1);
        assert_eq!(simulated[0].question, "Frage 2");
        for name in [
            "history.json",
            "history-simulated.json",
            "history.lock",
            "history-simulated.lock",
        ] {
            assert_eq!(
                fs::metadata(directory.join(name)).unwrap().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn reopened_history_keeps_exact_commands_and_is_scoped_to_canonical_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        let alias = temp.path().join("alias");
        symlink(&other, &alias).unwrap();
        record_at(&directory, temp.path(), entry(1)).unwrap();
        record_at(&directory, &other, entry(2)).unwrap();
        record_at(&directory, &alias, entry(3)).unwrap();
        let first = load_at(&directory, temp.path()).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].command, "  printf '%s\\n' 'ä e\u{301} 👩‍💻 1'\n\n");
        assert_eq!(
            first[0].cwd,
            temp.path().canonicalize().unwrap().to_str().unwrap()
        );
        let next = load_at(&directory, &other).unwrap();
        assert_eq!(
            next.iter().map(|e| e.question.as_str()).collect::<Vec<_>>(),
            ["Frage 2", "Frage 3"]
        );
    }
    #[test]
    fn twenty_latest_completed_entries_are_retained_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        for number in 0..25 {
            record_at(&directory, temp.path(), entry(number)).unwrap();
        }
        let loaded = load_at(&directory, temp.path()).unwrap();
        assert_eq!(loaded.len(), 20);
        assert_eq!(loaded.first().unwrap().question, "Frage 5");
        assert_eq!(loaded.last().unwrap().question, "Frage 24");
    }
    #[test]
    fn serialized_byte_limit_evicts_whole_old_entries_without_trimming_new_commands() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        let command = "\\".repeat(16000);
        for number in 0..8 {
            let mut item = entry(number);
            item.command = command.clone();
            item.explanation = "\\".repeat(16000);
            record_at(&directory, temp.path(), item).unwrap();
        }
        let loaded = load_at(&directory, temp.path()).unwrap();
        assert!(loaded.len() < 8 && !loaded.is_empty());
        assert_eq!(loaded.last().unwrap().question, "Frage 7");
        assert_eq!(loaded.last().unwrap().command, command);
        assert!(fs::metadata(directory.join("history.json")).unwrap().len() <= 128 * 1024);
    }
    #[test]
    fn invalid_or_oversized_entries_never_create_history() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        for command in ["", "\u{1b}[2J", "bad\0command", "bad\rcommand"] {
            let mut item = entry(1);
            item.command = command.into();
            assert!(record_at(&directory, temp.path(), item).is_err());
        }
        let mut item = entry(1);
        item.question = "x".repeat(8193);
        assert!(record_at(&directory, temp.path(), item).is_err());
        let mut item = entry(1);
        item.explanation = "x".repeat(16385);
        assert!(record_at(&directory, temp.path(), item).is_err());
        assert!(!directory.exists());
    }
    #[test]
    fn corrupt_or_oversized_existing_history_is_not_silently_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("history.json");
        for content in [b"not json".to_vec(), vec![b'x'; 128 * 1024 + 1]] {
            fs::write(&path, &content).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(load_at(&directory, temp.path()).is_err());
            assert!(record_at(&directory, temp.path(), entry(1)).is_err());
            assert_eq!(fs::read(&path).unwrap(), content);
        }
    }
    #[test]
    fn symlink_history_and_symlink_application_directory_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("untouched");
        fs::write(&target, "untouched").unwrap();
        let directory = temp.path().join("storage");
        fs::create_dir(&directory).unwrap();
        symlink(&target, directory.join("history.json")).unwrap();
        assert!(load_at(&directory, temp.path()).is_err());
        assert!(record_at(&directory, temp.path(), entry(1)).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        let alias = temp.path().join("storage-alias");
        symlink(&directory, &alias).unwrap();
        assert!(load_at(&alias, temp.path()).is_err());
        assert!(record_at(&alias, temp.path(), entry(1)).is_err());
    }
    #[test]
    fn saved_history_is_private_and_atomic_updates_leave_no_temporary_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        record_at(&directory, temp.path(), entry(1)).unwrap();
        record_at(&directory, temp.path(), entry(2)).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(directory.join("history.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fs::read_dir(&directory).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        let path = directory.join("history.json");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_at(&directory, temp.path()).is_err());
    }
    #[test]
    fn concurrent_writer_returns_promptly_and_preserves_existing_entries() {
        use std::{
            fs::File,
            os::fd::AsRawFd,
            time::{Duration, Instant},
        };
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        record_at(&directory, temp.path(), entry(1)).unwrap();
        let lock = File::open(directory.join("history.lock")).unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let started = Instant::now();
        assert!(record_at(&directory, temp.path(), entry(2)).is_err());
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(load_at(&directory, temp.path()).unwrap().len(), 1);
        drop(lock);
        record_at(&directory, temp.path(), entry(2)).unwrap();
        assert_eq!(load_at(&directory, temp.path()).unwrap().len(), 2);
    }
    #[test]
    fn hardlinked_and_fifo_history_are_rejected_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("storage");
        record_at(&directory, temp.path(), entry(1)).unwrap();
        let path = directory.join("history.json");
        fs::hard_link(&path, directory.join("other.json")).unwrap();
        assert!(load_at(&directory, temp.path()).is_err());
        assert!(record_at(&directory, temp.path(), entry(2)).is_err());
        fs::remove_file(&path).unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(load_at(&directory, temp.path()).is_err());
        assert!(record_at(&directory, temp.path(), entry(2)).is_err());
    }
}
