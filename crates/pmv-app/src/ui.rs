//! The egui drawing layer: theme, the footer tab bar, and the four
//! pages. Everything here only reads `AppState` and calls its
//! transition methods; job plumbing and pipeline logic live in
//! `jobs` and `state`, so this file is the replaceable shell the
//! locked interface decision describes.
//!
//! Provisional clips on the timeline strips are drawn dimmed with
//! a diagonal hatch, so a rough cut is visible at a glance; settled
//! clips are plain. A small circle at a clip's top right is its
//! pin toggle: filled teal when pinned, hollow when not. Clicking
//! the clip body selects it, clicking the circle toggles the pin.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Stroke};

use pmv_core::Tick;
use pmv_engine::ClipState;

use crate::jobs;
use crate::state::{
    AppState, ExportMsg, JobState, MediaKind, Page, PipelineMsg, ProbeResult,
};

// ---------------------------------------------------------------- theme

/// Near black app background.
pub const INK: Color32 = Color32::from_rgb(8, 9, 11);
/// Panel surface.
pub const SURFACE: Color32 = Color32::from_rgb(18, 20, 24);
/// Raised widget surface.
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(28, 31, 37);
/// Silver primary text.
pub const SILVER: Color32 = Color32::from_rgb(216, 220, 227);
/// Dimmed silver for secondary text.
pub const SILVER_DIM: Color32 = Color32::from_rgb(150, 157, 168);
/// Teal accent for primary actions and highlights.
pub const TEAL: Color32 = Color32::from_rgb(45, 212, 191);
/// Dark teal used as the fill behind selected and primary controls.
pub const TEAL_DARK: Color32 = Color32::from_rgb(13, 84, 77);
/// Hairline borders.
pub const LINE: Color32 = Color32::from_rgb(52, 57, 66);
/// Warning and error text.
pub const WARN: Color32 = Color32::from_rgb(232, 130, 130);

/// The locked theme: black and silver with teal accents, Material
/// You language, a subtle small corner radius everywhere.
pub fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.override_text_color = Some(SILVER);
    visuals.panel_fill = INK;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::from_rgb(4, 5, 6);
    visuals.faint_bg_color = SURFACE;
    visuals.selection.bg_fill = TEAL_DARK;
    visuals.selection.stroke = Stroke::new(1.0, TEAL);
    visuals.hyperlink_color = TEAL;
    let radius = CornerRadius::same(4);
    for widgets in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
        &mut visuals.widgets.noninteractive,
    ] {
        widgets.corner_radius = radius;
        widgets.bg_stroke = Stroke::new(1.0, LINE);
    }
    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.inactive.weak_bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 42, 50);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(38, 42, 50);
    visuals.widgets.active.bg_fill = TEAL_DARK;
    visuals.widgets.active.weak_bg_fill = TEAL_DARK;
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = radius;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_global_style(style);
}

// ------------------------------------------------------------------ app

pub struct EditorApp {
    state: AppState,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
    pipeline_rx: Option<Receiver<PipelineMsg>>,
    export_rx: Option<Receiver<ExportMsg>>,
    cache_home: PathBuf,
    autostart_pending: bool,
    started: Instant,
    shot_path: Option<PathBuf>,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
}

