use crate::config::{
    default_config_path, load_config_or_empty, save_config_with_backup, validate_config,
};
use crate::control::{ManualOutputPlacement, PreviewData};
use crate::gui::canvas::{
    CanvasState, ManualLayoutDraft, show_editable_live_monitor_canvas, show_monitor_canvas,
};
use crate::gui::daemon_client::{DaemonClient, DaemonConnectionState, fetch_daemon_status};
use crate::gui::output_editor::show_output_strip;
use crate::gui::profile_list::{build_new_profile, remove_profile};
use crate::model::{Config, Profile};
use crate::niri::ipc::{NiriClient, OutputAction, SocketNiriClient};
use crate::niri::output::NiriOutput;
use egui::{Color32, RichText, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const DAEMON_POLL_INTERVAL: Duration = Duration::from_secs(1);
const LIVE_OUTPUT_POLL_INTERVAL: Duration = Duration::from_secs(1);

const OK_COLOR: Color32 = Color32::from_rgb(72, 143, 92);
const WARNING_COLOR: Color32 = Color32::from_rgb(184, 111, 36);
const ERROR_COLOR: Color32 = Color32::from_rgb(184, 67, 67);

const APP_BG: Color32 = Color32::from_rgb(47, 47, 47);
const STRIP_BG: Color32 = Color32::from_rgb(47, 47, 47);
const BUTTON_BG: Color32 = Color32::from_rgb(43, 43, 43);
const BUTTON_STROKE: Color32 = Color32::from_rgb(65, 65, 65);

#[derive(Debug, Clone, Copy)]
struct GuiMetrics {
    scale: f32,
    compact: bool,
    toolbar_height: f32,
    bottom_height: f32,
    button_min_size: Vec2,
    profile_combo_width: f32,
}

impl GuiMetrics {
    fn from_context(ctx: &egui::Context) -> Self {
        let size = ctx.screen_rect().size();
        let short_side = size.x.min(size.y).max(480.0);
        let scale = (short_side / 900.0).clamp(0.82, 1.34);
        let compact = size.x < 780.0;
        let toolbar_height = if compact { 104.0 } else { 76.0 } * scale;
        let bottom_height = if compact { 176.0 } else { 126.0 } * scale;

        Self {
            scale,
            compact,
            toolbar_height,
            bottom_height,
            button_min_size: Vec2::new(68.0 * scale, 34.0 * scale),
            profile_combo_width: if compact { 220.0 } else { 260.0 } * scale,
        }
    }

    fn margin(self, x: f32, y: f32) -> egui::Margin {
        egui::Margin::symmetric(
            (x * self.scale).round() as i8,
            (y * self.scale).round() as i8,
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum LiveOutputsState {
    #[default]
    Checking,
    Offline {
        message: String,
    },
    Available(Vec<NiriOutput>),
}

pub struct GuiApp {
    pub config_path: Option<PathBuf>,
    pub config: Option<Config>,
    pub original_config: Option<Config>,
    pub validation_error: Option<String>,
    pub load_error: Option<String>,
    pub save_error: Option<String>,
    pub last_save_message: Option<String>,
    pub action_error: Option<String>,
    pub action_message: Option<String>,
    pub preview_data: Option<PreviewData>,
    pub selected_profile_id: Option<String>,
    pub pending_delete_profile_id: Option<String>,
    pub renaming_profile: bool,
    pub canvas: CanvasState,
    pub runtime_canvas: CanvasState,
    pub manual_layout: ManualLayoutDraft,
    pub daemon: DaemonConnectionState,
    pub live_outputs: LiveOutputsState,
    daemon_status_rx: Option<Receiver<DaemonConnectionState>>,
    live_outputs_rx: Option<Receiver<LiveOutputsState>>,
    last_daemon_poll: Option<Instant>,
    last_live_output_poll: Option<Instant>,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx, 1.0);
        Self::from_default_config_path()
    }

    pub fn from_default_config_path() -> Self {
        match default_config_path() {
            Ok(path) => Self::from_config_path(path),
            Err(error) => Self::from_startup_error(error.to_string()),
        }
    }

    pub fn from_config_path(config_path: PathBuf) -> Self {
        match load_config_or_empty(&config_path) {
            Ok(config) => Self::from_loaded_config(config_path, config),
            Err(error) => Self::from_load_error(config_path, error.to_string()),
        }
    }

    fn from_load_error(config_path: PathBuf, error: String) -> Self {
        Self {
            config_path: Some(config_path),
            config: None,
            original_config: None,
            validation_error: None,
            load_error: Some(error),
            save_error: None,
            last_save_message: None,
            action_error: None,
            action_message: None,
            preview_data: None,
            selected_profile_id: None,
            pending_delete_profile_id: None,
            renaming_profile: false,
            canvas: CanvasState::default(),
            runtime_canvas: CanvasState::default(),
            manual_layout: ManualLayoutDraft::default(),
            daemon: DaemonConnectionState::Checking,
            live_outputs: LiveOutputsState::Checking,
            daemon_status_rx: None,
            live_outputs_rx: None,
            last_daemon_poll: None,
            last_live_output_poll: None,
        }
    }

    fn from_loaded_config(config_path: PathBuf, config: Config) -> Self {
        let selected_profile_id = config.profiles.first().map(|profile| profile.id.clone());
        let validation_error = validate_config(&config)
            .err()
            .map(|error| error.to_string());

        Self {
            config_path: Some(config_path),
            original_config: Some(config.clone()),
            config: Some(config),
            validation_error,
            load_error: None,
            save_error: None,
            last_save_message: None,
            action_error: None,
            action_message: None,
            preview_data: None,
            selected_profile_id,
            pending_delete_profile_id: None,
            renaming_profile: false,
            canvas: CanvasState::default(),
            runtime_canvas: CanvasState::default(),
            manual_layout: ManualLayoutDraft::default(),
            daemon: DaemonConnectionState::Checking,
            live_outputs: LiveOutputsState::Checking,
            daemon_status_rx: None,
            live_outputs_rx: None,
            last_daemon_poll: None,
            last_live_output_poll: None,
        }
    }

    fn from_startup_error(error: String) -> Self {
        Self {
            config_path: None,
            config: None,
            original_config: None,
            validation_error: None,
            load_error: Some(error),
            save_error: None,
            last_save_message: None,
            action_error: None,
            action_message: None,
            preview_data: None,
            selected_profile_id: None,
            pending_delete_profile_id: None,
            renaming_profile: false,
            canvas: CanvasState::default(),
            runtime_canvas: CanvasState::default(),
            manual_layout: ManualLayoutDraft::default(),
            daemon: DaemonConnectionState::Checking,
            live_outputs: LiveOutputsState::Checking,
            daemon_status_rx: None,
            live_outputs_rx: None,
            last_daemon_poll: None,
            last_live_output_poll: None,
        }
    }

    fn is_dirty(&self) -> bool {
        configs_are_dirty(self.config.as_ref(), self.original_config.as_ref())
    }

    fn validate_draft(&mut self) {
        self.validation_error = self
            .config
            .as_ref()
            .and_then(|config| validate_config(config).err())
            .map(|error| error.to_string());
    }

    fn mark_draft_changed(&mut self) {
        self.save_error = None;
        self.last_save_message = None;
        self.action_error = None;
        self.action_message = None;
        self.preview_data = None;
        self.validate_draft();
    }

    fn save_draft(&mut self) {
        self.validate_draft();
        if let Some(error) = &self.validation_error {
            self.save_error = Some(format!("Fix validation errors before saving: {error}"));
            self.last_save_message = None;
            return;
        }

        if !self.is_dirty() {
            if self.save_error.is_none() {
                self.last_save_message = Some("No changes to save.".to_owned());
            }
            return;
        }

        let Some(config_path) = self.config_path.clone() else {
            self.save_error = Some("Cannot save because the config path is unresolved.".to_owned());
            self.last_save_message = None;
            return;
        };
        let Some(config) = self.config.clone() else {
            self.save_error = Some("Cannot save because no config is loaded.".to_owned());
            self.last_save_message = None;
            return;
        };

        match save_config_with_backup(&config_path, &config) {
            Ok(()) => {
                self.original_config = Some(config);
                self.save_error = None;
                self.last_save_message = Some("Saved config.".to_owned());
                self.reload_daemon_after_save();
            }
            Err(error) => {
                self.save_error = Some(error.to_string());
                self.last_save_message = None;
            }
        }
    }

    fn reload_daemon_after_save(&mut self) {
        if !matches!(self.daemon, DaemonConnectionState::Running(_)) {
            return;
        }

        match DaemonClient::from_default_socket().and_then(|client| client.reload_config()) {
            Ok(status) => {
                self.daemon = DaemonConnectionState::Running(status);
                self.last_save_message = Some("Saved config and reloaded daemon.".to_owned());
            }
            Err(error) => {
                self.save_error = Some(format!("Saved config, but daemon reload failed: {error}"));
            }
        }
    }

    fn connected_outputs(&self) -> &[NiriOutput] {
        match &self.live_outputs {
            LiveOutputsState::Available(outputs) => outputs,
            LiveOutputsState::Checking | LiveOutputsState::Offline { .. } => match &self.daemon {
                DaemonConnectionState::Running(status) => &status.outputs,
                DaemonConnectionState::Checking | DaemonConnectionState::Offline { .. } => &[],
            },
        }
    }

    fn selected_profile_id_for_action(&self) -> Option<String> {
        self.selected_profile().map(|profile| profile.id.clone())
    }

    fn preview_selected_profile(&mut self) {
        let Some(profile_id) = self.selected_profile_id_for_action() else {
            return;
        };
        match DaemonClient::from_default_socket()
            .and_then(|client| client.preview_profile(profile_id))
        {
            Ok(preview) => {
                self.preview_data = Some(preview);
                self.action_error = None;
                self.action_message = Some("Preview plan refreshed.".to_owned());
            }
            Err(error) => self.set_action_error(error.to_string()),
        }
    }

    fn apply_manual_layout(&mut self) {
        let connected_outputs = self.connected_outputs().to_vec();
        if let Some(message) = self.manual_layout.first_overlap_message(&connected_outputs) {
            self.set_action_error(format!("Cannot apply: {message}. Move monitors apart."));
            return;
        }

        let placements = self
            .manual_layout
            .normalized_changed_positions(&connected_outputs)
            .into_iter()
            .map(|(output, position)| ManualOutputPlacement { output, position })
            .collect::<Vec<_>>();

        if placements.is_empty() {
            self.action_error = None;
            self.action_message =
                Some("Layout already matches current monitor positions.".to_owned());
            return;
        }

        if matches!(self.daemon, DaemonConnectionState::Running(_)) {
            self.apply_manual_layout_via_daemon(placements);
        } else {
            self.apply_manual_layout_direct(placements);
        }
    }

    fn apply_manual_layout_via_daemon(&mut self, placements: Vec<ManualOutputPlacement>) {
        match DaemonClient::from_default_socket()
            .and_then(|client| client.apply_manual_layout(placements))
        {
            Ok(status) => {
                self.live_outputs = LiveOutputsState::Available(status.outputs.clone());
                self.manual_layout.sync_from_outputs(&status.outputs);
                self.daemon = DaemonConnectionState::Running(status);
                self.action_error = None;
                self.action_message =
                    Some("Manual layout applied. Automatic profile loading paused.".to_owned());
            }
            Err(error) => self.set_action_error(error.to_string()),
        }
    }

    fn apply_manual_layout_direct(&mut self, placements: Vec<ManualOutputPlacement>) {
        let result = (|| -> Result<Vec<NiriOutput>, String> {
            let mut client = SocketNiriClient::from_env().map_err(|error| error.to_string())?;
            for placement in placements {
                client
                    .apply_output_action(
                        &placement.output,
                        OutputAction::Position(placement.position),
                    )
                    .map_err(|error| error.to_string())?;
            }
            client.outputs().map_err(|error| error.to_string())
        })();

        match result {
            Ok(outputs) => {
                self.manual_layout.sync_from_outputs(&outputs);
                self.live_outputs = LiveOutputsState::Available(outputs);
                self.action_error = None;
                self.action_message =
                    Some("Manual layout applied directly through niri.".to_owned());
            }
            Err(error) => self.set_action_error(error),
        }
    }

    fn set_action_error(&mut self, error: String) {
        self.action_error = Some(error);
        self.action_message = None;
    }

    fn selected_profile_index(&self) -> Option<usize> {
        let config = self.config.as_ref()?;

        if let Some(selected_id) = self.selected_profile_id.as_deref()
            && let Some(index) = config
                .profiles
                .iter()
                .position(|profile| profile.id == selected_id)
        {
            return Some(index);
        }

        (!config.profiles.is_empty()).then_some(0)
    }

    fn create_profile(&mut self) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(config) = self.config.as_mut() else {
            return;
        };

        let profile = build_new_profile(&config.profiles, &connected_outputs);
        self.selected_profile_id = Some(profile.id.clone());
        config.profiles.push(profile);
        self.pending_delete_profile_id = None;
        self.renaming_profile = false;
        self.mark_draft_changed();
    }

    fn request_delete_selected_profile(&mut self) {
        self.pending_delete_profile_id = self.selected_profile().map(|profile| profile.id.clone());
    }

    fn confirm_delete_profile(&mut self, profile_id: &str) {
        let Some(config) = self.config.as_mut() else {
            return;
        };

        if remove_profile(&mut config.profiles, profile_id) {
            self.selected_profile_id = config.profiles.first().map(|profile| profile.id.clone());
            self.pending_delete_profile_id = None;
            self.renaming_profile = false;
            self.mark_draft_changed();
        }
    }

    fn refresh_daemon_status_if_needed(&mut self) {
        self.collect_daemon_status();

        if self.daemon_status_rx.is_some() {
            return;
        }

        let should_poll = self
            .last_daemon_poll
            .map(|last_poll| last_poll.elapsed() >= DAEMON_POLL_INTERVAL)
            .unwrap_or(true);
        if should_poll {
            self.queue_daemon_status_refresh();
        }
    }

    fn refresh_live_outputs_if_needed(&mut self) {
        self.collect_live_outputs();

        if self.live_outputs_rx.is_some() {
            return;
        }

        let should_poll = self
            .last_live_output_poll
            .map(|last_poll| last_poll.elapsed() >= LIVE_OUTPUT_POLL_INTERVAL)
            .unwrap_or(true);
        if should_poll {
            self.queue_live_outputs_refresh();
        }
    }

    fn collect_daemon_status(&mut self) {
        let status = self
            .daemon_status_rx
            .as_ref()
            .map(|receiver| receiver.try_recv());

        match status {
            Some(Ok(status)) => {
                self.daemon = status;
                self.daemon_status_rx = None;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.daemon = DaemonConnectionState::Offline {
                    message: "daemon status thread disconnected".to_owned(),
                };
                self.daemon_status_rx = None;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn collect_live_outputs(&mut self) {
        let status = self
            .live_outputs_rx
            .as_ref()
            .map(|receiver| receiver.try_recv());

        match status {
            Some(Ok(status)) => {
                if let LiveOutputsState::Available(outputs) = &status {
                    self.manual_layout.sync_from_outputs(outputs);
                }
                self.live_outputs = status;
                self.live_outputs_rx = None;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.live_outputs = LiveOutputsState::Offline {
                    message: "niri status thread disconnected".to_owned(),
                };
                self.live_outputs_rx = None;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    fn queue_daemon_status_refresh(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.daemon_status_rx = Some(receiver);
        self.last_daemon_poll = Some(Instant::now());

        thread::spawn(move || {
            let _ = sender.send(fetch_daemon_status());
        });
    }

    fn queue_live_outputs_refresh(&mut self) {
        let (sender, receiver) = mpsc::channel();
        self.live_outputs_rx = Some(receiver);
        self.last_live_output_poll = Some(Instant::now());

        thread::spawn(move || {
            let _ = sender.send(fetch_live_outputs());
        });
    }

    fn selected_profile(&self) -> Option<&Profile> {
        let config = self.config.as_ref()?;
        config.profiles.get(self.selected_profile_index()?)
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui, metrics: GuiMetrics) {
        ui.set_height(metrics.toolbar_height);
        if metrics.compact {
            ui.vertical_centered(|ui| {
                ui.add_space(4.0 * metrics.scale);
                ui.horizontal(|ui| self.show_profile_selector(ui, metrics));
                ui.add_space(4.0 * metrics.scale);
                ui.horizontal(|ui| self.show_toolbar_actions(ui, metrics, false));
            });
        } else {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                self.show_profile_selector(ui, metrics);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.show_toolbar_actions(ui, metrics, true);
                });
            });
        }
    }

    fn show_profile_selector(&mut self, ui: &mut egui::Ui, metrics: GuiMetrics) {
        ui.label(RichText::new("Profile:").size(15.0 * metrics.scale));

        if self.renaming_profile {
            let mut changed = false;
            let finish_rename;

            if let Some(profile_index) = self.selected_profile_index()
                && let Some(config) = self.config.as_mut()
                && let Some(profile) = config.profiles.get_mut(profile_index)
            {
                let response = ui.add_sized(
                    [metrics.profile_combo_width, metrics.button_min_size.y],
                    egui::TextEdit::singleline(&mut profile.name),
                );
                changed = response.changed();
                let pressed_done = ui.input(|input| {
                    input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Escape)
                });
                finish_rename = pressed_done || response.lost_focus();
                if !finish_rename {
                    response.request_focus();
                }
            } else {
                ui.add_sized(
                    [metrics.profile_combo_width, metrics.button_min_size.y],
                    egui::Label::new("No profile"),
                );
                finish_rename = true;
            }

            if changed {
                self.mark_draft_changed();
            }
            if finish_rename {
                self.renaming_profile = false;
            }
            return;
        }

        let selected_text = self
            .selected_profile()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "No profile".to_owned());
        let selected_index = self.selected_profile_index();
        let mut next_selection = None;

        egui::ComboBox::from_id_salt("profile_selector")
            .selected_text(selected_text)
            .width(metrics.profile_combo_width)
            .show_ui(ui, |ui| {
                if let Some(config) = &self.config {
                    for (index, profile) in config.profiles.iter().enumerate() {
                        if ui
                            .selectable_label(selected_index == Some(index), &profile.name)
                            .clicked()
                        {
                            next_selection = Some(profile.id.clone());
                        }
                    }
                }
            });

        if let Some(profile_id) = next_selection {
            self.selected_profile_id = Some(profile_id);
            self.canvas.selected_output = None;
            self.preview_data = None;
            self.action_error = None;
            self.action_message = None;
        }
    }

    fn show_toolbar_actions(
        &mut self,
        ui: &mut egui::Ui,
        metrics: GuiMetrics,
        right_to_left: bool,
    ) {
        let has_profile = self.selected_profile().is_some();
        let can_save =
            self.config.is_some() && self.config_path.is_some() && self.validation_error.is_none();

        let (new_clicked, rename_clicked, delete_clicked, save_clicked) = if right_to_left {
            let save_clicked = ui
                .add_enabled(can_save, save_toolbar_button(metrics))
                .clicked();
            let delete_clicked = ui
                .add_enabled(has_profile, toolbar_button("Delete", metrics))
                .clicked();
            let rename_clicked = ui
                .add_enabled(has_profile, toolbar_button("Rename", metrics))
                .clicked();
            let new_clicked = ui.add(toolbar_button("+ New", metrics)).clicked();
            (new_clicked, rename_clicked, delete_clicked, save_clicked)
        } else {
            let new_clicked = ui.add(toolbar_button("+ New", metrics)).clicked();
            let rename_clicked = ui
                .add_enabled(has_profile, toolbar_button("Rename", metrics))
                .clicked();
            let delete_clicked = ui
                .add_enabled(has_profile, toolbar_button("Delete", metrics))
                .clicked();
            let save_clicked = ui
                .add_enabled(can_save, save_toolbar_button(metrics))
                .clicked();
            (new_clicked, rename_clicked, delete_clicked, save_clicked)
        };

        if save_clicked {
            self.save_draft();
        }
        if delete_clicked {
            self.request_delete_selected_profile();
        }
        if rename_clicked {
            self.renaming_profile = true;
        }
        if new_clicked {
            self.create_profile();
        }
    }

    fn show_delete_confirmation(&mut self, ui: &mut egui::Ui) {
        let Some(profile_id) = self.pending_delete_profile_id.clone() else {
            return;
        };

        let profile_name = self
            .config
            .as_ref()
            .and_then(|config| {
                config
                    .profiles
                    .iter()
                    .find(|profile| profile.id == profile_id)
            })
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| profile_id.clone());

        ui.add_space(8.0);
        ui.colored_label(WARNING_COLOR, format!("Delete {profile_name}?"));
        ui.horizontal(|ui| {
            if ui.button("Confirm").clicked() {
                self.confirm_delete_profile(&profile_id);
            }
            if ui.button("Cancel").clicked() {
                self.pending_delete_profile_id = None;
            }
        });
    }

    fn show_action_feedback(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.action_error {
            ui.colored_label(ERROR_COLOR, error);
        } else if let Some(message) = &self.action_message {
            ui.colored_label(OK_COLOR, message);
        }
    }

    fn show_preview_data(&self, ui: &mut egui::Ui) {
        let Some(preview) = &self.preview_data else {
            return;
        };

        ui.add_space(8.0);
        ui.collapsing("Dry Run", |ui| {
            ui.label(format!("Profile: {}", preview.profile_id));
            ui.label(format!("Planned actions: {}", preview.actions.len()));
            for warning in &preview.warnings {
                ui.colored_label(WARNING_COLOR, warning);
            }
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.monospace(&preview.dry_run_output);
                });
        });
    }

    fn show_monitor_area(&mut self, ui: &mut egui::Ui) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(profile_index) = self.selected_profile_index() else {
            let changed = show_editable_live_monitor_canvas(
                ui,
                &connected_outputs,
                &mut self.manual_layout,
                &mut self.runtime_canvas,
            );

            if changed {
                self.action_error = None;
                self.action_message = None;
            }
            return;
        };

        let Some(config) = self.config.as_mut() else {
            ui.centered_and_justified(|ui| {
                ui.label("No config loaded.");
            });
            return;
        };

        let Some(profile) = config.profiles.get_mut(profile_index) else {
            ui.centered_and_justified(|ui| {
                ui.label("No profile selected.");
            });
            return;
        };

        let changed = show_monitor_canvas(ui, profile, &connected_outputs, &mut self.canvas);

        if changed {
            self.mark_draft_changed();
        }
    }

    fn show_bottom_panel(&mut self, ui: &mut egui::Ui, metrics: GuiMetrics) {
        ui.set_height(metrics.bottom_height);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_delete_confirmation(ui);
                self.show_status_messages(ui);

                let connected_outputs = self.connected_outputs().to_vec();
                let mut changed = false;

                if let Some(profile_index) = self.selected_profile_index() {
                    if let Some(config) = self.config.as_mut()
                        && let Some(profile) = config.profiles.get_mut(profile_index)
                    {
                        changed |= show_output_strip(
                            ui,
                            &mut self.canvas.selected_output,
                            profile,
                            &connected_outputs,
                        );
                    }
                } else {
                    self.show_live_layout_controls(ui, &connected_outputs, metrics);
                }

                if changed {
                    self.mark_draft_changed();
                }

                ui.add_space(6.0 * metrics.scale);
                ui.horizontal_wrapped(|ui| {
                    if self.selected_profile().is_some() {
                        let can_preview = matches!(self.daemon, DaemonConnectionState::Running(_))
                            && !self.is_dirty();
                        if ui
                            .add_enabled(
                                can_preview,
                                toolbar_button("Apply Preview", metrics).min_size(Vec2::new(
                                    112.0 * metrics.scale,
                                    34.0 * metrics.scale,
                                )),
                            )
                            .clicked()
                        {
                            self.preview_selected_profile();
                        }
                    }

                    self.show_action_feedback(ui);
                });

                self.show_preview_data(ui);
            });
    }

    fn show_live_layout_controls(
        &mut self,
        ui: &mut egui::Ui,
        outputs: &[NiriOutput],
        metrics: GuiMetrics,
    ) {
        let has_outputs = !outputs.is_empty();
        let has_changes = !self
            .manual_layout
            .normalized_changed_positions(outputs)
            .is_empty();
        let overlap_message = self.manual_layout.first_overlap_message(outputs);
        let has_overlap = overlap_message.is_some();

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Live Layout")
                    .strong()
                    .size(18.0 * metrics.scale),
            );
            if let Some(output) = self
                .runtime_canvas
                .selected_output
                .and_then(|index| outputs.get(index))
            {
                ui.label(&output.connector);
                ui.label(RichText::new(&output.description).weak());
                if let Some(logical) = &output.logical {
                    ui.label(format!("x {}", logical.x));
                    ui.label(format!("y {}", logical.y));
                    ui.label(format!("scale {}", logical.scale));
                }
            } else if has_outputs {
                ui.label("Click a monitor to select it, then drag to arrange.");
            } else {
                ui.label("No connected outputs available.");
            }
        });

        ui.add_space(6.0 * metrics.scale);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    has_outputs && has_changes && !has_overlap,
                    toolbar_button("Apply Changes", metrics)
                        .min_size(Vec2::new(120.0 * metrics.scale, 34.0 * metrics.scale)),
                )
                .clicked()
            {
                self.apply_manual_layout();
            }

            if ui
                .add_enabled(has_changes, toolbar_button("Reset Draft", metrics))
                .clicked()
            {
                self.manual_layout.reset();
                self.action_error = None;
                self.action_message = Some("Manual layout draft reset.".to_owned());
            }

            if has_overlap {
                ui.colored_label(ERROR_COLOR, "layout overlaps");
            } else if has_changes {
                ui.colored_label(WARNING_COLOR, "layout changed");
            } else if has_outputs {
                ui.colored_label(OK_COLOR, "layout current");
            }
        });

        if let Some(message) = overlap_message {
            ui.colored_label(
                ERROR_COLOR,
                format!("Cannot apply: {message}. Move monitors apart."),
            );
        }
    }

    fn show_status_messages(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.load_error {
            ui.colored_label(ERROR_COLOR, format!("Config load failed: {error}"));
        }
        if let Some(error) = &self.validation_error {
            ui.colored_label(ERROR_COLOR, format!("Validation failed: {error}"));
        }
        if let Some(error) = &self.save_error {
            ui.colored_label(ERROR_COLOR, error);
        } else if let Some(message) = &self.last_save_message {
            ui.colored_label(OK_COLOR, message);
        } else if self.is_dirty() {
            ui.colored_label(WARNING_COLOR, "Unsaved changes");
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let metrics = GuiMetrics::from_context(ctx);
        configure_style(ctx, metrics.scale);

        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S)) {
            self.save_draft();
        }

        if ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.refresh_daemon_status_if_needed();
        self.refresh_live_outputs_if_needed();

        egui::TopBottomPanel::top("status_bar")
            .exact_height(metrics.toolbar_height)
            .frame(
                egui::Frame::new()
                    .fill(STRIP_BG)
                    .inner_margin(metrics.margin(12.0, 8.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(51, 54, 77))),
            )
            .show(ctx, |ui| self.show_status_bar(ui, metrics));

        egui::TopBottomPanel::bottom("controls_and_info")
            .exact_height(metrics.bottom_height)
            .frame(
                egui::Frame::new()
                    .fill(STRIP_BG)
                    .inner_margin(metrics.margin(12.0, 8.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(51, 54, 77))),
            )
            .show(ctx, |ui| self.show_bottom_panel(ui, metrics));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(APP_BG).inner_margin(0))
            .show(ctx, |ui| {
                self.show_monitor_area(ui);
            });

        if self.daemon_status_rx.is_some() || self.live_outputs_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(DAEMON_POLL_INTERVAL);
        }
    }
}

