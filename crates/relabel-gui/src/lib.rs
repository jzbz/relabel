//! The relabel desktop application.
//!
//! Two panes: the label as dots, with a frame round every element and placement
//! by pointer or by keyboard, and beside it the selected element's fields. The
//! document lives in Rust; the `.slint` side holds flat models of rows and slots
//! and no rules at all.

pub mod a11y;
pub mod canvas;
pub mod doc;
pub mod geom;
pub mod ins;
pub mod preview;
pub mod rows;

slint::include_modules!();

impl_canvas_host!(AppWindow, ElRow);
impl_ins_host!(AppWindow, InsSlot);

/// Open a label and show it.
pub fn run_app(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let label = relabel_render::Label::from_json(&text)?;
    // Until a printer is connected, the only thing that knows which way round
    // the canvas goes is the model the file was authored for; `Left` is the
    // assumption when it says nothing, because it is the whole D11/D110/D101
    // family and the one this has been run against.
    let doc = doc::Doc::open(label, relabel_proto::PrintDirection::Left)?;
    let ui = AppWindow::new()?;
    let first = doc.orientation_note();
    let canvas = canvas::wire(&ui, doc);
    ins::wire(&ui, &canvas);
    {
        let c = canvas.borrow();
        let undrawable = c.doc().undrawable();
        ui.set_status(
            match undrawable {
                0 => first,
                1 => format!("{first}; 1 element this build cannot draw is kept as it is"),
                n => format!("{first}; {n} elements this build cannot draw are kept as they are"),
            }
            .as_str()
            .into(),
        );
    }
    // So the window opens with the keyboard in the canvas: F6 is a capture, and
    // the capture list is built from the focused item's ancestor chain, which is
    // empty when nothing has focus.
    ui.invoke_focus_canvas();
    ui.run()?;
    Ok(())
}
