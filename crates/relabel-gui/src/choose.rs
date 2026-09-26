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
    /// An image file to import: for a new image when `again` is `None`, and
    /// otherwise into the element it names, "Image element 3".
    Artwork {
        folder: Option<PathBuf>,
        again: Option<String>,
    },
}

/// A dialog as it is shown: what [`NativeChooser`] hands the platform for an
/// [`Ask`], worked out here so a test can read it without a dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub title: String,
    /// Each filter's name and the extensions it shows, without the dot.
    pub filters: Vec<(&'static str, &'static [&'static str])>,
    pub folder: Option<PathBuf>,
    /// The name a save starts with.
    pub name: Option<String>,
    /// A save dialog, which may name a file that does not exist yet; otherwise
    /// one that picks an existing file.
    pub save: bool,
}

/// What an artwork dialog shows. Each extension in both cases: the portal matches
/// its globs case-sensitively, and a camera writes `.JPG`. The decode sniffs the
/// bytes and never trusts a name, so a filter can hide a file but never let a bad
/// one through; "All files" is there for artwork saved under the wrong extension.
/// It shows a file with no extension only through the portal: rfd 0.17.2 passes
/// `*` to the portal as it is, but writes every extension as `*.{ext}` for zenity,
/// and the `*.*` that makes there matches no such file.
const IMAGES: &[&str] = &[
    "png", "PNG", "jpg", "JPG", "jpeg", "JPEG", "gif", "GIF", "bmp", "BMP", "webp", "WEBP", "svg",
    "SVG",
];

/// What the dialog for `ask` shows.
pub fn face(ask: &Ask) -> Face {
    match ask {
        Ask::Open { folder } => Face {
            title: "Open a label".into(),
            filters: vec![("relabel label", &["relabel"])],
            folder: folder.clone(),
            name: None,
            save: false,
        },
        Ask::SaveAs { folder, name } => Face {
            title: "Save the label as".into(),
            filters: vec![("relabel label", &["relabel"])],
            folder: folder.clone(),
            name: Some(name.clone()),
            save: true,
        },
        Ask::Artwork { folder, again } => Face {
            title: match again {
                None => "Add an image to the label".into(),
                Some(heading) => format!("Import the artwork of {heading} again"),
            },
            filters: vec![("Images", IMAGES), ("All files", &["*"])],
            folder: folder.clone(),
            name: None,
            save: false,
        },
    }
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
        let Face {
            title,
            filters,
            folder,
            name,
            save,
        } = face(&ask);
        let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
        for (filter, extensions) in filters {
            dialog = dialog.add_filter(filter, extensions);
        }
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = name {
            dialog = dialog.set_file_name(name);
        }
        if save {
            dialog = dialog.set_can_create_directories(true);
        }
        // Read now, while the window is on screen: rfd takes the raw handle when it
        // is set, and a handle that cannot be read leaves the dialog with no parent
        // rather than failing (rfd file_dialog.rs:96-102).
        let dialog = dialog.set_parent(&host.window().window_handle());
        slint::spawn_local(async move {
            let chosen = if save {
                dialog.save_file().await
            } else {
                dialog.pick_file().await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_artwork_dialog_offers_images_in_both_cases_and_all_files() {
        let new = face(&Ask::Artwork {
            folder: Some(PathBuf::from("/a")),
            again: None,
        });
        assert_eq!(
            new,
            Face {
                title: "Add an image to the label".into(),
                filters: vec![
                    (
                        "Images",
                        &[
                            "png", "PNG", "jpg", "JPG", "jpeg", "JPEG", "gif", "GIF", "bmp", "BMP",
                            "webp", "WEBP", "svg", "SVG"
                        ][..]
                    ),
                    ("All files", &["*"][..]),
                ],
                folder: Some(PathBuf::from("/a")),
                name: None,
                save: false,
            }
        );
        let again = face(&Ask::Artwork {
            folder: None,
            again: Some("Image element 3".into()),
        });
        assert_eq!(again.title, "Import the artwork of Image element 3 again");
        assert_eq!((again.folder, again.save), (None, false));
    }

    #[test]
    fn the_label_dialogs_keep_their_faces() {
        let open = face(&Ask::Open {
            folder: Some(PathBuf::from("/l")),
        });
        assert_eq!(
            open,
            Face {
                title: "Open a label".into(),
                filters: vec![("relabel label", &["relabel"][..])],
                folder: Some(PathBuf::from("/l")),
                name: None,
                save: false,
            }
        );
        let save_as = face(&Ask::SaveAs {
            folder: None,
            name: "a.relabel".into(),
        });
        assert_eq!(
            save_as,
            Face {
                title: "Save the label as".into(),
                filters: vec![("relabel label", &["relabel"][..])],
                folder: None,
                name: Some("a.relabel".into()),
                save: true,
            }
        );
    }
}
