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

use ricodex_core::{Command, ItemId, Project, Tick};
use ricodex_engine::ClipState;

use crate::jobs;
use crate::state::{
    AppState, ExportMsg, JobState, MediaKind, Page, PipelineMsg, ProbeResult, RelinkResult,
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
    dragging_item: Option<ItemId>,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
    relink_tx: Sender<RelinkResult>,
    relink_rx: Receiver<RelinkResult>,
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
        let (relink_tx, relink_rx) = std::sync::mpsc::channel();
        let mut state = AppState::new();
        if let Ok(page) = std::env::var("RICODEX_PAGE") {
            if let Some(page) = Page::from_env(&page) {
                state.page = page;
            }
        }
        if let Ok(path) = std::env::var("RICODEX_EXPORT") {
            if !path.trim().is_empty() {
                state.export_path = Some(PathBuf::from(path));
            }
        }
        let cache_home = std::env::var("RICODEX_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::temp_dir().join("ricodex-cache")
            });
        let shot_path = std::env::var("RICODEX_SHOT").ok().map(PathBuf::from);
        let mut app = EditorApp {
            state,
            dragging_item: None,
            probe_tx,
            probe_rx,
            relink_tx,
            relink_rx,
            pipeline_rx: None,
            export_rx: None,
            cache_home,
            autostart_pending: std::env::var("RICODEX_AUTOSTART").is_ok(),
            started: Instant::now(),
            shot_path,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
        };
        if let Ok(list) = std::env::var("RICODEX_IMPORT") {
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

    fn save_project_as(&mut self) {
        let path = rfd::FileDialog::new()
            .set_file_name("RicoDex.rdx")
            .add_filter("RicoDex project", &["rdx", "json"])
            .save_file();
        if let Some(path) = path {
            if let Err(error) = self.state.save_project_to(path) {
                self.state.notice = Some(format!("project save failed: {error}"));
            } else {
                self.state.notice = Some("project saved".to_string());
            }
        }
    }

    fn save_project(&mut self) {
        if let Some(path) = self.state.project_path.clone() {
            if let Err(error) = self.state.save_project_to(path) {
                self.state.notice = Some(format!("project save failed: {error}"));
            } else {
                self.state.notice = Some("project saved".to_string());
            }
        } else {
            self.save_project_as();
        }
    }

    fn open_project_dialog(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("RicoDex project", &["rdx", "json"])
            .pick_file();
        let Some(path) = path else {
            return;
        };
        match Project::load(&path) {
            Ok(project) => {
                let probes = self.state.restore_project(project, path);
                for (index, media_path, kind) in probes {
                    let tx = self.relink_tx.clone();
                    std::thread::spawn(move || {
                        jobs::run_relink_probe(index, media_path, kind, tx)
                    });
                }
            }
            Err(error) => {
                self.state.notice = Some(format!("could not open project: {error}"));
            }
        }
    }

    fn project_toolbar(&mut self, ui: &mut egui::Ui) {
        let busy = self.state.pipeline_running()
            || self.state.export_job.is_running()
            || self.state.probe_job.is_running();
        let can_save = !busy && self.state.is_dirty;
        let can_save_as = !busy;
        let can_open = !busy && !self.state.is_dirty;

        let mut save = false;
        let mut save_as = false;
        let mut open = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Project").strong());
            let name = self
                .state
                .project_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_string());
            ui.label(RichText::new(name).color(SILVER_DIM));
            if self.state.is_dirty {
                ui.label(RichText::new("unsaved changes").color(WARN).small());
            }
            save = ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .clicked();
            save_as = ui
                .add_enabled(can_save_as, egui::Button::new("Save As…"))
                .clicked();
            open = ui
                .add_enabled(can_open, egui::Button::new("Open…"))
                .clicked();
        });
        if self.state.is_dirty && !busy {
            ui.label(
                RichText::new("Save current changes before opening another project.")
                    .color(SILVER_DIM)
                    .small(),
            );
        }
        if save {
            self.save_project();
        }
        if save_as {
            self.save_project_as();
        }
        if open {
            self.open_project_dialog();
        }
        if let Some(notice) = &self.state.notice {
            ui.label(
                RichText::new(notice)
                    .color(if notice.contains("failed") || notice.contains("could not") {
                        WARN
                    } else {
                        SILVER_DIM
                    })
                    .small(),
            );
        }
    }

    fn drain_channels(&mut self) {
        while let Ok(result) = self.probe_rx.try_recv() {
            self.state.handle_probe_result(result);
        }
        while let Ok(result) = self.relink_rx.try_recv() {
            self.state.handle_relink_result(result);
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

        let (undo, redo, save, save_as, open) = ctx.input(|input| {
            let command = input.modifiers.command;
            (
                self.state.page == Page::Edit
                    && command
                    && input.key_pressed(egui::Key::Z)
                    && !input.modifiers.shift,
                self.state.page == Page::Edit
                    && ((command && input.key_pressed(egui::Key::Y))
                        || (command
                            && input.modifiers.shift
                            && input.key_pressed(egui::Key::Z))),
                command && input.key_pressed(egui::Key::S) && !input.modifiers.shift,
                command && input.key_pressed(egui::Key::S) && input.modifiers.shift,
                command && input.key_pressed(egui::Key::O),
            )
        });

        if undo && !self.state.pipeline_running() {
            self.state.undo_edit();
        } else if redo && !self.state.pipeline_running() {
            self.state.redo_edit();
        }

        let project_busy = self.state.pipeline_running()
            || self.state.export_job.is_running()
            || self.state.probe_job.is_running();
        if !project_busy {
            if save {
                self.save_project();
            } else if save_as {
                self.save_project_as();
            } else if open && !self.state.is_dirty {
                self.open_project_dialog();
            }
        }

        // Automated run hook: with RICODEX_AUTOSTART set, begin the
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
            self.project_toolbar(ui);
            ui.separator();
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
        egui::Panel::bottom("ricodex-footer")
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
            let can_import = !self.state.has_manual_edits && !self.state.pipeline_running();
            if ui
                .add_enabled(can_import, egui::Button::new("Import files"))
                .clicked()
            {
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
        if self.state.has_manual_edits {
            ui.label(
                RichText::new(
                    "Media import is paused until timeline edits are undone; the current project model cannot merge new assets into an edited cut yet.",
                )
                .color(SILVER_DIM)
                .small(),
            );
        }
        ui.separator();

        let mut set_song_to: Option<usize> = None;
        let mut trim_changes: Vec<(usize, f64, f64)> = Vec::new();
        let mut relink_index: Option<usize> = None;
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
                                    let can_choose_song = !self.state.has_manual_edits
                                        || self.state.project.song_asset_id.is_none();
                                    if ui
                                        .add_enabled(can_choose_song, egui::RadioButton::new(is_song, "song"))
                                        .clicked()
                                    {
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
                            if !entry.probing && entry.probe_error.is_some() {
                                if self.state.can_relink_asset(index) {
                                    if ui.button("Locate…").clicked() {
                                        relink_index = Some(index);
                                    }
                                } else {
                                    ui.label(
                                        RichText::new("Cannot relink safely: no fingerprint was saved. Restore the source to its original path.")
                                            .color(WARN)
                                            .small(),
                                    );
                                }
                            }
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
                                        .add_enabled(
                                            self.state.can_edit_source_trims(),
                                            egui::DragValue::new(&mut intro_s)
                                                .speed(0.05)
                                                .range(0.0..=max_s)
                                                .fixed_decimals(1)
                                                .suffix(" s"),
                                        )
                                        .changed();
                                    ui.label("outro trim");
                                    let changed_out = ui
                                        .add_enabled(
                                            self.state.can_edit_source_trims(),
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
        if let Some(index) = relink_index {
            let kind = self.state.pool.get(index).map(|entry| entry.kind);
            let path = match kind {
                Some(MediaKind::Audio) => rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "m4a", "aac", "opus"])
                    .pick_file(),
                Some(MediaKind::Video) => rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v"])
                    .pick_file(),
                None => None,
            };
            if let Some(path) = path {
                if let Some(kind) = self.state.begin_relink_probe(index) {
                    let tx = self.relink_tx.clone();
                    std::thread::spawn(move || {
                        jobs::run_relink_probe(index, path, kind, tx)
                    });
                }
            }
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
            } else if self.state.has_manual_edits {
                ui.label(
                    RichText::new(
                        "undo your timeline edits before starting another analysis; current generation cannot merge edited clips yet",
                    )
                    .color(SILVER_DIM)
                    .small(),
                );
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

        self.timeline_strip(ui, 72.0, false);
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

        let pipeline_running = self.state.pipeline_running();
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").strong());
            if ui
                .add_enabled(!pipeline_running && self.state.can_undo_edit(), egui::Button::new("Undo"))
                .clicked()
            {
                self.state.undo_edit();
            }
            if ui
                .add_enabled(!pipeline_running && self.state.can_redo_edit(), egui::Button::new("Redo"))
                .clicked()
            {
                self.state.redo_edit();
            }
            if pipeline_running {
                ui.label(
                    RichText::new("Editing is paused while the cut is being generated.")
                        .color(SILVER_DIM)
                        .small(),
                );
            }
        });

        self.timeline_strip(ui, 110.0, true);
        ui.label(
            RichText::new("Drag clips left or right to reorder. Reordered clips are kept contiguous for the current sequential export path.")
                .color(SILVER_DIM)
                .small(),
        );
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
                let mut duration_seconds = clip.duration.to_seconds();
                let mut source_in_seconds = clip.source_in.to_seconds();
                let mut new_duration = None;
                let mut new_source_in = None;
                let mut split_requested = false;
                let mut delete_requested = false;
                let mut pin_requested = false;

                egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Inspector").strong());
                        ui.add_space(4.0);
                        ui.label(format!("source: {}", self.state.asset_name(clip.asset_id)));
                        ui.label(format!(
                            "state: {}",
                            match clip.state {
                                ClipState::Provisional => "provisional",
                                ClipState::Settled => "settled",
                            }
                        ));

                        ui.label(format!("timeline start: {:.2} s", clip.start.to_seconds()));
                        let source_duration = self
                            .state
                            .asset_duration(clip.asset_id)
                            .map(|duration| duration.to_seconds())
                            .unwrap_or(36000.0);
                        let (source_start, source_end) = self
                            .state
                            .asset_source_window(clip.asset_id)
                            .unwrap_or((Tick::ZERO, Tick::from_seconds(source_duration)));
                        ui.horizontal(|ui| {
                            ui.label("Duration");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut duration_seconds)
                                        .speed(0.05)
                                        .range(
                                            0.01
                                                ..=((source_end - clip.source_in).to_seconds())
                                                    .max(0.01),
                                        )
                                        .fixed_decimals(2)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                new_duration = Some(Tick::from_seconds(duration_seconds.max(0.01)));
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Source in");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut source_in_seconds)
                                        .speed(0.05)
                                        .range(
                                            source_start.to_seconds()
                                                ..=((source_end - Tick::from_seconds(duration_seconds)).to_seconds())
                                                    .max(source_start.to_seconds()),
                                        )
                                        .fixed_decimals(2)
                                        .suffix(" s"),
                                )
                                .changed()
                            {
                                new_source_in = Some(Tick::from_seconds(
                                    source_in_seconds.max(source_start.to_seconds()),
                                ));
                            }
                        });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            delete_requested = ui
                                .add_enabled(!pipeline_running, egui::Button::new("Delete clip"))
                                .clicked();
                            split_requested = ui
                                .add_enabled(
                                    !pipeline_running && clip.duration > Tick::from_seconds(0.02),
                                    egui::Button::new("Split at midpoint"),
                                )
                                .clicked();
                            let pinned = self.state.is_pinned(clip.item_id);
                            pin_requested = ui
                                .button(if pinned { "Unpin clip" } else { "Pin clip" })
                                .clicked();
                        });
                        if self.state.is_pinned(clip.item_id) {
                            ui.label(
                                RichText::new("pinned clips are protected from planner refinement")
                                    .color(SILVER_DIM)
                                    .small(),
                            );
                        }
                    });

                if pin_requested {
                    self.state.toggle_pin(clip.item_id);
                }

                let command = if delete_requested {
                    Some(Command::RippleRemoveItem { item_id: clip.item_id })
                } else if split_requested {
                    self.state.next_item_id().map(|new_item_id| Command::SplitItem {
                        item_id: clip.item_id,
                        at: clip.start + Tick::from_ticks(clip.duration.as_ticks() / 2),
                        new_item_id,
                    })
                } else if new_duration.is_some() || new_source_in.is_some() {
                    Some(Command::RippleTrimItem {
                        item_id: clip.item_id,
                        new_source_in,
                        new_duration,
                    })
                } else {
                    None
                };

                if let Some(command) = command {
                    let split_id = match &command {
                        Command::SplitItem { new_item_id, .. } => Some(*new_item_id),
                        _ => None,
                    };
                    let removes_item = matches!(
                        &command,
                        Command::RippleRemoveItem { .. } | Command::RemoveItem { .. }
                    );
                    match self.state.apply_edit(&command) {
                        Ok(()) => {
                            self.state.selected_item = split_id.or_else(|| {
                                if removes_item { None } else { Some(clip.item_id) }
                            });
                        }
                        Err(error) => self.state.notice = Some(error),
                    }
                }
            }
            None => {
                ui.label(
                    RichText::new("Select a clip on the timeline to inspect and edit it.")
                        .color(SILVER_DIM),
                );
            }
        }

        if let Some(notice) = &self.state.notice {
            ui.add_space(4.0);
            ui.label(RichText::new(notice).color(WARN));
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
                let selected_song = self.state.project.song_asset_id.and_then(|song_id| {
                    self.state
                        .project
                        .assets
                        .iter()
                        .find(|asset| asset.id == song_id)
                        .map(|asset| {
                            std::path::Path::new(&asset.path_hint)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| asset.path_hint.clone())
                        })
                });
                let soundtrack_status = match selected_song {
                    Some(name) => format!(
                        "Soundtrack: {name} (stereo AAC). Starts at timeline zero and is fitted to the video duration. Clip-source audio, fades and track gain are not included yet."
                    ),
                    None => "No song selected. This export will be video-only.".to_string(),
                };
                ui.label(
                    RichText::new(soundtrack_status)
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
    /// duration. On the Edit page, dragging a block reorders the
    /// sequential cut. The circle at a block's top right toggles its
    /// pin, and clicking the body selects the clip.
    fn timeline_strip(&mut self, ui: &mut egui::Ui, height: f32, allow_reorder: bool) {
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
        let mut completed_drag: Option<(ItemId, egui::Pos2)> = None;
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
            let sense = if allow_reorder && !self.state.pipeline_running() {
                egui::Sense::click_and_drag()
            } else {
                egui::Sense::click()
            };
            let response =
                ui.interact(block, ui.scope_id().with(("clip", clip.item_id)), sense);
            if response.drag_started() {
                self.dragging_item = Some(clip.item_id);
            }
            if response.drag_stopped() && self.dragging_item == Some(clip.item_id) {
                completed_drag = ui
                    .input(|input| input.pointer.interact_pos())
                    .map(|position| (clip.item_id, position));
                self.dragging_item = None;
            }
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
        if allow_reorder {
            if let Some(dragged_item_id) = self.dragging_item {
                if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                    if rect.contains(pointer) {
                        let before_item_id = clips
                            .iter()
                            .filter(|clip| clip.item_id != dragged_item_id)
                            .find(|clip| {
                                let midpoint = rect.left()
                                    + ((clip.start.to_seconds()
                                        + clip.duration.to_seconds() * 0.5)
                                        / total_s) as f32
                                        * rect.width();
                                pointer.x < midpoint
                            })
                            .map(|clip| clip.item_id);
                        let marker_x = before_item_id
                            .and_then(|target| clips.iter().find(|clip| clip.item_id == target))
                            .map(|target| {
                                rect.left()
                                    + (target.start.to_seconds() / total_s) as f32
                                        * rect.width()
                            })
                            .unwrap_or(rect.right() - 2.0);
                        painter.line_segment(
                            [
                                egui::pos2(marker_x, rect.top() + 2.0),
                                egui::pos2(marker_x, rect.bottom() - 2.0),
                            ],
                            Stroke::new(2.0, TEAL),
                        );
                    }
                }
            }
        }

        if let Some((dragged_item_id, pointer)) = completed_drag {
            if allow_reorder && rect.contains(pointer) {
                let before_item_id = clips
                    .iter()
                    .filter(|clip| clip.item_id != dragged_item_id)
                    .find(|clip| {
                        let midpoint = rect.left()
                            + ((clip.start.to_seconds()
                                + clip.duration.to_seconds() * 0.5)
                                / total_s) as f32
                                * rect.width();
                        pointer.x < midpoint
                    })
                    .map(|clip| clip.item_id);
                let current_before = clips
                    .iter()
                    .position(|clip| clip.item_id == dragged_item_id)
                    .and_then(|position| clips.get(position + 1))
                    .map(|clip| clip.item_id);
                if before_item_id != current_before {
                    match self.state.apply_edit(&Command::ReorderItem {
                        item_id: dragged_item_id,
                        before_item_id,
                    }) {
                        Ok(()) => self.state.selected_item = Some(dragged_item_id),
                        Err(error) => self.state.notice = Some(error),
                    }
                }
            }
        }

    }
}