impl EditorApp {
    pub fn from_env() -> EditorApp {
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let mut state = AppState::new();
        if let Ok(page) = std::env::var("PMV9_PAGE") {
            if let Some(page) = Page::from_env(&page) {
                state.page = page;
            }
        }
        if let Ok(path) = std::env::var("PMV9_EXPORT") {
            if !path.trim().is_empty() {
                state.export_path = Some(PathBuf::from(path));
            }
        }
        let cache_home = std::env::var("PMV9_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::temp_dir().join("ricodex-cache")
            });
        let shot_path = std::env::var("PMV9_SHOT").ok().map(PathBuf::from);
        let mut app = EditorApp {
            state,
            probe_tx,
            probe_rx,
            pipeline_rx: None,
            export_rx: None,
            cache_home,
            autostart_pending: std::env::var("PMV9_AUTOSTART").is_ok(),
            started: Instant::now(),
            shot_path,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
        };
        if let Ok(list) = std::env::var("PMV9_IMPORT") {
            for path in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                app.import_and_probe(PathBuf::from(path));
            }
        }
        app
    }

    fn import_and_probe(&mut self, path: PathBuf) {
        let before = self.state.pool.len();
        let index = self.state.import_path(path.clone());
        if self.state.pool.len() == before {
            return; // Already in the pool; its probe already ran or runs.
        }
        let kind = self.state.pool[index].kind;
        let tx = self.probe_tx.clone();
        std::thread::spawn(move || jobs::run_probe(index, path, kind, tx));
    }

    fn start_pipeline(&mut self) {
        if let Some(request) = self.state.begin_analysis(self.cache_home.clone()) {
            let (tx, rx) = std::sync::mpsc::channel();
            self.pipeline_rx = Some(rx);
            let pinned = self.state.pinned.clone();
            let cancel = self.state.pipeline_cancel.clone();
            std::thread::spawn(move || jobs::run_pipeline(request, pinned, cancel, tx));
        }
    }

    fn start_export(&mut self, path: PathBuf) {
        self.state.begin_export(path.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        self.export_rx = Some(rx);
        let project = self.state.project.clone();
        std::thread::spawn(move || jobs::run_export(project, path, tx));
    }

    fn drain_channels(&mut self) {
        while let Ok(result) = self.probe_rx.try_recv() {
            self.state.handle_probe_result(result);
        }
        if let Some(rx) = &self.pipeline_rx {
            let mut drained = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                drained.push(msg);
            }
            for msg in drained {
                self.state.handle_pipeline_msg(msg);
            }
        }
        if let Some(rx) = &self.export_rx {
            let mut drained = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                drained.push(msg);
            }
            for msg in drained {
                self.state.handle_export_msg(msg);
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_channels();

        // Automated run hook: with PMV9_AUTOSTART set, begin the
        // pipeline as soon as probing finishes, marking the first
        // audio file as the song. This is how the Cut page is
        // exercised end to end without a hand on the mouse.
        if self.autostart_pending
            && !self.state.pool.is_empty()
            && !self.state.probe_job.is_running()
        {
            self.autostart_pending = false;
            if self.state.song_index.is_none() {
                if let Some(index) = self
                    .state
                    .pool
                    .iter()
                    .position(|entry| entry.kind == MediaKind::Audio)
                {
                    self.state.set_song(index);
                }
            }
            if self.state.can_begin_analysis() {
                self.start_pipeline();
            }
        }
        if self.state.probe_job.is_running()
            || self.state.pipeline_running()
            || self.state.export_job.is_running()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // Screenshot harness for automated runs: capture the window
        // once the app has settled (giving a running pipeline a
        // bounded chance to finish first), then close. While a shot
        // is pending the app must keep repainting, otherwise the
        // idle event loop would never call logic again.
        if let Some(shot) = self.shot_path.clone() {
            let elapsed = self.started.elapsed();
            if self.shot_done.load(Ordering::SeqCst)
                || (self.shot_requested && elapsed > Duration::from_secs(20))
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                // Keep the loop awake both while waiting to settle
                // and while waiting for the capture callback, which
                // fires outside the normal repaint cycle.
                ctx.request_repaint_after(Duration::from_millis(100));
                if !self.shot_requested {
                let settled = elapsed > Duration::from_millis(2_500)
                    && (!self.state.pipeline_running()
                        || elapsed > Duration::from_secs(25));
                if settled {
                    self.shot_requested = true;
                    let done = self.shot_done.clone();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                        egui::ScreenshotCallback::new(move |image| {
                            if let Err(e) = write_color_image_png(&shot, &image) {
                                eprintln!("screenshot write failed: {e}");
                            }
                            done.store(true, Ordering::SeqCst);
                        }),
                    ));
                }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.footer(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            match self.state.page {
                Page::Media => self.media_page(ui),
                Page::Cut => self.cut_page(ui),
                Page::Edit => self.edit_page(ui),
                Page::Export => self.export_page(ui),
            }
        });
    }
}

/// Save an egui screenshot image as a PNG file.
fn write_color_image_png(path: &std::path::Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_array());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(std::io::Error::other)?;
    writer
        .write_image_data(&rgba)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)?;
    Ok(())
}

// ---------------------------------------------------------------- footer

