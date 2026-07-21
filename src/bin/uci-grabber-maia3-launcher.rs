//! Minimal process-boundary launcher for a locally assembled Maia3 package.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use uci_grabber::registry::{
    PAYLOAD_REVIEW_COUNT_LEN, PAYLOAD_REVIEW_DIGEST_LEN, PAYLOAD_REVIEW_LEN,
    PAYLOAD_REVIEW_MARKER_LEN, PAYLOAD_REVIEW_PLACEHOLDER, PackageSnapshot,
    package_payload_snapshot,
};

const MODELS: [&str; 3] = ["maia3-5m", "maia3-23m", "maia3-79m"];

#[used]
static PAYLOAD_REVIEW: [u8; PAYLOAD_REVIEW_LEN] = *PAYLOAD_REVIEW_PLACEHOLDER;

#[allow(unsafe_code)]
fn embedded_payload_review() -> Result<PackageSnapshot, String> {
    // SAFETY: PAYLOAD_REVIEW is a live static byte array. A volatile read is
    // intentional because UCI Grabber personalizes these reserved bytes in the
    // installed executable after downloading and verifying the whole package.
    let bytes = unsafe { std::ptr::read_volatile(&raw const PAYLOAD_REVIEW) };
    if bytes[..PAYLOAD_REVIEW_MARKER_LEN] != PAYLOAD_REVIEW_PLACEHOLDER[..PAYLOAD_REVIEW_MARKER_LEN]
    {
        return Err("the embedded package-review marker is invalid".to_owned());
    }
    let digest_end = PAYLOAD_REVIEW_MARKER_LEN + PAYLOAD_REVIEW_DIGEST_LEN;
    let files_end = digest_end + PAYLOAD_REVIEW_COUNT_LEN;
    let digest = std::str::from_utf8(&bytes[PAYLOAD_REVIEW_MARKER_LEN..digest_end])
        .map_err(|_| "the embedded package digest is not UTF-8")?;
    if digest
        .as_bytes()
        .iter()
        .any(|byte| !byte.is_ascii_hexdigit())
        || digest.as_bytes().iter().any(u8::is_ascii_uppercase)
    {
        return Err("the launcher has not been personalized by UCI Grabber".to_owned());
    }
    let parse_count = |value: &[u8], label: &str| -> Result<u64, String> {
        let value =
            std::str::from_utf8(value).map_err(|_| format!("the embedded {label} is not UTF-8"))?;
        if !value.as_bytes().iter().all(u8::is_ascii_digit) {
            return Err(format!("the embedded {label} is invalid"));
        }
        value
            .parse::<u64>()
            .map_err(|error| format!("the embedded {label} is invalid: {error}"))
    };
    Ok(PackageSnapshot {
        sha256: digest.to_owned(),
        file_count: parse_count(&bytes[digest_end..files_end], "file count")?,
        byte_count: parse_count(&bytes[files_end..], "byte count")?,
    })
}

fn verify_payload(install_root: &Path, executable: &Path) -> Result<(), String> {
    let expected = embedded_payload_review()?;
    let actual = package_payload_snapshot(install_root, executable)
        .map_err(|error| format!("could not verify the locally assembled package: {error}"))?;
    if actual != expected {
        return Err(
            "the locally assembled Maia3 package has changed since installation".to_owned(),
        );
    }
    Ok(())
}

