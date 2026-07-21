#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    if let Err(error) = uci_grabber::app::run_gui() {
        uci_grabber::app::show_startup_error(&error);
        std::process::exit(1);
    }
}
