//! YesVox — the egui **Standard-view** shell over the agent-native
//! [`yesvox_core::Session`] (UI ref `docs/ui/yesvox-v1-ui-reference.md`, #17).
//!
//! Every control here calls a `Session` method and nothing in this crate knows
//! DSP — the shell is a thin renderer over state that is mechanically tested in
//! `yesvox-core`. Visual fidelity is the one thing verified by eye (Dan); the
//! behavior behind each widget is verified by `cargo test`.
//!
//! Deferred (per the plan / guardrails, not yet wired here): audio playback
//! transport, the difference-spectrum monitor + reblend slider (#18 Advanced),
//! background-thread progress, and the native file-picker (a path field stands in
//! for Import for now).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use eframe::egui;
use yesvox_core::{CardId, ContentMode, LoudnessTarget, OutputFormat, Preset, Session};

const DEFAULT_VIEWPORT_SIZE: [f32; 2] = [1920.0, 1080.0];
const MIN_VIEWPORT_SIZE: [f32; 2] = [1280.0, 720.0];

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_VIEWPORT_SIZE)
            .with_min_inner_size(MIN_VIEWPORT_SIZE)
            .with_title("YesVox"),
        ..Default::default()
    };
    eframe::run_native(
        "YesVox",
        options,
        Box::new(|cc| Ok(Box::new(YesVoxApp::new(cc)))),
    )
}

/// The two rack groups, shown as the top-bar segmented tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Restore,
    Polish,
}

/// Which side of the A/B the center display shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AbView {
    Original,
    Processed,
}

struct YesVoxApp {
    session: Session,
    tab: Tab,
    ab: AbView,
    /// Stand-in for the native file picker until `rfd` is wired.
    path_input: String,
    /// Editable mirror of the output file name (committed on change).
    name_input: String,
    status: String,
}

// Palette approximating the reference mock (near-black navy + blue→cyan accents).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(86, 148, 255);
const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(58, 96, 168);
const PANEL: egui::Color32 = egui::Color32::from_rgb(16, 21, 34);
const CARD: egui::Color32 = egui::Color32::from_rgb(22, 28, 44);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(140, 152, 178);

impl YesVoxApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);
        YesVoxApp {
            session: Session::new(),
            tab: Tab::Restore,
            ab: AbView::Original,
            path_input: String::new(),
            name_input: String::new(),
            status: "Import a vocal to begin.".to_owned(),
        }
    }

    /// Open (decode + analyze) the path field, then render with current settings.
    fn import(&mut self) {
        let path = PathBuf::from(self.path_input.trim());
        if path.as_os_str().is_empty() {
            self.status = "Enter a .wav/.flac path to import.".to_owned();
            return;
        }
        match self.session.open(&path) {
            Ok(_) => {
                self.name_input = self.session.output_name().unwrap_or("vocal").to_owned();
                self.render_now();
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    /// Run the chain on the open vocal (the heavy step; synchronous for now).
    fn render_now(&mut self) {
        match self.session.render() {
            Ok(()) => {
                self.ab = AbView::Processed;
                let lufs = self.session.output_lufs().unwrap_or(f32::NAN);
                self.status = format!("Rendered · OUT {lufs:.1} LUFS");
            }
            Err(e) => self.status = format!("Render failed: {e}"),
        }
    }
}

impl eframe::App for YesVoxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top")
            .exact_size(52.0)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .exact_size(28.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.colored_label(TEXT_DIM, self.status.as_str());
                });
            });
        egui::Panel::left("left")
            .exact_size(232.0)
            .show(ui, |ui| self.left_sidebar(ui));
        egui::Panel::right("right")
            .exact_size(296.0)
            .show(ui, |ui| self.right_sidebar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.center(ui));
    }
}

