use std::io::{BufRead, BufReader, Read, Write as _};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Error, Result};

#[derive(Clone, Copy, Debug)]
pub struct ValidationTimeouts {
    pub uci: Duration,
    pub ready: Duration,
    pub search: Duration,
    pub quit: Duration,
}

impl Default for ValidationTimeouts {
    fn default() -> Self {
        Self {
            uci: Duration::from_secs(10),
            ready: Duration::from_secs(10 * 60),
            search: Duration::from_secs(15),
            quit: Duration::from_secs(2),
        }
    }
}

const MAX_OUTPUT_LINE_BYTES: u64 = 16 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const READER_CLEANUP_TIMEOUT: Duration = Duration::from_millis(100);

enum OutputEvent {
    Line(String),
    Rejected(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UciIdentity {
    pub name: Option<String>,
    pub author: Option<String>,
    pub bestmove: String,
}

pub fn validate_engine(
    executable: &Path,
    working_directory: &Path,
    timeouts: ValidationTimeouts,
) -> Result<UciIdentity> {
    validate_engine_with_cancel(
        executable,
        working_directory,
        timeouts,
        &AtomicBool::new(false),
    )
}

pub fn validate_engine_with_cancel(
    executable: &Path,
    working_directory: &Path,
    timeouts: ValidationTimeouts,
    cancel: &AtomicBool,
) -> Result<UciIdentity> {
    check_cancel(cancel)?;
    if !executable.is_file() {
        return Err(Error::MissingInstall(executable.to_path_buf()));
    }
    let mut command = Command::new(executable);
    command
        .current_dir(working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|source| Error::UciValidation(format!("could not start engine: {source}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::UciValidation("could not capture engine output".into()))?;
    let (sender, receiver) = mpsc::sync_channel::<OutputEvent>(256);
    let (reader_done_sender, reader_done_receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        read_bounded_output(BufReader::new(stdout), &sender);
        let _ = reader_done_sender.send(());
    });
    let result = run_protocol(&mut child, &receiver, timeouts, cancel);
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    drop(receiver);
    finish_reader(reader, &reader_done_receiver);
    result
}

fn finish_reader(reader: thread::JoinHandle<()>, done: &Receiver<()>) {
    match done.recv_timeout(READER_CLEANUP_TIMEOUT) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = reader.join();
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            drop(reader);
        }
    }
}

fn run_protocol(
    child: &mut Child,
    receiver: &Receiver<OutputEvent>,
    timeouts: ValidationTimeouts,
    cancel: &AtomicBool,
) -> Result<UciIdentity> {
    check_cancel(cancel)?;
    send(child, "uci")?;
    let (name, author) = wait_for_uciok(receiver, timeouts.uci, cancel)?;
    send(child, "isready")?;
    wait_for_exact(receiver, "readyok", timeouts.ready, cancel)?;
    send(child, "ucinewgame")?;
    send(child, "position startpos")?;
    send(child, "go depth 1")?;
    let bestmove = wait_for_bestmove(receiver, timeouts.search, cancel)?;
    check_cancel(cancel)?;
    send(child, "quit")?;
    let deadline = Instant::now() + timeouts.quit;
    loop {
        check_cancel(cancel)?;
        if child
            .try_wait()
            .map_err(|source| Error::UciValidation(format!("could not wait for engine: {source}")))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .map_err(|source| Error::UciValidation(format!("engine ignored quit: {source}")))?;
            return Err(Error::UciValidation(
                "engine did not exit after quit".into(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(UciIdentity {
        name,
        author,
        bestmove,
    })
}

fn send(child: &mut Child, command: &str) -> Result<()> {
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| Error::UciValidation("engine input closed unexpectedly".into()))?;
    writeln!(stdin, "{command}")
        .and_then(|()| stdin.flush())
        .map_err(|source| Error::UciValidation(format!("could not send `{command}`: {source}")))
}

fn wait_for_uciok(
    receiver: &Receiver<OutputEvent>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(Option<String>, Option<String>)> {
    let deadline = Instant::now() + timeout;
    let mut name = None;
    let mut author = None;
    loop {
        let line = receive_before(receiver, deadline, "uciok", cancel)?;
        if let Some(value) = line.strip_prefix("id name ") {
            name = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("id author ") {
            author = Some(value.trim().to_owned());
        } else if line.trim() == "uciok" {
            return Ok((name, author));
        }
    }
}

fn wait_for_exact(
    receiver: &Receiver<OutputEvent>,
    expected: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if receive_before(receiver, deadline, expected, cancel)?.trim() == expected {
            return Ok(());
        }
    }
}

fn wait_for_bestmove(
    receiver: &Receiver<OutputEvent>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let line = receive_before(receiver, deadline, "bestmove", cancel)?;
        let Some(value) = line.strip_prefix("bestmove ") else {
            continue;
        };
        let movement = value.split_whitespace().next().unwrap_or_default();
        if valid_starting_move(movement) {
            return Ok(movement.to_owned());
        }
        return Err(Error::UciValidation(format!(
            "engine returned invalid bestmove `{movement}`"
        )));
    }
}

fn receive_before(
    receiver: &Receiver<OutputEvent>,
    deadline: Instant,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<String> {
    loop {
        check_cancel(cancel)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::UciValidation(format!(
                "timed out waiting for {expected}"
            )));
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(OutputEvent::Line(line)) => return Ok(line),
            Ok(OutputEvent::Rejected(reason)) => {
                return Err(Error::UciValidation(reason.into()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::UciValidation(format!(
                    "engine exited before {expected}"
                )));
            }
        }
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn read_bounded_output(mut reader: impl BufRead, sender: &mpsc::SyncSender<OutputEvent>) {
    let mut total = 0_u64;
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_OUTPUT_LINE_BYTES + 1)
            .read_until(b'\n', &mut bytes);
        let Ok(read) = read else {
            break;
        };
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if bytes.len() as u64 > MAX_OUTPUT_LINE_BYTES {
            let _ = sender.send(OutputEvent::Rejected(
                "engine emitted a line larger than 16 KiB",
            ));
            break;
        }
        if total > MAX_TOTAL_OUTPUT_BYTES {
            let _ = sender.send(OutputEvent::Rejected(
                "engine emitted more than 4 MiB during validation",
            ));
            break;
        }
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        let line = String::from_utf8_lossy(&bytes).into_owned();
        if sender.send(OutputEvent::Line(line)).is_err() {
            break;
        }
    }
}

fn valid_starting_move(value: &str) -> bool {
    matches!(
        value,
        "a2a3"
            | "a2a4"
            | "b2b3"
            | "b2b4"
            | "c2c3"
            | "c2c4"
            | "d2d3"
            | "d2d4"
            | "e2e3"
            | "e2e4"
            | "f2f3"
            | "f2f4"
            | "g2g3"
            | "g2g4"
            | "h2h3"
            | "h2h4"
            | "b1a3"
            | "b1c3"
            | "g1f3"
            | "g1h3"
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn validates_coordinate_moves() {
        assert!(valid_starting_move("e2e4"));
        assert!(valid_starting_move("g1f3"));
        assert!(!valid_starting_move("e2e5"));
        assert!(!valid_starting_move("a7a8q"));
        assert!(!valid_starting_move("(none)"));
    }

    #[test]
    fn rejects_unbounded_engine_lines() {
        let bytes = vec![b'x'; 16 * 1024 + 1];
        let (sender, receiver) = mpsc::sync_channel(1);
        read_bounded_output(Cursor::new(bytes), &sender);
        assert!(matches!(receiver.recv().unwrap(), OutputEvent::Rejected(_)));
    }

    #[cfg(unix)]
    #[test]
    fn validates_fake_uci_engine() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("fake engine");
        fs::write(
            &engine,
            "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\nuci) printf 'id name Fixture Engine\\nuciok\\n';;\nisready) printf 'readyok\\n';;\n'go depth 1') printf 'bestmove e2e4\\n';;\nquit) exit 0;;\nesac\ndone\n",
        )
        .unwrap();
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
        let identity =
            validate_engine(&engine, temporary.path(), ValidationTimeouts::default()).unwrap();
        assert_eq!(identity.name.as_deref(), Some("Fixture Engine"));
        assert_eq!(identity.bestmove, "e2e4");
    }

    #[cfg(unix)]
    #[test]
    fn descendant_holding_stdout_does_not_block_reader_cleanup() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("engine-with-descendant");
        fs::write(
            &engine,
            "#!/bin/sh\nsleep 2 &\nwhile IFS= read -r line; do\ncase \"$line\" in\nuci) printf 'uciok\\n';;\nisready) printf 'readyok\\n';;\n'go depth 1') printf 'bestmove e2e4\\n';;\nquit) exit 0;;\nesac\ndone\n",
        )
        .unwrap();
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();

        let started = Instant::now();
        validate_engine(&engine, temporary.path(), ValidationTimeouts::default()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_interrupts_readiness_wait_and_kills_engine() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;
        use std::sync::Arc;

        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("waiting-engine");
        fs::write(
            &engine,
            "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\nuci) printf 'uciok\\n';;\nquit) exit 0;;\nesac\ndone\n",
        )
        .unwrap();
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancel);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            trigger.store(true, Ordering::Relaxed);
        });
        let error = validate_engine_with_cancel(
            &engine,
            temporary.path(),
            ValidationTimeouts::default(),
            &cancel,
        )
        .unwrap_err();
        setter.join().unwrap();
        assert!(matches!(error, Error::Cancelled));
    }
}
