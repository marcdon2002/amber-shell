//! Amber Shell's own settings; never reads or edits Codex authentication/configuration.
use crate::provider::{DEFAULT_MODEL, validate_model_id};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    #[default]
    Automatic,
    Terminal,
    Off,
}
impl ContextMode {
    pub fn choice(self) -> usize {
        match self {
            Self::Automatic => 3,
            Self::Terminal => 2,
            Self::Off => 4,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Terminal => "Terminal only",
            Self::Off => "Off",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub model: String,
    #[serde(default)]
    pub fast: bool,
    #[serde(default)]
    pub context: ContextMode,
}

pub fn load() -> Result<Settings, String> {
    load_settings_from(&settings_directory(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )?)
}
pub fn save(settings: &Settings) -> Result<(), String> {
    save_settings_to(
        &settings_directory(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )?,
        settings,
    )
}

fn settings_directory(xdg: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, String> {
    let base = match xdg.filter(|v| !v.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            PathBuf::from(home.ok_or("HOME is missing; cannot determine the settings directory.")?)
                .join(".config")
        }
    };
    if !base.is_absolute() {
        return Err("The settings directory must be an absolute path.".into());
    }
    Ok(base.join("amber-shell"))
}

pub fn load_model() -> Result<String, String> {
    load_from(&settings_directory(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )?)
}
pub fn save_model(model: &str) -> Result<(), String> {
    save_to(
        &settings_directory(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )?,
        model,
    )
}

fn open_directory(path: &Path, create: bool) -> Result<Option<File>, String> {
    if create {
        let parent = path.parent().ok_or("Invalid settings directory.")?;
        fs::create_dir_all(parent).map_err(|_| "Could not create the settings directory.")?;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Could not create the settings directory.".into()),
        }
    }
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(_) => {
            return Err(
                "Unsafe or unreadable settings directory (symlinks are not allowed).".into(),
            );
        }
    };
    let metadata = directory
        .metadata()
        .map_err(|_| "Could not inspect the settings directory.")?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err("The settings directory must belong to the current user and must not be writable by others.".into());
    }
    Ok(Some(directory))
}

fn existing_settings(directory: &File) -> Result<Option<File>, String> {
    // O_NONBLOCK also prevents a malicious FIFO from blocking before the type check.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"settings.json".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err("Unsafe or unreadable settings.json (symlinks are not allowed).".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|_| "Could not inspect settings.json.")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.len() > 4096
    {
        return Err(
            "settings.json must be a regular file owned by the current user, no larger than 4 KiB."
                .into(),
        );
    }
    Ok(Some(file))
}

fn load_from(path: &Path) -> Result<String, String> {
    load_settings_from(path).map(|settings| settings.model)
}
fn save_to(path: &Path, model: &str) -> Result<(), String> {
    validate_model_id(model)?;
    let mut settings = load_settings_from(path)?;
    settings.model = model.into();
    save_settings_to(path, &settings)
}

fn load_settings_from(path: &Path) -> Result<Settings, String> {
    let Some(directory) = open_directory(path, false)? else {
        return Ok(Settings {
            model: DEFAULT_MODEL.into(),
            fast: false,
            context: ContextMode::Automatic,
        });
    };
    let Some(file) = existing_settings(&directory)? else {
        return Ok(Settings {
            model: DEFAULT_MODEL.into(),
            fast: false,
            context: ContextMode::Automatic,
        });
    };
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read settings.json.")?;
    if bytes.len() > 4096 {
        return Err("settings.json exceeds the size limit.".into());
    }
    let settings: Settings = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid settings.json; save your model selection again.")?;
    validate_model_id(&settings.model)?;
    Ok(settings)
}

fn save_settings_to(path: &Path, settings: &Settings) -> Result<(), String> {
    validate_model_id(&settings.model)?;
    let directory = open_directory(path, true)?.ok_or("The settings directory is missing.")?;
    existing_settings(&directory)?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let mut temporary = None;
    for _ in 0..64 {
        let name = CString::new(format!(
            ".settings.{}.{}.tmp",
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
            return Err("Could not create a temporary settings file.".into());
        }
    }
    let (name, mut file) = temporary.ok_or("Could not create a temporary settings file.")?;
    let result = (|| -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|_| "Could not serialize the model selection.")?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|_| "Could not save the model selection.")?;
        // Both names are resolved against the opened directory, even if its path is replaced.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                name.as_ptr(),
                directory.as_raw_fd(),
                c"settings.json".as_ptr(),
            )
        } != 0
        {
            return Err("Could not atomically replace the model selection.".into());
        }
        directory
            .sync_all()
            .map_err(|_| "Could not synchronize the settings directory.".to_string())
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
    #[test]
    fn fast_choice_roundtrips_and_old_model_only_files_default_to_standard() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings {
            model: "gpt-6-luna".into(),
            fast: true,
            context: ContextMode::Terminal,
        };
        save_settings_to(temp.path(), &settings).unwrap();
        assert_eq!(load_settings_from(temp.path()).unwrap(), settings);
        fs::write(
            temp.path().join("settings.json"),
            r#"{"model":"gpt-6-astra"}"#,
        )
        .unwrap();
        assert_eq!(
            load_settings_from(temp.path()).unwrap(),
            Settings {
                model: "gpt-6-astra".into(),
                fast: false,
                context: ContextMode::Automatic
            }
        );
    }

    #[test]
    fn absent_settings_use_luna_without_creating_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("amber-shell");
        assert_eq!(load_from(&directory).unwrap(), "gpt-6-luna");
        assert!(!directory.exists());
    }
    #[test]
    fn selection_survives_atomic_replace_with_private_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("amber-shell");
        save_to(&directory, "gpt-6-luna").unwrap();
        save_to(&directory, "gpt-6-astra").unwrap();
        assert_eq!(load_from(&directory).unwrap(), "gpt-6-astra");
        assert_eq!(
            fs::metadata(directory.join("settings.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }
    #[test]
    fn corrupted_and_invalid_settings_do_not_become_default() {
        let temp = tempfile::tempdir().unwrap();
        for content in [
            "broken",
            r#"{"model":"../secret"}"#,
            r#"{"model":"gpt-6-luna\n"}"#,
            r#"{}"#,
        ] {
            fs::write(temp.path().join("settings.json"), content).unwrap();
            assert!(load_from(temp.path()).is_err(), "{content}");
        }
        for id in ["", "../secret", "gpt-6-luna\n", "/model", "foo bar"] {
            assert!(save_to(temp.path(), id).is_err(), "{id}");
        }
    }
    #[test]
    fn settings_symlink_and_directory_are_refused_without_touching_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::write(&target, "untouched").unwrap();
        let directory = temp.path().join("amber-shell");
        fs::create_dir(&directory).unwrap();
        let settings = directory.join("settings.json");
        symlink(&target, &settings).unwrap();
        assert!(load_from(&directory).is_err());
        assert!(save_to(&directory, "gpt-6-luna").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        fs::remove_file(&settings).unwrap();
        fs::create_dir(&settings).unwrap();
        assert!(load_from(&directory).is_err());
        assert!(save_to(&directory, "gpt-6-luna").is_err());
    }
    #[test]
    fn application_directory_symlink_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("amber-shell");
        symlink(temp.path(), &directory).unwrap();
        assert!(load_from(&directory).is_err());
        assert!(save_to(&directory, "gpt-6-luna").is_err());
    }
}
