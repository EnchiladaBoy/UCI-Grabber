//! Synthetic portable-Python stand-in for native launcher release tests.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    assert_eq!(arguments.len(), 5, "unexpected launcher argument count");
    assert_eq!(arguments[1], OsStr::new("-I"));
    assert_eq!(arguments[2], OsStr::new("-B"));
    assert_eq!(arguments[3], OsStr::new("-u"));
    assert_eq!(
        Path::new(&arguments[4]).file_name(),
        Some(OsStr::new("maia3_entry.py"))
    );
    assert_eq!(
        std::env::var("UCI_GRABBER_MODEL").as_deref(),
        Ok("maia3-5m")
    );
    assert!(std::env::var_os("UCI_GRABBER_INSTALL_ROOT").is_some());
    assert_eq!(std::env::var("PYTHONDONTWRITEBYTECODE").as_deref(), Ok("1"));

    if std::env::var_os("UCI_GRABBER_SMOKE_WAIT").is_some() {
        println!("SMOKE_PID={}", std::process::id());
        std::io::stdout().flush().expect("flush synthetic PID");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    println!("synthetic Maia3 child reached");
}
