use super::*;

const ACTION: egui::Color32 = egui::Color32::from_rgb(90, 213, 221);
const INK: egui::Color32 = egui::Color32::from_rgb(9, 27, 39);

impl YesVoxApp {
    pub(super) fn has_vocal_workspace(&self) -> bool {
        // The render worker temporarily owns the session, including its input.
        self.session.input().is_some() || self.render_job.is_some()
    }

    pub(super) fn import_dropped_vocal(&mut self, ui: &egui::Ui) {
        let dropped = ui.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        if let [file] = dropped.as_slice() {
            if let Some(path) = file.path.clone() {
                self.open_path(path);
            }
        } else if !dropped.is_empty() {
            self.status = "Drop one vocal at a time".to_owned();
        }
    }

    pub(super) fn leave_special_audition(&mut self) {
        if self.ab == AbView::CompareSnapshot
            || self.auditioning_reblend
            || (self.ab == AbView::Processed && self.session.output().is_none())
        {
            let side = if self.session.output().is_some() {
                AbView::Processed
            } else {
                AbView::Original
            };
            self.set_ab_view(side);
        }
    }

    pub(super) fn draw_import_screen(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
    ) {
        self.draw_wordmark(painter, c, egui::pos2(50.0, 46.0));
        let hovering_file = ui.input(|input| !input.raw.hovered_files.is_empty());
        let panel = rect(430.0, 258.0, 1060.0, 510.0);
        paint_panel(
            painter,
            c,
            panel,
            16,
            PANEL,
            if hovering_file { ACTION } else { BORDER_DIM },
        );

        let center = egui::Align2::CENTER_CENTER;
        painter.text(
            c.pos(960.0, 385.0),
            center,
            "Import a vocal",
            c.font(64.0),
            TEXT,
        );
        painter.text(
            c.pos(960.0, 457.0),
            center,
            "Drop a WAV or FLAC file here, or choose one to open.",
            c.font(24.0),
            TEXT_MUTED,
        );
        if action_button(ui, c, rect(797.0, 518.0, 326.0, 76.0), "Import Vocal", true) {
            self.import();
        }
        painter.text(
            c.pos(960.0, 635.0),
            center,
            "Use an isolated singing or voiceover track.",
            c.font(20.0),
            TEXT_MUTED,
        );
        if self.status != "Ready - import a vocal" && self.status != "Import cancelled" {
            ui.put(
                c.rect(rect(470.0, 678.0, 980.0, 60.0)),
                egui::Label::new(
                    egui::RichText::new(&self.status)
                        .size(c.v(20.0))
                        .color(AMBER),
                )
                .truncate()
                .show_tooltip_when_elided(true),
            );
        }
    }

    pub(super) fn draw_standard_workspace(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        self.draw_standard_preview(ui, painter, c);
        self.draw_standard_settings(ui, painter, c);
        self.draw_standard_output(ui, painter, c);
        paint_panel(painter, c, l.status, 0, PANEL, BORDER_DIM);
        if let (Some(player), Some(input)) = (self.player.as_ref(), self.session.input()) {
            if let Some(badge) = preview_rate_badge(player.sample_rate(), input.sample_rate()) {
                self.small_text(painter, c, 1440.0, 1066.0, &badge, TEXT_MUTED);
            }
        }
    }