impl EditorApp {
    /// The footer bar: page tabs live here and nowhere else, wide,
    /// with the status readings at the edges.
    fn footer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("pmv-footer")
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(300.0, 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(self.state.footer_left())
                                    .color(SILVER_DIM)
                                    .small(),
                            );
                        },
                    );
                    let right_reserve = 170.0;
                    let tab_width =
                        ((ui.available_width() - right_reserve) / 4.0).max(96.0);
                    for page in Page::ALL {
                        let selected = self.state.page == page;
                        let mut button =
                            egui::Button::new(RichText::new(page.label()).size(15.0));
                        if selected {
                            button = button.fill(TEAL_DARK);
                        }
                        if ui.add_sized([tab_width, 30.0], button).clicked() {
                            self.state.page = page;
                        }
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(self.state.footer_right())
                                    .color(SILVER_DIM)
                                    .small(),
                            );
                        },
                    );
                });
            });
    }
}

// ------------------------------------------------------------- media page

impl EditorApp {
    fn media_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Media");
            ui.add_space(12.0);
            if ui.button("Import files").clicked() {
                let dialog = rfd::FileDialog::new().add_filter(
                    "Media",
                    &[
                        "mp4", "mkv", "mov", "webm", "avi", "m4v", "wav", "mp3", "flac",
                        "ogg", "m4a", "aac", "opus",
                    ],
                );
                if let Some(files) = dialog.pick_files() {
                    for file in files {
                        self.import_and_probe(file);
                    }
                }
            }
            ui.label(
                RichText::new(
                    "Importing only probes files (Stage 0). Analysis starts below.",
                )
                .color(SILVER_DIM)
                .small(),
            );
        });
        ui.separator();

        let mut set_song_to: Option<usize> = None;
        let mut trim_changes: Vec<(usize, f64, f64)> = Vec::new();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 64.0)
            .show(ui, |ui| {
                if self.state.pool.is_empty() {
                    ui.label("No files yet. Import a song and some videos.");
                }
                for index in 0..self.state.pool.len() {
                    let entry = &self.state.pool[index];
                    egui::Frame::new()
                        .fill(SURFACE)
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(entry.name()).strong());
                                ui.label(
                                    RichText::new(match entry.kind {
                                        crate::state::MediaKind::Video => "video",
                                        crate::state::MediaKind::Audio => "audio",
                                    })
                                    .color(TEAL)
                                    .small(),
                                );
                                if entry.kind == crate::state::MediaKind::Audio {
                                    let is_song = self.state.song_index == Some(index);
                                    if ui.radio(is_song, "song").clicked() {
                                        set_song_to = Some(index);
                                    }
                                }
                                if entry.stage2.is_some() {
                                    ui.label(
                                        RichText::new("full analysis")
                                            .color(SILVER_DIM)
                                            .small(),
                                    );
                                } else if entry.stage1.is_some() {
                                    ui.label(
                                        RichText::new("quick analysis")
                                            .color(SILVER_DIM)
                                            .small(),
                                    );
                                }
                            });
                            let (detail, warn) = pool_detail(entry);
                            ui.label(
                                RichText::new(detail)
                                    .color(if warn { WARN } else { SILVER_DIM })
                                    .small(),
                            );
                            if entry.kind == crate::state::MediaKind::Video {
                                let mut intro_s = entry.intro_trim.to_seconds();
                                let mut outro_s = entry.outro_trim.to_seconds();
                                let max_s = entry
                                    .duration
                                    .map(|d| d.to_seconds())
                                    .unwrap_or(3600.0);
                                ui.horizontal(|ui| {
                                    ui.label("intro trim");
                                    let changed_in = ui
                                        .add(
                                            egui::DragValue::new(&mut intro_s)
                                                .speed(0.05)
                                                .range(0.0..=max_s)
                                                .fixed_decimals(1)
                                                .suffix(" s"),
                                        )
                                        .changed();
                                    ui.label("outro trim");
                                    let changed_out = ui
                                        .add(
                                            egui::DragValue::new(&mut outro_s)
                                                .speed(0.05)
                                                .range(0.0..=max_s)
                                                .fixed_decimals(1)
                                                .suffix(" s"),
                                        )
                                        .changed();
                                    if changed_in || changed_out {
                                        trim_changes.push((index, intro_s, outro_s));
                                    }
                                });
                            }
                        });
                    ui.add_space(6.0);
                }
            });
        if let Some(index) = set_song_to {
            self.state.set_song(index);
        }
        for (index, intro, outro) in trim_changes {
            self.state.set_trims_seconds(index, intro, outro);
        }

        ui.separator();
        ui.horizontal(|ui| {
            let can = self.state.can_begin_analysis();
            let mut button = egui::Button::new(RichText::new("Begin analysis").size(16.0));
            if can {
                button = button.fill(TEAL_DARK);
            }
            if ui.add_enabled(can, button).clicked() {
                self.start_pipeline();
            }
            if self.state.pipeline_running() {
                ui.label("analysis is running; its progress lives on the Cut page");
            } else if !can {
                ui.label(
                    RichText::new(
                        "needs a song (mark one with its song radio) and at least one video",
                    )
                    .color(SILVER_DIM)
                    .small(),
                );
            }
        });
        if let Some(notice) = &self.state.notice {
            ui.label(RichText::new(notice).color(WARN));
        }
    }
}

