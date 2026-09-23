//! Reading a label from disk and writing one back, with std alone.
//!
//! Every message leaves the file's name out: the caller knows which file it
//! asked for and puts the name in front, as the file name only, never the whole
//! path.
//!
//! # A save replaces the file whole or not at all
//!
//! The label is written to a new file beside the old one and renamed onto it, so
//! a full disk, a refused write or a crash part-way leaves the old file exactly
//! as it was. What the rename costs: the file is a new one, so a hard link to
//! the old one keeps the old text, and extended attributes and ACLs are not
//! carried across - only the permission bits are. The folder is not synced, so a
//! power cut just after a save can bring back the file as it was before it.

use relabel_render::Label;
use relabel_render::document::LoadError;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Far past any label: the reference label is under a kilobyte, and one carrying
/// a full-length image as base64 is a few hundred. The same cap relabel-cli puts
/// on a CSV, and the one the preview puts on a buffer.
pub const MAX_LABEL_BYTES: u64 = 64 << 20;

/// What a UTF-8 byte order mark looks like. Some Windows editors write one, and
/// JSON does not allow it.
const BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("it does not exist")]
    Missing,
    /// The length in bytes; said in megabytes, to the nearest.
    #[error("it is {} MB, far larger than any label", (.0 + 500_000) / 1_000_000)]
    TooLarge(u64),
    #[error("it is empty")]
    Empty,
    #[error("it is not UTF-8 text")]
    NotUtf8,
    #[error(transparent)]
    Label(#[from] LoadError),
    #[error(transparent)]
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> ReadError {
        match e.kind() {
            io::ErrorKind::NotFound => ReadError::Missing,
            _ => ReadError::Io(e),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// Refused rather than replaced: the rename would succeed in a folder the
    /// operator can write to, and replace a file they had marked not to be, or
    /// one they could not have written.
    #[error("it is read-only; Save as saves a copy")]
    ReadOnly,
    #[error("it is a link that points nowhere")]
    DanglingLink,
    /// The label itself would not load back; nothing was written.
    #[error(transparent)]
    Label(#[from] LoadError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Read a label: at most [`MAX_LABEL_BYTES`], one leading byte order mark
/// dropped, UTF-8 required.
pub fn read_label(path: &Path) -> Result<Label, ReadError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_LABEL_BYTES {
        return Err(ReadError::TooLarge(len));
    }
    // Capped as it is read as well: a pipe or a device reports no length, and a
    // file can grow between the two calls.
    let mut bytes = Vec::new();
    file.take(MAX_LABEL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LABEL_BYTES {
        return Err(ReadError::TooLarge(bytes.len() as u64));
    }
    let body = bytes.strip_prefix(BOM).unwrap_or(&bytes);
    if body.trim_ascii().is_empty() {
        return Err(ReadError::Empty);
    }
    let text = std::str::from_utf8(body).map_err(|_| ReadError::NotUtf8)?;
    Ok(Label::from_json(text)?)
}

/// Write a label to `path`, or nothing at all: a label that would not load back
/// is refused before a byte is written.
pub fn save(path: &Path, label: &Label) -> Result<(), SaveError> {
    let text = label.to_json()?;
    write_atomic(path, text.as_bytes())
}

/// Replace `target` with `bytes`, whole, or leave it as it was.
///
/// Written to `NAME.part` beside the target and renamed onto it: rename(2) on
/// Unix, and MoveFileExW with MOVEFILE_REPLACE_EXISTING on Windows (std 1.98.1
/// sys/fs/windows.rs:1321-1390), each of which replaces the target in one step.
/// A symbolic link is followed, so the link stays a link and the file it points
/// at is the one replaced. A `.part` file already there is stepped round, never
/// removed: it may be another program's, or the only copy of an earlier save.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let target = followed(target)?;
    let existing = match fs::metadata(&target) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if existing
        .as_ref()
        .is_some_and(|m| m.permissions().readonly())
    {
        return Err(SaveError::ReadOnly);
    }
    // The mode bits above say whether anyone may write the file, not whether this
    // operator may: in a folder shared with a group, a colleague's label left at
    // 0644 can be renamed over although no editor could have written it. Opening
    // it for writing, without truncating, puts that question to the system. Only a
    // regular file is opened, since opening a FIFO for writing waits for a reader;
    // any other failure is left to the rename to report.
    if existing.as_ref().is_some_and(fs::Metadata::is_file)
        && let Err(e) = OpenOptions::new().write(true).open(&target)
        && e.kind() == io::ErrorKind::PermissionDenied
    {
        return Err(SaveError::ReadOnly);
    }
    let path = staging_name(&target);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let staged = Unfinished { path, kept: false };
    // Declared after `staged`, so on every way out, a panic included, the file is
    // closed before its name is removed.
    let mut file = file;
    file.write_all(bytes)?;
    // Best effort. On macOS this is F_FULLFSYNC, which fails outright on SMB,
    // FAT and exFAT volumes, and a failure there says nothing about the bytes
    // just written; a label is small, and the write above already caught a full
    // disk.
    let _ = file.sync_all();
    if let Some(m) = &existing {
        // So a label kept at 0640 stays 0640: the new file was made with the
        // process's default mode.
        file.set_permissions(m.permissions())?;
    }
    drop(file);
    fs::rename(&staged.path, &target)?;
    staged.keep();
    Ok(())
}

/// The file a save replaces: `target` itself, or what it links to.
fn followed(target: &Path) -> Result<PathBuf, SaveError> {
    match fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(target).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                SaveError::DanglingLink
            } else {
                SaveError::Io(e)
            }
        }),
        _ => Ok(target.to_path_buf()),
    }
}

/// `a.relabel.part`, or `a.relabel (1).part` and on past any already taken.
///
/// Appended rather than substituted, so the staging file cannot take the name of
/// another of the operator's files. Taken means anything at the name, a dangling
/// link included. After 999 the plain name comes back, and `create_new` refuses
/// it: the save fails rather than touching a file it did not make.
fn staging_name(target: &Path) -> PathBuf {
    // An OsString, not a String: a name that is not UTF-8 is still a name.
    let beside = |suffix: &str| {
        let mut name = target.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        target.with_file_name(name)
    };
    let taken = |p: &Path| fs::symlink_metadata(p).is_ok();
    let first = beside(".part");
    if !taken(&first) {
        return first;
    }
    (1..1000)
        .map(|n| beside(&format!(" ({n}).part")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

/// A staging file this save made, removed when dropped unless the save kept it.
///
/// Every way out of [`write_atomic`] passes through the drop: an error from any
/// step, one added later included, and a panic unwinding. Only a process that
/// ends without unwinding leaves the file behind, and the next save steps round
/// it.
struct Unfinished {
    path: PathBuf,
    kept: bool,
}

impl Unfinished {
    fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if !self.kept {
            // Best effort: the error on its way back is the one that matters.
            let _ = fs::remove_file(&self.path);
        }
    }
}