fn fetch_live_outputs() -> LiveOutputsState {
    match SocketNiriClient::from_env().and_then(|mut client| client.outputs()) {
        Ok(outputs) => LiveOutputsState::Available(outputs),
        Err(error) => LiveOutputsState::Offline {
            message: error.to_string(),
        },
    }
}

fn configure_style(ctx: &egui::Context, scale: f32) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = APP_BG;
    visuals.panel_fill = APP_BG;
    visuals.faint_bg_color = Color32::from_rgb(35, 35, 35);
    visuals.extreme_bg_color = Color32::from_rgb(20, 20, 20);
    visuals.widgets.noninteractive.bg_fill = APP_BG;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(43, 43, 43);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 58, 58);
    visuals.widgets.active.bg_fill = Color32::from_rgb(62, 62, 68);
    visuals.selection.bg_fill = Color32::from_rgb(92, 94, 128);

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(10.0 * scale, 8.0 * scale);
    style.spacing.button_padding = Vec2::new(12.0 * scale, 6.0 * scale);
    style.spacing.interact_size = Vec2::new(28.0 * scale, 32.0 * scale);
    style.spacing.combo_width = 180.0 * scale;
    style.spacing.text_edit_width = 180.0 * scale;
    style.spacing.slider_width = 120.0 * scale;
    style.spacing.icon_width = 16.0 * scale;
    style.spacing.icon_width_inner = 10.0 * scale;
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::proportional(22.0 * scale),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::proportional(15.0 * scale),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::proportional(15.0 * scale),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::proportional(12.0 * scale),
    );
    ctx.set_style(style);
}

