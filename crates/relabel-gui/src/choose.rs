//! Asking the operator for a file: what is asked, what comes back, and the seam
//! a test stands in for.
//!
//! No toolkit type appears in [`Chooser`], so the file state machine and every
//! test of it are written against these types alone. The answer arrives later,
//! through a callback, because a native dialog runs outside the event loop and
//! the window has to keep drawing while it is open.
//!
//! This is the only module that names the dialog library: [`NativeChooser`] is
//! the one implementation the application uses, and tests stand in their own.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What a file dialog is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A label to open, starting in `folder` when there is one.
    Open { folder: Option<PathBuf> },
    /// Where to save. The folder and the name are separate because not every
    /// dialog takes a whole path to start from.
    SaveAs {
        folder: Option<PathBuf>,
        name: String,
    },
}

/// What a file dialog answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    /// The operator closed the dialog without choosing.
    Nothing,
    /// No dialog could be shown, which is said differently from a cancel: the
    /// operator did not decline anything, and needs to hear what else works.
    NoDialog,
}

/// Called once with the answer, on the UI thread.
pub type Answer = Box<dyn FnOnce(Picked)>;

/// Something that can ask the operator for a file.
pub trait Chooser {
    /// Start asking. `Ok` means `answer` will be called exactly once, later or
    /// at once; `Err` means it never will be.
    fn ask(&self, ask: Ask, answer: Answer) -> Result<(), NoDialog>;
}

/// Why a dialog could not even be started.
#[derive(Debug, thiserror::Error)]
pub enum NoDialog {
    #[error("the file dialog could not start: {0}")]
    Spawn(String),
    #[error("the window has closed")]
    Gone,
}

/// An empty answer sooner than this after asking is taken as no dialog at all
/// rather than a cancel: with neither a portal nor zenity, the dialog library
/// answers `None` within a couple of milliseconds, exactly as it does for a
/// cancel, and no operator dismisses a dialog that fast.
pub const NO_DIALOG_WITHIN: Duration = Duration::from_millis(150);

/// The platform's own file dialog, through rfd: the xdg-desktop-portal on Linux,
/// with zenity when there is no portal; IFileDialog on Windows; a sheet on macOS.
///
/// Only rfd's async API, driven by `slint::spawn_local` on the event loop. On
/// Linux and Windows rfd runs each dialog on a thread of its own and wakes the
/// future from there, and the wake is posted back to the loop to poll
/// (i-slint-core future.rs:60-86); on macOS it builds the sheet on the first poll,
/// which is on the main thread. The blocking API would stop the window drawing,
/// and on Linux waits for the portal with no timeout at all (rfd
/// portal/mod.rs:129-144).
pub struct NativeChooser<H: slint::ComponentHandle + 'static> {
    window: slint::Weak<H>,
}

impl<H: slint::ComponentHandle + 'static> NativeChooser<H> {
    pub fn new(host: &H) -> NativeChooser<H> {
        NativeChooser {
            window: host.as_weak(),
        }
    }
}

impl<H: slint::ComponentHandle + 'static> Chooser for NativeChooser<H> {
    fn ask(&self, ask: Ask, answer: Answer) -> Result<(), NoDialog> {
        let host = self.window.upgrade().ok_or(NoDialog::Gone)?;
        let asked = Instant::now();
        let (open, folder, name) = match ask {
            Ask::Open { folder } => (true, folder, None),
            Ask::SaveAs { folder, name } => (false, folder, Some(name)),
        };
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title(if open {
                "Open a label"
            } else {
                "Save the label as"
            })
            .add_filter("relabel label", &["relabel"]);
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = name {
            dialog = dialog.set_file_name(name).set_can_create_directories(true);
        }
        // Read now, while the window is on screen: rfd takes the raw handle when it
        // is set, and a handle that cannot be read leaves the dialog with no parent
        // rather than failing (rfd file_dialog.rs:96-102).
        let dialog = dialog.set_parent(&host.window().window_handle());
        slint::spawn_local(async move {
            let chosen = if open {
                dialog.pick_file().await
            } else {
                dialog.save_file().await
            };
            answer(match chosen {
                Some(file) => Picked::Path(file.path().to_path_buf()),
                None if asked.elapsed() < NO_DIALOG_WITHIN => Picked::NoDialog,
                None => Picked::Nothing,
            });
        })
        // A future that was never spawned is dropped with `answer` inside it, so
        // an error here is the only answer there will be.
        .map(drop)
        .map_err(|e| NoDialog::Spawn(e.to_string()))
    }
}
