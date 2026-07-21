use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::{Error, Result};

pub fn open_in_fisheye(engine: &Path, explicit_fisheye: Option<&Path>) -> Result<Child> {
    if !engine.is_file() {
        return Err(Error::MissingInstall(engine.to_path_buf()));
    }
    let fisheye = find_fisheye(explicit_fisheye)?;
    Command::new(&fisheye)
        .args(["gui", "--add-external-engine"])
        .arg(engine)
        .spawn()
        .map_err(|source| {
            Error::Other(format!(
                "could not launch FishEye at {}: {source}",
                fisheye.display()
            ))
        })
}

pub fn find_fisheye(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return executable_candidate(path).ok_or_else(|| {
            Error::Other(format!(
                "FishEye executable was not found at {}",
                path.display()
            ))
        });
    }
    if let Some(path) = env::var_os("FISHEYE_PATH") {
        let path = PathBuf::from(path);
        if let Some(path) = executable_candidate(&path) {
            return Ok(path);
        }
    }
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
        && let Some(path) = executable_candidate(&parent.join(executable_name()))
    {
        return Ok(path);
    }
    if let Some(search_path) = env::var_os("PATH") {
        for directory in env::split_paths(&search_path) {
            if let Some(path) = executable_candidate(&directory.join(executable_name())) {
                return Ok(path);
            }
        }
    }
    Err(Error::Other(
        "FishEye was not found; use --fisheye, set FISHEYE_PATH, or copy the engine path".into(),
    ))
}

pub fn reveal(path: &Path) -> Result<Child> {
    if !path.exists() {
        return Err(Error::MissingInstall(path.to_path_buf()));
    }
    #[cfg(target_os = "windows")]
    let child = Command::new("explorer.exe")
        .arg("/select,")
        .arg(path)
        .spawn();
    #[cfg(target_os = "macos")]
    let child = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let child = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    child.map_err(|source| Error::Other(format!("could not reveal {}: {source}", path.display())))
}

fn executable_candidate(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

#[cfg(target_os = "windows")]
fn executable_name() -> &'static str {
    "fisheye.exe"
}

#[cfg(not(target_os = "windows"))]
fn executable_name() -> &'static str {
    "fisheye"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_missing_fisheye_is_rejected() {
        assert!(find_fisheye(Some(Path::new("/definitely/missing/fisheye"))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn handoff_uses_exact_fisheye_cli_contract() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let fisheye = temporary.path().join("fake fisheye");
        let engine = temporary.path().join("engine with spaces");
        let output = temporary.path().join("arguments");
        fs::write(&engine, b"fixture").unwrap();
        fs::write(
            &fisheye,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                output.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fisheye, fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = open_in_fisheye(&engine, Some(&fisheye)).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            fs::read_to_string(output).unwrap(),
            format!("gui\n--add-external-engine\n{}\n", engine.to_string_lossy())
        );
    }
}