    fn draw_standard_preview(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
        paint_panel(
            painter,
            c,
            rect(104.0, 122.0, 1256.0, 535.0),
            12,
            PANEL,
            BORDER_DIM,
        );
        workspace_label(
            ui,
            c,
            rect(140.0, 150.0, 850.0, 60.0),
            &self.source_name,
            34.0,
            TEXT,
        );
        if outline_action(
            ui,
            c,
            rect(1110.0, 150.0, 214.0, 50.0),
            "Import Another",
            !self.busy(),
        ) {
            self.import();
        }
        if self.session.input().is_some() {
            let metadata = format!(
                "{}  /  {}  /  {}  /  {}",
                self.source_format_label(),
                self.sample_rate_label(),
                self.channel_label(),
                self.duration_label(),
            );
            self.body_text(painter, c, 140.0, 226.0, &metadata, TEXT_MUTED);
        }

        let playing = self.player.as_ref().is_some_and(|p| p.is_playing());
        if action_button(
            ui,
            c,
            rect(140.0, 268.0, 132.0, 56.0),
            if playing { "Pause" } else { "Play" },
            self.session.input().is_some(),
        ) {
            self.auditioning_reblend = false;
            self.play_pause_audible();
        }
        let processed = self.session.output().is_some();
        for (label, side, x, enabled) in [
            (
                "Original",
                AbView::Original,
                300.0,
                self.session.input().is_some(),
            ),
            ("Processed", AbView::Processed, 462.0, processed),
        ] {
            let selected = enabled && self.ab == side;
            if button(
                ui,
                c,
                rect(x, 268.0, 156.0, 56.0),
                label,
                enabled,
                selected,
                false,
            ) {
                self.set_ab_view(side);
            }
        }
        let mut matched = self.volume_match;
        if ui
            .put(
                c.rect(rect(654.0, 268.0, 220.0, 56.0)),
                egui::Checkbox::new(
                    &mut matched,
                    egui::RichText::new("Volume Match").size(c.v(19.0)),
                ),
            )
            .changed()
        {
            self.volume_match = matched;
            self.refresh_playback_source();
        }

        if !self.busy()
            && !processed
            && self.session.input().is_some()
            && action_button(
                ui,
                c,
                rect(1034.0, 268.0, 290.0, 56.0),
                "Update Preview",
                true,
            )
        {
            self.render_now();
        }

        let wave = rect(140.0, 350.0, 1184.0, 208.0);
        paint_panel(painter, c, wave, 6, FIELD, BORDER_DIM);
        self.draw_waveform(painter, c, wave);
        if let Some(player) = &self.player {
            let hit = ui.interact(
                c.rect(wave),
                ui.make_persistent_id("standard_seek"),
                egui::Sense::click_and_drag(),
            );
            if hit.clicked() || hit.dragged() {
                if let Some(pos) = hit.interact_pointer_pos() {
                    let screen = c.rect(wave);
                    player
                        .seek_fraction(((pos.x - screen.left()) / screen.width()).clamp(0.0, 1.0));
                }
            }
        }
        if self.render_job.is_some() {
            let progress = self.processing_progress();
            self.body_text(
                painter,
                c,
                140.0,
                595.0,
                &format!("Processing vocal… {:.0}%", progress * 100.0),
                TEXT,
            );
            self.progress_bar(painter, c, rect(140.0, 618.0, 1184.0, 5.0), progress);
        } else {
            let pos = self
                .player
                .as_ref()
                .map_or(0.0, |p| p.position() as f32 / p.sample_rate().max(1) as f32);
            self.body_text(
                painter,
                c,
                140.0,
                602.0,
                &format!("{} / {}", format_duration(pos), self.duration_label()),
                TEXT_MUTED,
            );
            if let Some(lufs) = self.audible_metrics().and_then(|m| m.lufs) {
                let level = lufs_label(Some(lufs));
                self.body_text(
                    painter,
                    c,
                    1094.0,
                    602.0,
                    &format!("Preview   {level}"),
                    TEXT_MUTED,
                );
            }
        }
    }

    fn draw_standard_settings(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
        let panel = rect(104.0, 681.0, 1256.0, 285.0);
        paint_panel(painter, c, panel, 12, PANEL, BORDER_DIM);
        self.title_text(painter, c, 140.0, 727.0, "Vocal treatment");
        if outline_action(
            ui,
            c,
            rect(1102.0, 701.0, 222.0, 48.0),
            "More Controls",
            true,
        ) {
            self.show_standard_controls = true;
        }
        if self.render_job.is_some() {
            self.body_text(
                painter,
                c,
                140.0,
                800.0,
                "Settings are available when processing finishes.",
                TEXT_MUTED,
            );
            return;
        }

        self.body_text(painter, c, 140.0, 787.0, "Voice type", TEXT_MUTED);
        let mode = self.session.settings().content_mode;
        for (label, value, x) in [
            ("Voiceover", ContentMode::Voiceover, 140.0),
            ("Singing", ContentMode::Singing, 293.0),
        ] {
            if button(
                ui,
                c,
                rect(x, 810.0, 144.0, 54.0),
                label,
                !self.busy(),
                mode == value,
                false,
            ) && mode != value
            {
                self.apply_edit("content_mode", |s| s.set_content_mode(value));
                self.reset_preview();
            }
        }

        self.body_text(painter, c, 492.0, 787.0, "Preset", TEXT_MUTED);
        let mut preset = self.session.preset();
        ui.add_enabled_ui(!self.busy(), |ui| {
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(c.rect(rect(492.0, 810.0, 246.0, 54.0))),
                |ui| {
                    ui.spacing_mut().interact_size.y = c.v(54.0);
                    egui::ComboBox::from_id_salt("standard_preset")
                        .width(c.v(246.0))
                        .selected_text(egui::RichText::new(preset.name).size(c.v(19.0)))
                        .show_ui(ui, |ui| {
                            for choice in Preset::ALL {
                                ui.selectable_value(&mut preset, choice, choice.name);
                            }
                        });
                },
            );
        });
        if preset != self.session.preset() && self.apply_edit("preset", |s| s.select_preset(preset))
        {
            self.reset_preview();
        }
        if self.session.generative_reconstruction_badge() {
            self.small_text(
                painter,
                c,
                492.0,
                900.0,
                "Generative reconstruction selected",
                TEXT_MUTED,
            );
        }