/// The Stage 0 detail line for one pool entry, and whether it is an
/// error line.
fn pool_detail(entry: &crate::state::PoolEntry) -> (String, bool) {
    if entry.probing {
        return ("probing…".to_string(), false);
    }
    if let Some(error) = &entry.probe_error {
        if entry.probe.is_none() && entry.duration.is_none() {
            return (error.clone(), true);
        }
    }
    if let Some(probe) = &entry.probe {
        return (
            format!(
                "{}x{} · {} · {:.1} s · {:.0} fps",
                probe.width,
                probe.height,
                probe.codec_name,
                probe.duration.to_seconds(),
                probe.frame_rate
            ),
            false,
        );
    }
    if let Some(duration) = entry.duration {
        return (format!("audio · {:.1} s", duration.to_seconds()), false);
    }
    ("no probe data".to_string(), false)
}

// --------------------------------------------------------------- cut page

impl EditorApp {
    fn cut_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Cut");
            ui.add_space(12.0);
            if self.state.pipeline_running() {
                if ui.button("Cancel").clicked() {
                    self.state.request_pipeline_cancel();
                }
            }
        });
        ui.label(RichText::new(self.state.cut_status_line()).strong());
        let fraction = self.state.pipeline_fraction();
        ui.add(
            egui::ProgressBar::new(if self.state.pipeline_job == JobState::Idle {
                0.0
            } else {
                fraction
            })
            .text(format!("{:.0}%", fraction * 100.0)),
        );
        if let Some(eta) = self.state.pipeline_eta_seconds() {
            ui.label(
                RichText::new(format!("about {:.0} s left", eta.ceil()))
                    .color(SILVER_DIM)
                    .small(),
            );
        }
        ui.separator();

        // Per asset progress.
        let indices: Vec<usize> = if self.state.pipeline_videos.is_empty() {
            self.state.video_indices()
        } else {
            self.state.pipeline_videos.clone()
        };
        for index in indices {
            let entry = &self.state.pool[index];
            let (stage_text, bar) = asset_progress(&self.state, index);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(220.0, 18.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(RichText::new(entry.name()).small());
                    },
                );
                ui.add(
                    egui::ProgressBar::new(bar)
                        .desired_width(260.0)
                        .text(stage_text),
                );
            });
        }
        ui.separator();

        self.timeline_strip(ui, 72.0);
        ui.label(
            RichText::new(
                "hatched and dimmed = provisional, plain = settled, circle at the top right of a clip toggles its pin",
            )
            .color(SILVER_DIM)
            .small(),
        );
        if let Some(notice) = &self.state.notice {
            ui.label(RichText::new(notice).color(WARN));
        }
    }
}

/// Stage label and bar fraction for one video's pipeline progress:
/// the quick pass fills the first half of the bar, the full pass
/// the second half.
fn asset_progress(state: &AppState, index: usize) -> (String, f32) {
    let entry = &state.pool[index];
    if entry.stage2.is_some() {
        return ("full pass done".to_string(), 1.0);
    }
    if entry.stage1.is_some() {
        let text = if state.stage_current == Some(index) {
            "full pass"
        } else {
            "quick pass done"
        };
        return (text.to_string(), 0.5 + 0.5 * entry.stage2_fraction);
    }
    if state.stage_current == Some(index) {
        return ("quick pass".to_string(), 0.25);
    }
    if entry.probe_error.is_some() && state.pipeline_job == JobState::Done {
        return ("failed".to_string(), 0.0);
    }
    ("waiting".to_string(), 0.0)
}

