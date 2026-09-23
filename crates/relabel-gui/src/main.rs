//! The window, on the label named on the command line or on a new one; files are
//! opened and saved through the platform's own dialog (choose.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    match relabel_gui::run_app(path.as_deref()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("relabel: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