fn toolbar_button(label: &'static str, metrics: GuiMetrics) -> egui::Button<'static> {
    egui::Button::new(label)
        .min_size(metrics.button_min_size)
        .fill(BUTTON_BG)
        .stroke(Stroke::new(1.0_f32, BUTTON_STROKE))
}

fn save_toolbar_button(metrics: GuiMetrics) -> egui::Button<'static> {
    egui::Button::new("Save")
        .min_size(metrics.button_min_size)
        .fill(BUTTON_BG)
        .stroke(Stroke::new(1.0_f32, BUTTON_STROKE))
}

fn configs_are_dirty(config: Option<&Config>, original_config: Option<&Config>) -> bool {
    match (config, original_config) {
        (Some(config), Some(original_config)) => config != original_config,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::empty_config;

    #[test]
    fn unchanged_config_is_not_dirty() {
        let config = empty_config();

        assert!(!configs_are_dirty(Some(&config), Some(&config)));
    }

    #[test]
    fn changed_config_is_dirty() {
        let original_config = empty_config();
        let mut edited_config = empty_config();
        edited_config.profiles.push(Profile {
            id: "home".to_owned(),
            name: "Home".to_owned(),
            priority: 0,
            enabled: true,
            condition: Default::default(),
            outputs: Vec::new(),
        });

        assert!(configs_are_dirty(
            Some(&edited_config),
            Some(&original_config)
        ));
    }

    #[test]
    fn missing_config_is_not_dirty() {
        assert!(!configs_are_dirty(None, None));
    }

    #[test]
    fn missing_config_path_starts_empty_draft() {
        let path = temp_config_path("missing-config");

        let app = GuiApp::from_config_path(path.clone());

        assert!(app.load_error.is_none());
        assert_eq!(
            app.config.as_ref().map(|config| config.profiles.len()),
            Some(0)
        );
        assert_eq!(app.config_path.as_ref(), Some(&path));

        std::fs::remove_dir_all(path.parent().expect("temp path should have parent"))
            .expect("temp directory should be removed");
    }

    #[test]
    fn empty_config_path_starts_empty_draft() {
        let path = temp_config_path("empty-config");
        std::fs::write(&path, "\n  \n").expect("empty config file should be written");

        let app = GuiApp::from_config_path(path.clone());

        assert!(app.load_error.is_none());
        assert_eq!(
            app.config.as_ref().map(|config| config.profiles.len()),
            Some(0)
        );
        assert_eq!(app.config_path.as_ref(), Some(&path));

        std::fs::remove_dir_all(path.parent().expect("temp path should have parent"))
            .expect("temp directory should be removed");
    }

    fn temp_config_path(test_name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "niri-monitors-gui-{test_name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("temp directory should be created");
        directory.join("config.toml")
    }
}