// -------------------------------------------------------------- edit page

impl EditorApp {
    fn edit_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Edit");
        ui.add_space(4.0);
        self.timeline_strip(ui, 110.0);
        ui.separator();
        let selected = self
            .state
            .selected_item
            .and_then(|id| {
                self.state
                    .timeline_clips()
                    .into_iter()
                    .find(|clip| clip.item_id == id)
            });
        match selected {
            Some(clip) => {
                egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Inspector").strong());
                        ui.add_space(4.0);
                        ui.label(format!("source: {}", self.state.asset_name(clip.asset_id)));
                        ui.label(format!("start: {:.2} s", clip.start.to_seconds()));
                        ui.label(format!("duration: {:.2} s", clip.duration.to_seconds()));
                        ui.label(format!(
                            "source in: {:.2} s",
                            clip.source_in.to_seconds()
                        ));
                        ui.label(format!(
                            "state: {}",
                            match clip.state {
                                ClipState::Provisional => "provisional",
                                ClipState::Settled => "settled",
                            }
                        ));
                        ui.add_space(4.0);
                        let pinned = self.state.is_pinned(clip.item_id);
                        if ui
                            .button(if pinned { "Unpin clip" } else { "Pin clip" })
                            .clicked()
                        {
                            self.state.toggle_pin(clip.item_id);
                        }
                        if pinned {
                            ui.label(
                                RichText::new("pinned clips are never replaced by refinement")
                                    .color(SILVER_DIM)
                                    .small(),
                            );
                        }
                    });
            }
            None => {
                ui.label(
                    RichText::new(
                        "Select a clip on the timeline. Full editing tools arrive in a later slice.",
                    )
                    .color(SILVER_DIM),
                );
            }
        }
    }
}

// ------------------------------------------------------------ export page

impl EditorApp {
    fn export_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Export");
        ui.add_space(4.0);
        let settings = &self.state.project.export_settings;
        egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(format!("size: {}x{}", settings.width, settings.height));
                ui.label(format!(
                    "frame rate: {}/{} ({:.0} fps)",
                    settings.frame_rate_num,
                    settings.frame_rate_den,
                    settings.frame_rate_num as f64
                        / settings.frame_rate_den.max(1) as f64
                ));
                ui.label(format!(
                    "codec: {} in {}",
                    settings.video_codec, settings.container
                ));
                ui.label(format!(
                    "video bitrate: {} kbps · audio bitrate: {} kbps",
                    settings.video_bitrate_kbps, settings.audio_bitrate_kbps
                ));
                ui.label(
                    RichText::new(
                        "Read only in this slice. The export is a sequential hard cut render, video only.",
                    )
                    .color(SILVER_DIM)
                    .small(),
                );
            });
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            match &self.state.export_path {
                Some(path) => {
                    ui.label(format!("output: {}", path.display()));
                }
                None => {
                    ui.label(RichText::new("no output file chosen").color(SILVER_DIM));
                }
            }
            if ui.button("Choose output…").clicked() {
                let dialog = rfd::FileDialog::new()
                    .set_file_name("export.mp4")
                    .add_filter("MP4 video", &["mp4"]);
                if let Some(path) = dialog.save_file() {
                    self.state.export_path = Some(path);
                }
            }
        });
        ui.add_space(4.0);

        let can = self.state.can_export();
        ui.horizontal(|ui| {
            let mut button = egui::Button::new(RichText::new("Export").size(16.0));
            if can {
                button = button.fill(TEAL_DARK);
            }
            if ui.add_enabled(can, button).clicked() {
                match self.state.export_path.clone() {
                    Some(path) => self.start_export(path),
                    None => {
                        let dialog = rfd::FileDialog::new()
                            .set_file_name("export.mp4")
                            .add_filter("MP4 video", &["mp4"]);
                        if let Some(path) = dialog.save_file() {
                            self.start_export(path);
                        }
                    }
                }
            }
            if !can && !self.state.export_job.is_running() {
                ui.label(
                    RichText::new("needs a timeline with at least one clip (run analysis on the Media page)")
                        .color(SILVER_DIM)
                        .small(),
                );
            }
        });

        if let JobState::Running { fraction, .. } = &self.state.export_job {
            ui.add(
                egui::ProgressBar::new(*fraction)
                    .text(format!("exporting {:.0}%", fraction * 100.0)),
            );
        }
        match &self.state.export_job {
            JobState::Done => {
                if let Some((encoder, frames, path)) = &self.state.export_result {
                    ui.label(
                        RichText::new(format!(
                            "exported {frames} frames with encoder {encoder} to {}",
                            path.display()
                        ))
                        .color(TEAL),
                    );
                }
            }
            JobState::Failed { message } => {
                ui.label(RichText::new(format!("export failed: {message}")).color(WARN));
            }
            JobState::Cancelled => {
                ui.label(RichText::new("export cancelled").color(WARN));
            }
            _ => {}
        }
    }
}