        self.body_text(painter, c, 792.0, 787.0, "Effect strength", TEXT_MUTED);
        let mut flow = self.session.flow_amount();
        let response = ui
            .add_enabled_ui(!self.busy(), |ui| {
                ui.spacing_mut().slider_width = c.v(288.0);
                ui.put(
                    c.rect(rect(792.0, 812.0, 486.0, 50.0)),
                    egui::Slider::new(&mut flow, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                        .custom_parser(|s| {
                            s.trim_end_matches('%')
                                .parse::<f64>()
                                .ok()
                                .map(|v| v / 100.0)
                        })
                        .trailing_fill(true),
                )
            })
            .inner;
        if response.changed() {
            self.apply_edit("flow", |s| s.set_flow_amount(flow));
            self.reset_preview();
        }
        self.small_text(painter, c, 792.0, 899.0, "Dry", TEXT_MUTED);
        self.small_text(painter, c, 1052.0, 899.0, "Full", TEXT_MUTED);
    }

    fn draw_standard_output(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
        paint_panel(
            painter,
            c,
            rect(1386.0, 122.0, 430.0, 535.0),
            12,
            PANEL,
            BORDER_DIM,
        );
        self.title_text(painter, c, 1420.0, 172.0, "Export");
        if let Some(target) = self.session.export_target() {
            let file_name = target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&self.name_input);
            workspace_label(
                ui,
                c,
                rect(1420.0, 206.0, 362.0, 38.0),
                file_name,
                21.0,
                TEXT,
            );
            let format = format!(
                "{}  /  {}",
                self.session.output_format().label(),
                self.bit_depth_label()
            );
            self.body_text(painter, c, 1420.0, 277.0, &format, TEXT_MUTED);
        }

        self.draw_export_action(ui, c, rect(1420.0, 316.0, 362.0, 72.0));

        if outline_action(
            ui,
            c,
            rect(1420.0, 416.0, 362.0, 52.0),
            "Output Settings",
            true,
        ) {
            self.show_standard_controls = true;
        }
        workspace_label(
            ui,
            c,
            rect(1420.0, 493.0, 362.0, 100.0),
            self.standard_status(),
            17.0,
            TEXT_MUTED,
        );
    }

    fn standard_status(&self) -> &str {
        if !self.busy()
            && self.session.output().is_none()
            && (self.status.starts_with("Rendered") || self.status.starts_with("Exported"))
        {
            "Update the preview to compare and export."
        } else {
            &self.status
        }
    }

    fn reset_preview(&mut self) {
        self.isolation_check = None;
        self.set_ab_view(AbView::Original);
        self.status = "Update the preview to compare and export.".to_owned();
    }

    pub(super) fn draw_export_action(
        &mut self,
        ui: &mut egui::Ui,
        c: CanvasMap,
        bounds: egui::Rect,
    ) {
        let enabled = !self.busy() && self.session.output().is_some();
        if action_button(ui, c, bounds, "Export Vocal", enabled) {
            self.export_vocal();
        }
    }

    fn export_vocal(&mut self) {
        match self.session.prepare_export() {
            Ok(prepared) => {
                self.job_progress = 0.0;
                self.status = "Exporting…".to_owned();
                self.export_job = Some(spawn(move |ctx| {
                    ctx.progress(0.1);
                    let path = prepared.write().map_err(|e| e.to_string())?;
                    ctx.progress(1.0);
                    Ok(path)
                }));
            }
            Err(error) => self.status = format!("Export failed: {error}"),
        }
    }
}

fn workspace_label(
    ui: &mut egui::Ui,
    c: CanvasMap,
    bounds: egui::Rect,
    text: &str,
    size: f32,
    color: egui::Color32,
) {
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(c.rect(bounds))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(text).size(c.v(size)).color(color))
                    .truncate()
                    .show_tooltip_when_elided(true),
            );
        },
    );
}

fn action_button(
    ui: &mut egui::Ui,
    c: CanvasMap,
    bounds: egui::Rect,
    label: &str,
    enabled: bool,
) -> bool {
    button(ui, c, bounds, label, enabled, false, true)
}

fn outline_action(
    ui: &mut egui::Ui,
    c: CanvasMap,
    bounds: egui::Rect,
    label: &str,
    enabled: bool,
) -> bool {
    button(ui, c, bounds, label, enabled, false, false)
}

fn button(
    ui: &mut egui::Ui,
    c: CanvasMap,
    bounds: egui::Rect,
    label: &str,
    enabled: bool,
    selected: bool,
    primary: bool,
) -> bool {
    let text = egui::RichText::new(label)
        .size(c.v(if primary { 25.0 } else { 19.0 }))
        .color(if primary { INK } else { TEXT });
    let mut widget = egui::Button::new(text)
        .min_size(c.rect(bounds).size())
        .corner_radius(7)
        .selected(selected);
    if primary {
        widget = widget
            .fill(ACTION)
            .stroke(egui::Stroke::new(c.v(1.0), ACTION));
    }
    let response = ui
        .add_enabled_ui(enabled, |ui| ui.put(c.rect(bounds), widget))
        .inner;
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(c.v(3.0)),
            8,
            egui::Stroke::new(c.v(2.0), TEXT),
            egui::StrokeKind::Outside,
        );
    }
    response.clicked()
}