impl YesVoxApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("YES")
                    .size(20.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
            ui.label(egui::RichText::new("Vox").size(20.0).strong().color(ACCENT));

            ui.add_space(24.0);
            // Restore / Polish segmented tabs.
            if ui
                .selectable_label(self.tab == Tab::Restore, "  Restore  ")
                .clicked()
            {
                self.tab = Tab::Restore;
            }
            if ui
                .selectable_label(self.tab == Tab::Polish, "  Polish  ")
                .clicked()
            {
                self.tab = Tab::Polish;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                // Preset gallery dropdown.
                let current = self.session.preset();
                let mut chosen = current;
                egui::ComboBox::from_id_salt("preset_top")
                    .selected_text(format!("Preset: {}", current.name))
                    .show_ui(ui, |ui| {
                        for p in Preset::ALL {
                            ui.selectable_value(&mut chosen, p, p.name);
                        }
                    });
                if chosen != current {
                    self.session.select_preset(chosen);
                    self.status = format!("Preset: {}", chosen.name);
                }
                // A/B compare (view toggle; playback is deferred).
                if ui.button("A / B").clicked() {
                    self.ab = match self.ab {
                        AbView::Original => AbView::Processed,
                        AbView::Processed => AbView::Original,
                    };
                }
            });
        });
    }

    fn left_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        section_header(ui, "VOCAL ORDER");
        match self.session.input() {
            Some(buf) => {
                ui.label(egui::RichText::new(self.name_input.as_str()).color(egui::Color32::WHITE));
                ui.colored_label(TEXT_DIM, fmt_duration(buf));
            }
            None => {
                ui.colored_label(TEXT_DIM, "No vocal loaded");
            }
        }

        ui.add_space(14.0);
        section_header(ui, "VOICE TYPE");
        let mode = self.session.settings().content_mode;
        ui.horizontal(|ui| {
            if ui
                .selectable_label(mode == ContentMode::Voiceover, "🎙 Voiceover")
                .clicked()
            {
                self.session.set_content_mode(ContentMode::Voiceover);
            }
            if ui
                .selectable_label(mode == ContentMode::Singing, "🎵 Singing")
                .clicked()
            {
                self.session.set_content_mode(ContentMode::Singing);
            }
        });

        ui.add_space(14.0);
        section_header(ui, "SMART ASSIST");
        ui.colored_label(
            TEXT_DIM,
            "Analyze your vocal and render a clean, natural result.",
        );
        ui.add_space(6.0);
        let can = self.session.input().is_some();
        if ui
            .add_enabled(can, egui::Button::new("  Analyze Vocal  ").fill(ACCENT_DIM))
            .clicked()
        {
            self.render_now();
        }

        ui.add_space(16.0);
        section_header(ui, "IMPORT");
        ui.colored_label(TEXT_DIM, "Path to a .wav / .flac:");
        ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(f32::INFINITY));
        ui.add_space(4.0);
        if ui
            .add(egui::Button::new("⬆  Import Vocal").fill(ACCENT))
            .clicked()
        {
            self.import();
        }
    }

    fn center(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        // Header: name + format chips.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if self.name_input.is_empty() {
                    "—".to_owned()
                } else {
                    self.name_input.to_uppercase()
                })
                .size(18.0)
                .strong()
                .color(egui::Color32::WHITE),
            );
        });
        if let Some(buf) = self.session.input() {
            ui.colored_label(
                TEXT_DIM,
                format!(
                    "{} · {} kHz · {}-bit · {} · {}",
                    self.session.output_format().label(),
                    buf.sample_rate() / 1000,
                    buf.bits_per_sample(),
                    if buf.channel_count() <= 1 {
                        "Mono"
                    } else {
                        "Stereo"
                    },
                    fmt_duration(buf),
                ),
            );
        }

        // Flow Amount master.
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.colored_label(TEXT_DIM, "FLOW AMOUNT");
            let mut flow = self.session.flow_amount();
            if ui.add(egui::Slider::new(&mut flow, 0.0..=1.0)).changed() {
                self.session.set_flow_amount(flow);
            }
            // Original / Processed + IN/OUT loudness.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(o) = self.session.output_lufs() {
                    ui.colored_label(TEXT_DIM, format!("OUT {o:.1}"));
                }
                if let Some(i) = self.session.input_lufs() {
                    ui.colored_label(TEXT_DIM, format!("IN {i:.1}"));
                }
                if ui
                    .selectable_label(self.ab == AbView::Processed, "Processed")
                    .clicked()
                {
                    self.ab = AbView::Processed;
                }
                if ui
                    .selectable_label(self.ab == AbView::Original, "Original")
                    .clicked()
                {
                    self.ab = AbView::Original;
                }
            });
        });

        self.waveform(ui);

        ui.add_space(8.0);
        // The active rack group + its intensity radial.
        let cards = match self.tab {
            Tab::Restore => &CardId::RESTORE[..],
            Tab::Polish => &CardId::POLISH[..],
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(match self.tab {
                    Tab::Restore => "RESTORE",
                    Tab::Polish => "POLISH",
                })
                .strong()
                .color(ACCENT),
            );
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match self.tab {
                    Tab::Restore => {
                        let mut t = self.session.restoration_focus();
                        ui.colored_label(TEXT_DIM, "Restoration Focus");
                        if ui.add(egui::Slider::new(&mut t, 0.0..=1.0)).changed() {
                            self.session.set_restoration_focus(t);
                        }
                    }
                    Tab::Polish => {
                        let mut t = self.session.polish_intensity();
                        ui.colored_label(TEXT_DIM, "Polish Intensity");
                        if ui.add(egui::Slider::new(&mut t, 0.0..=1.0)).changed() {
                            self.session.set_polish_intensity(t);
                        }
                    }
                },
            );
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for &card in cards {
                self.rack_card(ui, card);
            }
        });
    }

    fn waveform(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width(), 132.0),
            egui::Sense::hover(),
        );
        let rect = resp.rect;
        painter.rect_filled(rect, egui::CornerRadius::same(6), CARD);
        let buf = match self.ab {
            AbView::Original => self.session.input(),
            AbView::Processed => self.session.output().or_else(|| self.session.input()),
        };
        if let Some(buf) = buf {
            let buckets = (rect.width() as usize).max(1);
            let peaks = yesvox_core::monitor::waveform_peaks(buf, 0, buckets);
            let mid = rect.center().y;
            let h = rect.height() * 0.46;
            let color = if matches!(self.ab, AbView::Processed) {
                ACCENT
            } else {
                ACCENT_DIM
            };
            for (i, (lo, hi)) in peaks.iter().enumerate() {
                let x = rect.left() + i as f32;
                painter.line_segment(
                    [egui::pos2(x, mid - hi * h), egui::pos2(x, mid - lo * h)],
                    egui::Stroke::new(1.0, color),
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "waveform",
                egui::FontId::proportional(13.0),
                TEXT_DIM,
            );
        }
    }

    fn rack_card(&mut self, ui: &mut egui::Ui, card: CardId) {
        egui::Frame::new()
            .fill(CARD)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = self.session.card_enabled(card);
                    if ui.toggle_value(&mut on, "").changed() {
                        self.session.set_card_enabled(card, on);
                    }
                    ui.label(
                        egui::RichText::new(card.label())
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if card == CardId::Loudness {
                            ui.colored_label(
                                TEXT_DIM,
                                loudness_label(self.session.loudness_target()),
                            );
                        } else {
                            let mut amount = self.session.card_amount(card);
                            if ui
                                .add(egui::Slider::new(&mut amount, 0.0..=1.0).show_value(false))
                                .changed()
                            {
                                self.session.set_card_amount(card, amount);
                            }
                            ui.colored_label(TEXT_DIM, format!("{:.0}%", amount * 100.0));
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }

    fn right_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        section_header(ui, "PRESET");
        let current = self.session.preset();
        let mut chosen = current;
        egui::ComboBox::from_id_salt("preset_side")
            .selected_text(current.name)
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for p in Preset::ALL {
                    ui.selectable_value(&mut chosen, p, p.name);
                }
            });
        if chosen != current {
            self.session.select_preset(chosen);
        }

        ui.add_space(14.0);
        section_header(ui, "OUTPUT");
        // Format.
        let fmt = self.session.output_format();
        let mut new_fmt = fmt;
        egui::ComboBox::from_label("Format")
            .selected_text(fmt.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut new_fmt, OutputFormat::Wav, "WAV");
                ui.selectable_value(&mut new_fmt, OutputFormat::Flac, "FLAC");
            });
        if new_fmt != fmt {
            self.session.set_output_format(new_fmt);
        }
        // Sample rate is read-only (locked to input).
        if let Some(buf) = self.session.input() {
            ui.colored_label(
                TEXT_DIM,
                format!(
                    "Sample Rate   {:.1} kHz (locked)",
                    buf.sample_rate() as f32 / 1000.0
                ),
            );
        }
        // Bit depth.
        let cur_bits = self.session.settings().export.bit_depth_override;
        let mut new_bits = cur_bits;
        egui::ComboBox::from_label("Bit Depth")
            .selected_text(match cur_bits {
                None => "Source".to_owned(),
                Some(b) => format!("{b}-bit"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut new_bits, None, "Source");
                ui.selectable_value(&mut new_bits, Some(16), "16-bit");
                ui.selectable_value(&mut new_bits, Some(24), "24-bit");
                ui.selectable_value(&mut new_bits, Some(32), "32-bit");
            });
        if new_bits != cur_bits {
            self.session.set_bit_depth(new_bits);
        }
        // Loudness target.
        let cur_t = self.session.loudness_target();
        let mut new_t = cur_t;
        egui::ComboBox::from_label("Loudness")
            .selected_text(loudness_label(cur_t))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut new_t, LoudnessTarget::Podcast, "−16 LUFS (Podcast)");
                ui.selectable_value(
                    &mut new_t,
                    LoudnessTarget::Streaming,
                    "−14 LUFS (Streaming)",
                );
                ui.selectable_value(&mut new_t, LoudnessTarget::MatchInput, "Match input");
                ui.selectable_value(&mut new_t, LoudnessTarget::PeakNormalize, "Peak normalize");
            });
        if new_t != cur_t {
            self.session.set_loudness_target(new_t);
        }
        // Dither.
        let mut dither = self.session.settings().export.dither;
        if ui.checkbox(&mut dither, "Dither").changed() {
            self.session.set_dither(dither);
        }

        ui.add_space(14.0);
        section_header(ui, "FILE");
        ui.colored_label(TEXT_DIM, "Output File Name");
        if ui
            .add(egui::TextEdit::singleline(&mut self.name_input).desired_width(f32::INFINITY))
            .changed()
        {
            self.session.set_output_name(&self.name_input);
        }
        if let Some(loc) = self.session.output_location() {
            ui.colored_label(TEXT_DIM, format!("Location: {}", loc.display()));
        }

        ui.add_space(16.0);
        let can_export = self.session.output().is_some();
        if ui
            .add_enabled(
                can_export,
                egui::Button::new("⬆  EXPORT VOCAL").fill(ACCENT),
            )
            .clicked()
        {
            match self.session.export_to_target() {
                Ok(p) => self.status = format!("Exported → {}", p.display()),
                Err(e) => self.status = format!("Export failed: {e}"),
            }
        }
    }
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).strong().color(ACCENT));
    ui.add_space(2.0);
}