// --------------------------------------------------------- timeline strip

impl EditorApp {
    /// The timeline as a horizontal strip of clip blocks sized by
    /// duration. Provisional clips are dimmed and hatched, settled
    /// clips plain; the circle at a block's top right toggles its
    /// pin, and clicking the body selects the clip.
    fn timeline_strip(&mut self, ui: &mut egui::Ui, height: f32) {
        let clips = self.state.timeline_clips();
        let total = self.state.timeline_total();
        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            CornerRadius::same(4),
            Color32::from_rgb(13, 15, 18),
        );
        if clips.is_empty() || total <= Tick::ZERO {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "no clips yet",
                egui::FontId::proportional(13.0),
                SILVER_DIM,
            );
            return;
        }
        let total_s = total.to_seconds();
        for clip in &clips {
            let x0 = rect.left()
                + (clip.start.to_seconds() / total_s) as f32 * rect.width();
            let x1 = rect.left()
                + ((clip.start + clip.duration).to_seconds() / total_s) as f32
                    * rect.width();
            let block = egui::Rect::from_min_max(
                egui::pos2(x0 + 1.0, rect.top() + 4.0),
                egui::pos2((x1 - 1.0).max(x0 + 4.0), rect.bottom() - 4.0),
            );
            let response =
                ui.interact(block, ui.scope_id().with(("clip", clip.item_id)), egui::Sense::click());
            let pinned = self.state.is_pinned(clip.item_id);
            let selected = self.state.selected_item == Some(clip.item_id);

            let fill = match clip.state {
                ClipState::Provisional => Color32::from_rgb(52, 60, 72),
                ClipState::Settled => Color32::from_rgb(88, 98, 112),
            };
            painter.rect_filled(block, CornerRadius::same(3), fill);
            if clip.state == ClipState::Provisional {
                // Diagonal hatch, clipped to the block.
                let hatch = painter.with_clip_rect(block);
                let step = 7.0;
                let mut x = block.left() - block.height();
                while x < block.right() {
                    hatch.line_segment(
                        [
                            egui::pos2(x, block.bottom()),
                            egui::pos2(x + block.height(), block.top()),
                        ],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
                    );
                    x += step;
                }
            }
            if block.width() > 48.0 {
                painter.text(
                    egui::pos2(block.left() + 6.0, block.center().y),
                    egui::Align2::LEFT_CENTER,
                    self.state.asset_name(clip.asset_id),
                    egui::FontId::proportional(11.0),
                    SILVER,
                );
            }
            let dot_center = egui::pos2(block.right() - 9.0, block.top() + 9.0);
            if pinned {
                painter.circle_filled(dot_center, 4.5, TEAL);
            } else {
                painter.circle_stroke(dot_center, 4.5, Stroke::new(1.0, SILVER_DIM));
            }
            if selected {
                painter.rect_stroke(
                    block,
                    CornerRadius::same(3),
                    Stroke::new(2.0, TEAL),
                    egui::StrokeKind::Inside,
                );
            }
            if response.clicked() {
                let in_pin_zone = response
                    .interact_pointer_pos()
                    .map(|pos| (pos - dot_center).length() <= 10.0)
                    .unwrap_or(false);
                if in_pin_zone && block.width() > 24.0 {
                    self.state.toggle_pin(clip.item_id);
                } else {
                    self.state.selected_item = Some(clip.item_id);
                }
            }
        }
    }
}
