//! HyperDex desktop app: the desktop editor front end. A thin shell over the
//! engine: every user action is expressed through the state and
//! command layers, and nothing in the crates below this one knows
//! the interface toolkit exists.
//!
//! The four pages (Media, Cut, Edit, Export) live in a footer tab
//! bar only, per the locked interface decision. Background work
//! (Stage 0 probing, the staged analysis pipeline, export) runs on
//! worker threads from the `jobs` module and reports over channels;
//! the state transitions those messages drive live in `state` and
//! are unit tested there.
//!
//! Startup diagnostics: the app appends to `hyperdex.log` beside
//! the executable from its first moments, records what the machine
//! offers, and records any error or panic that ends the run. A GUI
//! binary has no console, so without this log a failed start is
//! invisible.
//!
//! Renderer choice: HyperDex defaults to the OpenGL (glow)
//! renderer because it has broader compatibility across Windows
//! graphics drivers. The wgpu renderer remains available as an
//! explicit opt-in through HYPERDEX_RENDERER=wgpu. We do not probe
//! wgpu automatically at startup: some graphics drivers can hang
//! or crash during adapter enumeration, before the app can recover.
//!
//! Environment hooks, used by automated runs:
//!   HYPERDEX_IMPORT   comma separated file paths to import at start
//!   HYPERDEX_PAGE     initial page: media, cut, edit or export
//!   HYPERDEX_EXPORT   default export output path
//!   HYPERDEX_AUTOSTART when set, begin analysis once probing finishes
//!   HYPERDEX_CACHE    analysis cache home (default: the system temp
//!                 directory under hyperdex-cache)
//!   HYPERDEX_SHOT     when set, capture the window to this PNG path a
//!                 few seconds after start, then close
//!   HYPERDEX_RENDERER wgpu or glow: select a renderer (default: glow)

// A Windows GUI binary must not open a console window beside it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

mod jobs;
mod state;
mod ui;

/// Path of the startup log: beside the running executable, so it
/// is always findable next to HyperDex desktop app itself.
fn log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("hyperdex.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("hyperdex.log"))
}

/// Append one line to the startup log. Opens and closes the file
/// on every call so the last line survives a crash. Never panics:
/// diagnostics must not become a failure cause of their own.
fn log_line(text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "{text}");
    }
}

/// Choose the renderer before creating the window. OpenGL is the
/// default for startup compatibility; wgpu is opt-in because adapter
/// enumeration can hang or crash inside some native graphics drivers.
fn choose_renderer() -> (eframe::Renderer, &'static str) {
    match std::env::var("HYPERDEX_RENDERER") {
        Ok(v) if v.eq_ignore_ascii_case("wgpu") => {
            log_line("renderer explicitly selected by HYPERDEX_RENDERER: wgpu");
            (eframe::Renderer::Wgpu, "wgpu")
        }
        Ok(v) if v.eq_ignore_ascii_case("glow") => {
            log_line("renderer explicitly selected by HYPERDEX_RENDERER: glow");
            (eframe::Renderer::Glow, "glow")
        }
        _ => {
            log_line("renderer default: glow (set HYPERDEX_RENDERER=wgpu to opt in)");
            (eframe::Renderer::Glow, "glow")
        }
    }
}

fn main() -> eframe::Result<()> {
    log_line(&format!(
        "HyperDex Studio {} starting, exe: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    ));
    // Record panics in the log before the default hook runs; in a
    // GUI binary the default panic output goes nowhere visible.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_line(&format!("panic: {info}"));
        default_hook(info);
    }));

    let (renderer, label) = choose_renderer();
    log_line(&format!("starting window on renderer: {label}"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("HyperDex Studio"),
        renderer,
        ..Default::default()
    };
    let result = eframe::run_native(
        "HyperDex Studio",
        options,
        Box::new(|cc| {
            log_line("window created, building app state");
            ui::apply_theme(&cc.egui_ctx);
            Ok(Box::new(ui::EditorApp::from_env()))
        }),
    );
    match &result {
        Ok(()) => log_line("exited cleanly"),
        Err(e) => log_line(&format!("failed with error: {e}")),
    }
    result
}
