//! The editor's one line about the printer runtime: it depends on relabel-core,
//! whose `link` API names no tokio or btleplug type, and never on either crate
//! itself. A tokio future polled on the Slint thread panics with no reactor
//! running, so a path to one in this crate is the first step to that panic.
//!
//! Read from the source and the manifest rather than from `cargo tree`, which
//! would need cargo and the network; both are plain text beside this file.

use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("the source folder reads") {
        let path = entry.expect("an entry reads").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The keys of a manifest table: `name = ...` and `name.workspace = ...`, with
/// comments and continuation lines left out.
fn keys_under(manifest: &str, table: &str) -> Vec<String> {
    let mut inside = false;
    let mut keys = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == format!("[{table}]");
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let key = key.split_once('.').map_or(key, |(k, _)| k);
        if !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            keys.push(key.to_owned());
        }
    }
    keys
}

#[test]
fn the_gui_names_no_runtime_crate() {
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    assert!(!files.is_empty());
    for file in &files {
        let text = fs::read_to_string(file).expect("a source file reads");
        for path in ["tokio::", "btleplug::"] {
            assert!(
                !text.contains(path),
                "{} names {path}",
                file.strip_prefix(crate_dir()).unwrap_or(file).display()
            );
        }
    }

    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).expect("the manifest reads");
    let deps = keys_under(&manifest, "dependencies");
    assert!(deps.iter().any(|k| k == "relabel-core"), "{deps:?}");
    for table in ["dependencies", "dev-dependencies"] {
        let keys = keys_under(&manifest, table);
        assert!(!keys.is_empty(), "[{table}] has keys");
        for runtime in ["tokio", "btleplug"] {
            assert!(
                !keys.iter().any(|k| k == runtime),
                "[{table}] has {runtime}"
            );
        }
    }
}
