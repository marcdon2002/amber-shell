//! Official ChatGPT browser login in Amber's own Codex profile.
use std::{
    fs::{self, OpenOptions},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
};

fn ensure_directory(path: &Path, private: bool, create: bool) -> Result<bool, String> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("Could not create Amber's login directory.".into()),
        }
    }
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(directory) => directory,
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(_) => return Err("Unsafe Amber login directory; symlinks are not allowed.".into()),
    };
    let metadata = directory
        .metadata()
        .map_err(|_| "Could not inspect Amber's login directory.")?;
    let forbidden = if private { 0o077 } else { 0o022 };
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & forbidden != 0 {
        return Err(
            "Amber's login directory must belong to you and be private (chmod 700).".into(),
        );
    }
    Ok(true)
}

fn profile_directory(create: bool) -> Result<Option<PathBuf>, String> {
    let directory = crate::settings::directory()?;
    if create {
        fs::create_dir_all(
            directory
                .parent()
                .ok_or("Invalid Amber settings directory.")?,
        )
        .map_err(|_| "Could not create Amber's settings directory.")?;
    }
    if !ensure_directory(&directory, false, create)? {
        return Ok(None);
    }
    let home = directory.join("codex");
    if !ensure_directory(&home, true, create)? {
        return Ok(None);
    }
    Ok(Some(home))
}

fn credentials_metadata(home: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(home.join("auth.json")) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0 =>
        {
            Err("Amber's auth.json must be a private regular file; links are not allowed.".into())
        }
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Could not inspect Amber's auth.json.".into()),
    }
}

/// A local metadata check only: Codex owns credential parsing and account validation.
/// Missing credentials do not create directories or start a child process.
pub fn has_credentials() -> Result<bool, String> {
    let Some(home) = profile_directory(false)? else {
        return Ok(false);
    };
    Ok(credentials_metadata(&home)?.is_some_and(|metadata| metadata.len() > 0))
}

/// Every Codex invocation must use this profile, including status and logout.
/// No credentials are copied from the user's ordinary Codex installation.
pub fn command(program: &Path) -> Result<Command, String> {
    let home = profile_directory(true)?.ok_or("Could not create Amber's login directory.")?;
    credentials_metadata(&home)?;
    let mut command = Command::new(program);
    command.env("CODEX_HOME", home).args([
        "-c",
        "cli_auth_credentials_store=\"file\"",
        "-c",
        "forced_login_method=\"chatgpt\"",
    ]);
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "OPENAI_BASE_URL",
    ] {
        command.env_remove(key);
    }
    // Apply only in the child, leaving the user's shell and this process alone.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o077);
            Ok(())
        });
    }
    Ok(command)
}
