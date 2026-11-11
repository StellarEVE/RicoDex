//! pmv-app: the desktop editor front end. A thin shell over the
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
//! Startup diagnostics: the app appends to `pmv-genie.log` beside
//! the executable from its first moments, records what the machine
//! offers, and records any error or panic that ends the run. A GUI
//! binary has no console, so without this log a failed start is
//! invisible.
//!
//! Renderer choice: winit allows only one event loop per process,
//! so the renderer must be chosen before the window is created,
//! never retried after a failed start. The app therefore probes
//! the wgpu stack first (the same wgpu version the interface
//! uses), asking for any real adapter. If an adapter answers, the
//! app runs on wgpu. If none does, as happens on older laptops
//! and dual GPU machines whose Vulkan or DirectX 12 path is
//! unhealthy, the app runs on the OpenGL (glow) renderer instead,
//! whose drivers are far older and more forgiving.
//!
//! Environment hooks, used by automated runs:
//!   PMV9_IMPORT   comma separated file paths to import at start
//!   PMV9_PAGE     initial page: media, cut, edit or export
//!   PMV9_EXPORT   default export output path
//!   PMV9_AUTOSTART when set, begin analysis once probing finishes
//!   PMV9_CACHE    analysis cache home (default: the system temp
//!                 directory under pmv-genie-alpha9-cache)
//!   PMV9_SHOT     when set, capture the window to this PNG path a
//!                 few seconds after start, then close
//!   PMV9_RENDERER wgpu or glow: force a renderer, skipping the
//!                 probe based choice

// A Windows GUI binary must not open a console window beside it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

mod jobs;
mod state;
mod ui;

/// Path of the startup log: beside the running executable, so it
/// is always findable next to pmv-app itself.
fn log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("pmv-genie.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("pmv-genie.log"))
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

/// Ask the wgpu stack whether this machine has any usable
/// adapter, before any window exists. Returns a short description
/// of the adapter when one answers. Panics inside the graphics
/// stack are caught and treated as no adapter.
fn probe_wgpu() -> Option<String> {
    let outcome = std::panic::catch_unwind(|| {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(descriptor);
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            },
        ))
        .ok()?;
        let info = adapter.get_info();
        Some(format!(
            "{} via {:?} ({:?})",
            info.name, info.backend, info.device_type
        ))
    });
    match outcome {
        Ok(found) => found,
        Err(_) => {
            log_line("wgpu probe: panicked inside the graphics stack");
            None
        }
    }
}

/// Choose the renderer for this run: an explicit PMV9_RENDERER
/// wins, otherwise wgpu when the probe found an adapter, glow
/// when it did not.
fn choose_renderer() -> (eframe::Renderer, &'static str) {
    match std::env::var("PMV9_RENDERER") {
        Ok(v) if v.eq_ignore_ascii_case("glow") => {
            log_line("renderer forced by PMV9_RENDERER: glow");
            return (eframe::Renderer::Glow, "glow");
        }
        Ok(v) if v.eq_ignore_ascii_case("wgpu") => {
            log_line("renderer forced by PMV9_RENDERER: wgpu");
            return (eframe::Renderer::Wgpu, "wgpu");
        }
        _ => {}
    }
    match probe_wgpu() {
        Some(adapter) => {
            log_line(&format!("wgpu probe: adapter found, {adapter}"));
            (eframe::Renderer::Wgpu, "wgpu")
        }
        None => {
            log_line("wgpu probe: no adapter, falling back to glow");
            (eframe::Renderer::Glow, "glow")
        }
    }
}

fn main() -> eframe::Result<()> {
    log_line(&format!(
        "PMV-Genie {} starting, exe: {}",
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
            .with_title("PMV-Genie"),
        renderer,
        ..Default::default()
    };
    let result = eframe::run_native(
        "PMV-Genie",
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