fn regular_file(path: &Path, description: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "{description} is unavailable at `{}`: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "{description} is not a regular file: `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn find_model(launcher_directory: &Path) -> Result<&'static str, String> {
    let models_directory = launcher_directory.join("models");
    let metadata = fs::symlink_metadata(&models_directory).map_err(|error| {
        format!(
            "Maia3 models directory is unavailable at `{}`: {error}",
            models_directory.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "Maia3 models path is not a directory: `{}`",
            models_directory.display()
        ));
    }

    let mut selected = None;
    for model in MODELS {
        let checkpoint = models_directory.join(format!("{model}.pt"));
        match fs::symlink_metadata(&checkpoint) {
            Ok(metadata) if metadata.file_type().is_file() => {
                if selected.replace(model).is_some() {
                    return Err(
                        "Maia3 launcher requires exactly one 5M, 23M, or 79M checkpoint".to_owned(),
                    );
                }
            }
            Ok(_) => {
                return Err(format!(
                    "Maia3 checkpoint is not a regular file: `{}`",
                    checkpoint.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "could not inspect Maia3 checkpoint `{}`: {error}",
                    checkpoint.display()
                ));
            }
        }
    }
    selected
        .ok_or_else(|| "Maia3 launcher requires exactly one 5M, 23M, or 79M checkpoint".to_owned())
}

fn installation_layout(executable: &Path) -> Result<(&Path, &Path), String> {
    let launcher_directory = executable
        .parent()
        .ok_or_else(|| "the Maia3 launcher has no parent directory".to_owned())?;
    if launcher_directory.file_name() != Some(OsStr::new("launcher")) {
        return Err("the Maia3 launcher must be installed in the `launcher` directory".to_owned());
    }
    let install_root = launcher_directory
        .parent()
        .ok_or_else(|| "the Maia3 launcher directory has no install root".to_owned())?;
    Ok((launcher_directory, install_root))
}

#[cfg(windows)]
fn python_path(install_root: &Path) -> PathBuf {
    install_root
        .join("python-runtime")
        .join("python")
        .join("python.exe")
}

#[cfg(unix)]
fn run_python(mut command: Command) -> Result<i32, String> {
    use std::os::unix::process::CommandExt as _;

    let error = command.exec();
    Err(format!(
        "could not replace the Maia3 launcher with portable Python: {error}"
    ))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn run_python(mut command: Command) -> Result<i32, String> {
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    struct KillOnCloseJob(HANDLE);

    impl KillOnCloseJob {
        fn new() -> Result<Self, String> {
            // SAFETY: Null security attributes/name request an unnamed job with
            // default security. The returned owned handle is closed by Drop.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(format!(
                    "could not create the Maia3 process job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is the structure required by the selected
            // information class and remains valid for the duration of the call.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    u32::try_from(std::mem::size_of_val(&limits))
                        .expect("job limit structure size fits in u32"),
                )
            };
            if configured == 0 {
                return Err(format!(
                    "could not configure the Maia3 process job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(job)
        }

        fn contain_current_process(&self) -> Result<(), String> {
            // SAFETY: `self.0` is a live owned job handle and
            // `GetCurrentProcess` returns a valid pseudo-handle for this
            // launcher. Children inherit the launcher's job membership.
            if unsafe { AssignProcessToJobObject(self.0, GetCurrentProcess()) } == 0 {
                return Err(format!(
                    "could not contain the Maia3 launcher process: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl Drop for KillOnCloseJob {
        fn drop(&mut self) {
            // SAFETY: this type owns the non-null handle and closes it once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    let job = KillOnCloseJob::new()?;
    job.contain_current_process()?;
    // Keep the unnamed, non-inheritable handle open until this launcher exits.
    // If FishEye terminates the launcher, Windows closes the handle and the
    // job atomically terminates Python and every descendant. Assigning the
    // launcher before spawning removes the spawn-to-assignment race.
    std::mem::forget(job);
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start portable Python: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for portable Python: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(not(windows))]
fn python_path(install_root: &Path) -> PathBuf {
    install_root
        .join("python-runtime")
        .join("python")
        .join("bin")
        .join("python3.12")
}

fn run() -> Result<i32, String> {
    if env::args_os().len() != 1 {
        return Err("the packaged Maia3 launcher does not accept arguments".to_owned());
    }

    let executable = env::current_exe()
        .map_err(|error| format!("could not locate the Maia3 launcher: {error}"))?
        .canonicalize()
        .map_err(|error| format!("could not resolve the Maia3 launcher: {error}"))?;
    let (launcher_directory, install_root) = installation_layout(&executable)?;
    let model = find_model(launcher_directory)?;
    let python = python_path(install_root);
    let entry_point = launcher_directory.join("maia3_entry.py");
    regular_file(&python, "portable Python interpreter")?;
    regular_file(&entry_point, "Maia3 Python entry point")?;
    verify_payload(install_root, &executable)?;

    let mut command = Command::new(&python);
    command
        .args([
            OsStr::new("-I"),
            OsStr::new("-B"),
            OsStr::new("-u"),
            entry_point.as_os_str(),
        ])
        .env("UCI_GRABBER_MODEL", model)
        .env("UCI_GRABBER_INSTALL_ROOT", install_root.as_os_str())
        .env("HF_HUB_OFFLINE", "1")
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1")
        .env("PIP_NO_INDEX", "1")
        .env("CUDA_VISIBLE_DEVICES", "")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    run_python(command)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("UCI Grabber Maia3 launcher: {message}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "uci-grabber-launcher-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn selects_exactly_one_regular_checkpoint() {
        let root = temporary_directory("one-model");
        fs::create_dir(root.join("models")).unwrap();
        fs::write(root.join("models/maia3-23m.pt"), b"fixture").unwrap();
        assert_eq!(find_model(&root).unwrap(), "maia3-23m");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_zero_or_multiple_checkpoints() {
        let root = temporary_directory("model-count");
        fs::create_dir(root.join("models")).unwrap();
        assert!(find_model(&root).is_err());
        fs::write(root.join("models/maia3-5m.pt"), b"fixture").unwrap();
        fs::write(root.join("models/maia3-79m.pt"), b"fixture").unwrap();
        assert!(find_model(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn derives_install_root_only_from_exact_launcher_directory() {
        let executable = Path::new("package/launcher/maia3-launcher");
        let (launcher, root) = installation_layout(executable).unwrap();
        assert_eq!(launcher, Path::new("package/launcher"));
        assert_eq!(root, Path::new("package"));
        assert!(installation_layout(Path::new("package/maia3-launcher")).is_err());
        assert!(installation_layout(Path::new("maia3-launcher")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_checkpoint_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("model-link");
        fs::create_dir(root.join("models")).unwrap();
        fs::write(root.join("target"), b"fixture").unwrap();
        symlink(root.join("target"), root.join("models/maia3-5m.pt")).unwrap();
        assert!(find_model(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
