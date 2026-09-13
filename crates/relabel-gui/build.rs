//! Compiles the .slint files. Two details are load-bearing.
//!
//! `with_debug_info(true)` is what lets `i-slint-backend-testing`'s
//! `ElementHandle` see the element tree at all; without it every lookup
//! returns nothing and the crate warns once per call instead of failing, so
//! every canvas test passes vacuously. It is set on the test probe alone, so
//! the shipped binary does not carry the element names.
//!
//! The probe is compiled *before* the application, because each call
//! overwrites the one variable `slint::include_modules!()` reads: the last
//! component compiled is the one the library gets. The test reaches its own
//! generated file by name instead.
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    for probe in [
        "ui/testing/canvas-probe.slint",
        "ui/testing/naive-drag.slint",
        "ui/testing/ins-probe.slint",
        "ui/testing/naive-field.slint",
    ] {
        slint_build::compile_with_config(probe, config.clone().with_debug_info(true))
            .unwrap_or_else(|e| panic!("compiling {probe}: {e}"));
    }
    slint_build::compile_with_config("ui/app-window.slint", config)
        .expect("compiling ui/app-window.slint");
}
