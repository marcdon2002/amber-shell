//! Official ChatGPT browser login in Amber's own Codex profile.
use std::{
    fs::{self, OpenOptions},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::Path,
    process::Command,
};

fn ensure_directory(path: &Path, private: bool) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("Could not create Amber's login directory.".into()),
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "Unsafe Amber login directory; symlinks are not allowed.")?;
    let metadata = directory
        .metadata()
        .map_err(|_| "Could not inspect Amber's login directory.")?;
    let forbidden = if private { 0o077 } else { 0o022 };
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & forbidden != 0 {
        return Err(
            "Amber's login directory must belong to you and be private (chmod 700).".into(),
        );
    }
    Ok(())
}

/// Every Codex invocation must use this profile, including status and logout.
/// No credentials are copied from the user's ordinary Codex installation.
pub fn command(program: &Path) -> Result<Command, String> {
    let directory = crate::settings::directory()?;
    fs::create_dir_all(
        directory
            .parent()
            .ok_or("Invalid Amber settings directory.")?,
    )
    .map_err(|_| "Could not create Amber's settings directory.")?;
    ensure_directory(&directory, false)?;
    let home = directory.join("codex");
    ensure_directory(&home, true)?;
    match fs::symlink_metadata(home.join("auth.json")) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0 =>
        {
            return Err(
                "Amber's auth.json must be a private regular file; links are not allowed.".into(),
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("Could not inspect Amber's auth.json.".into()),
    }
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