fn loudness_label(t: LoudnessTarget) -> String {
    match t {
        LoudnessTarget::Podcast => "−16 LUFS".to_owned(),
        LoudnessTarget::Streaming => "−14 LUFS".to_owned(),
        LoudnessTarget::MatchInput => "Match input".to_owned(),
        LoudnessTarget::PeakNormalize => "Peak norm".to_owned(),
        LoudnessTarget::Custom(v) => format!("{v:.1} LUFS"),
    }
}

fn fmt_duration(buf: &yesvox_core::AudioBuffer) -> String {
    let secs = buf.frames() as f32 / buf.sample_rate() as f32;
    let m = (secs / 60.0) as u32;
    let s = (secs % 60.0) as u32;
    format!("{m}:{s:02}")
}

fn install_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = egui::Color32::from_rgb(10, 14, 24);
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.widgets.hovered.bg_fill = ACCENT_DIM;
    visuals.hyperlink_color = ACCENT;
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_viewport_matches_reference_canvas() {
        assert_eq!(DEFAULT_VIEWPORT_SIZE, [1920.0, 1080.0]);
        assert_eq!(MIN_VIEWPORT_SIZE, [1280.0, 720.0]);
        assert_eq!(
            DEFAULT_VIEWPORT_SIZE[0] / DEFAULT_VIEWPORT_SIZE[1],
            16.0 / 9.0
        );
        assert_eq!(MIN_VIEWPORT_SIZE[0] / MIN_VIEWPORT_SIZE[1], 16.0 / 9.0);
    }

    #[test]
    fn loudness_labels_cover_every_target() {
        // A non-panicking label for every variant (incl. the editable custom box).
        for t in [
            LoudnessTarget::Podcast,
            LoudnessTarget::Streaming,
            LoudnessTarget::MatchInput,
            LoudnessTarget::PeakNormalize,
            LoudnessTarget::Custom(-18.5),
        ] {
            assert!(!loudness_label(t).is_empty());
        }
    }
}
