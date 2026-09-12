//! The window, from a path on the command line. A file dialog is the next
//! slice; an argument is testable and needs no fourth dependency.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: relabel <label.json>");
        return std::process::ExitCode::FAILURE;
    };
    match relabel_gui::run_app(std::path::Path::new(&path)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("relabel: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
