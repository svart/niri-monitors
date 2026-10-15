use crate::config::{
    default_config_path, load_config_or_empty, save_config_with_backup, validate_config,
};
use crate::control::PreviewData;
use crate::gui::canvas::{
    CanvasState, ManualLayoutDraft, nudge_profile_output, show_editable_live_monitor_canvas,
    show_monitor_canvas,
};
use crate::gui::daemon_client::{DaemonClient, DaemonConnectionState, fetch_daemon_status};
use crate::gui::header::{HeaderAction, daemon_summary, show_header};
use crate::gui::inspector::{
    LiveLayoutAction, LiveLayoutStatus, show_live_layout, show_output_settings,
};
use crate::gui::poller::Poller;
use crate::gui::profile_list::{build_new_profile, remove_profile};
use crate::gui::sidebar::{self, SidebarAction};
use crate::gui::theme;
use crate::gui::toast::Toasts;
use crate::matching::matcher_matches_output;
use crate::model::{Config, ManualOutputPlacement, Profile};
use crate::niri::apply::{build_manual_layout_plan, execute_apply_plan};
use crate::niri::ipc::{NiriClient, SocketNiriClient};
use crate::niri::output::NiriOutput;
use crate::placement::{derive_output_rects, first_overlap_message};
use eframe::egui::{self, Margin, RichText};
use std::path::PathBuf;
use std::time::Duration;

const DAEMON_POLL_INTERVAL: Duration = Duration::from_secs(1);
const LIVE_OUTPUT_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Below this window width, the inspector moves under the canvas.
const COMPACT_WIDTH: f32 = 1000.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub enum LiveOutputsState {
    #[default]
    Checking,
    Offline {
        message: String,
    },
    Available(Vec<NiriOutput>),
}

#[derive(Default)]
pub struct GuiApp {
    config_path: Option<PathBuf>,
    config: Option<Config>,
    original_config: Option<Config>,
    validation_error: Option<String>,
    load_error: Option<String>,
    preview_data: Option<PreviewData>,
    selected_index: Option<usize>,
    pending_delete: Option<usize>,
    renaming_profile: bool,
    canvas: CanvasState,
    runtime_canvas: CanvasState,
    manual_layout: ManualLayoutDraft,
    daemon: DaemonConnectionState,
    live_outputs: LiveOutputsState,
    daemon_poller: Poller<DaemonConnectionState>,
    live_outputs_poller: Poller<LiveOutputsState>,
    toasts: Toasts,
}

impl GuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx, theme::load(cc.storage));
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
            load_error: Some(error),
            ..Self::default()
        }
    }

    fn from_loaded_config(config_path: PathBuf, config: Config) -> Self {
        Self {
            config_path: Some(config_path),
            validation_error: validate_config(&config)
                .err()
                .map(|error| error.to_string()),
            original_config: Some(config.clone()),
            config: Some(config),
            ..Self::default()
        }
    }

    fn from_startup_error(error: String) -> Self {
        Self {
            load_error: Some(error),
            ..Self::default()
        }
    }

    fn is_dirty(&self) -> bool {
        configs_are_dirty(self.config.as_ref(), self.original_config.as_ref())
    }

    fn can_save(&self) -> bool {
        self.config.is_some()
            && self.config_path.is_some()
            && self.validation_error.is_none()
            && self.is_dirty()
    }

    fn validate_draft(&mut self) {
        self.validation_error = self
            .config
            .as_ref()
            .and_then(|config| validate_config(config).err())
            .map(|error| error.to_string());
    }

    fn mark_draft_changed(&mut self) {
        self.preview_data = None;
        self.validate_draft();
    }

    fn save_draft(&mut self) {
        self.validate_draft();
        if let Some(error) = &self.validation_error {
            self.toasts.error(format!("Cannot save: {error}"));
            return;
        }
        if !self.is_dirty() {
            return;
        }
        let (Some(config_path), Some(config)) = (self.config_path.clone(), self.config.clone())
        else {
            self.toasts
                .error("Cannot save: the config file location is unknown.");
            return;
        };

        match save_config_with_backup(&config_path, &config) {
            Ok(()) => {
                self.original_config = Some(config);
                self.reload_daemon_after_save();
            }
            Err(error) => self.toasts.error(format!("Saving failed: {error}")),
        }
    }

    fn reload_daemon_after_save(&mut self) {
        if !self.daemon_running() {
            self.toasts.success("Saved.");
            return;
        }

        match DaemonClient::from_default_socket().and_then(|client| client.reload_config()) {
            Ok(status) => {
                self.daemon = DaemonConnectionState::Running(status);
                self.toasts
                    .success("Saved. The daemon reloaded the config.");
            }
            Err(error) => self
                .toasts
                .error(format!("Saved, but the daemon did not reload: {error}")),
        }
    }

    fn daemon_running(&self) -> bool {
        matches!(self.daemon, DaemonConnectionState::Running(_))
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

    fn niri_error(&self) -> Option<&str> {
        match &self.live_outputs {
            LiveOutputsState::Offline { message } => Some(message),
            LiveOutputsState::Checking | LiveOutputsState::Available(_) => None,
        }
    }

    fn preview_selected_profile(&mut self) {
        let Some(name) = self.selected_profile().map(|profile| profile.name.clone()) else {
            return;
        };
        match DaemonClient::from_default_socket().and_then(|client| client.preview_profile(name)) {
            Ok(preview) => self.preview_data = Some(preview),
            Err(error) => self.toasts.error(format!("Preview failed: {error}")),
        }
    }

    fn apply_manual_layout(&mut self) {
        let connected_outputs = self.connected_outputs().to_vec();
        if let Some(message) = self.manual_layout.first_overlap_message(&connected_outputs) {
            self.toasts
                .error(format!("Cannot apply: {message}. Move the monitors apart."));
            return;
        }

        let placements = self.manual_layout.changed_placements(&connected_outputs);
        if placements.is_empty() {
            self.toasts
                .success("The layout already matches the monitors.");
            return;
        }

        if self.daemon_running() {
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
                self.show_applied_layout(&status.outputs);
                self.daemon = DaemonConnectionState::Running(status);
                self.toasts
                    .success("Layout applied. Automatic profiles are paused.");
            }
            Err(error) => self.toasts.error(format!("Applying failed: {error}")),
        }
    }

    fn apply_manual_layout_direct(&mut self, placements: Vec<ManualOutputPlacement>) {
        let result = (|| -> Result<Vec<NiriOutput>, String> {
            let mut client = SocketNiriClient::from_env().map_err(|error| error.to_string())?;
            let outputs = client.outputs().map_err(|error| error.to_string())?;
            let plan = build_manual_layout_plan(&outputs, &placements)
                .map_err(|error| error.to_string())?;
            execute_apply_plan(&mut client, &plan).map_err(|error| error.to_string())?;
            client.outputs().map_err(|error| error.to_string())
        })();

        match result {
            Ok(outputs) => {
                self.show_applied_layout(&outputs);
                self.live_outputs = LiveOutputsState::Available(outputs);
                self.toasts.success("Layout applied through niri.");
            }
            Err(error) => self.toasts.error(format!("Applying failed: {error}")),
        }
    }

    /// Replaces the draft with what niri reports after an apply. niri may pick a new
    /// automatic scale for a new mode, so the result can differ from the draft.
    fn show_applied_layout(&mut self, outputs: &[NiriOutput]) {
        self.manual_layout.sync_from_outputs(outputs);
        self.manual_layout.reset();
    }

    /// The selected profile, or the first one when nothing valid is selected.
    fn selected_profile_index(&self) -> Option<usize> {
        let count = self.config.as_ref()?.profiles.len();
        self.selected_index
            .filter(|index| *index < count)
            .or((count > 0).then_some(0))
    }

    fn selected_profile(&self) -> Option<&Profile> {
        let config = self.config.as_ref()?;
        config.profiles.get(self.selected_profile_index()?)
    }

    fn select_profile(&mut self, index: usize) {
        if self
            .config
            .as_ref()
            .is_none_or(|config| index >= config.profiles.len())
        {
            return;
        }
        if self.selected_profile_index() != Some(index) {
            self.selected_index = Some(index);
            self.canvas.selected_output = None;
            self.preview_data = None;
        }
        self.renaming_profile = false;
    }

    fn create_profile(&mut self) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(config) = self.config.as_mut() else {
            return;
        };

        let profile = build_new_profile(&config.profiles, &connected_outputs);
        config.profiles.push(profile);
        self.selected_index = Some(config.profiles.len() - 1);
        self.canvas.selected_output = None;
        self.renaming_profile = true;
        self.mark_draft_changed();
    }

    fn confirm_delete_profile(&mut self, index: usize) {
        self.pending_delete = None;
        let Some(config) = self.config.as_mut() else {
            return;
        };

        if remove_profile(&mut config.profiles, index) {
            self.selected_index = None;
            self.canvas.selected_output = None;
            self.renaming_profile = false;
            self.mark_draft_changed();
        }
    }

    fn refresh_daemon_status(&mut self) {
        match self
            .daemon_poller
            .poll(DAEMON_POLL_INTERVAL, fetch_daemon_status)
        {
            Some(Ok(status)) => self.daemon = status,
            Some(Err(_)) => {
                self.daemon = DaemonConnectionState::Offline {
                    message: "daemon status thread disconnected".to_owned(),
                };
            }
            None => {}
        }
    }

    fn refresh_live_outputs(&mut self) {
        match self
            .live_outputs_poller
            .poll(LIVE_OUTPUT_POLL_INTERVAL, fetch_live_outputs)
        {
            Some(Ok(status)) => {
                if let LiveOutputsState::Available(outputs) = &status {
                    self.manual_layout.sync_from_outputs(outputs);
                }
                self.live_outputs = status;
            }
            Some(Err(_)) => {
                self.live_outputs = LiveOutputsState::Offline {
                    message: "niri status thread disconnected".to_owned(),
                };
            }
            None => {}
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S)) {
            self.save_draft();
        }
        if ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.wants_keyboard_input() || self.pending_delete.is_some() {
            return;
        }

        let (delta, delete) = ctx.input(|input| {
            let step = if input.modifiers.shift { 10 } else { 1 };
            let mut delta = (0, 0);
            for (key, dx, dy) in [
                (egui::Key::ArrowLeft, -step, 0),
                (egui::Key::ArrowRight, step, 0),
                (egui::Key::ArrowUp, 0, -step),
                (egui::Key::ArrowDown, 0, step),
            ] {
                if input.key_pressed(key) {
                    delta = (delta.0 + dx, delta.1 + dy);
                }
            }
            (delta, input.key_pressed(egui::Key::Delete))
        });
        if delta != (0, 0) {
            self.nudge_selected_output(delta);
        }
        if delete {
            self.pending_delete = self.selected_profile_index();
        }
    }

    fn nudge_selected_output(&mut self, delta: (i32, i32)) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(profile_index) = self.selected_profile_index() else {
            if let Some(output_index) = self.runtime_canvas.selected_output {
                self.manual_layout
                    .nudge(&connected_outputs, output_index, delta);
            }
            return;
        };

        if let Some(output_index) = self.canvas.selected_output
            && let Some(profile) = self
                .config
                .as_mut()
                .and_then(|config| config.profiles.get_mut(profile_index))
            && nudge_profile_output(profile, &connected_outputs, output_index, delta)
        {
            self.mark_draft_changed();
        }
    }

    fn show_header(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let summary = daemon_summary(&self.daemon);
        match show_header(
            ui,
            &summary,
            self.niri_error(),
            self.is_dirty(),
            self.can_save(),
        ) {
            Some(HeaderAction::Save) => self.save_draft(),
            Some(HeaderAction::ToggleTheme) => {
                let theme = theme::opposite(ui.ctx().theme());
                ui.ctx().set_theme(theme);
                // The rest of this frame still uses the palette read at the start of `update`.
                ui.ctx().request_repaint();
                if let Some(storage) = frame.storage_mut() {
                    theme::store(storage, theme);
                }
            }
            None => {}
        }
    }

    fn show_problems(&self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        if let Some(error) = &self.load_error {
            ui.colored_label(
                palette.error,
                format!("The config could not be loaded: {error}"),
            );
        }
        if let Some(error) = &self.validation_error {
            ui.colored_label(
                palette.error,
                format!("The config has a problem, so it cannot be saved: {error}"),
            );
        }
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(profile_index) = self.selected_profile_index() else {
            match sidebar::show_outputs(ui, &connected_outputs, self.runtime_canvas.selected_output)
            {
                Some(SidebarAction::Select(index)) => {
                    self.runtime_canvas.selected_output = Some(index);
                }
                Some(SidebarAction::New) => self.create_profile(),
                _ => {}
            }
            return;
        };

        let active_profile = match &self.daemon {
            DaemonConnectionState::Running(status) => status.selected_profile.clone(),
            DaemonConnectionState::Checking | DaemonConnectionState::Offline { .. } => None,
        };
        let Some(config) = self.config.as_mut() else {
            return;
        };
        let (action, renamed) = sidebar::show_profiles(
            ui,
            &mut config.profiles,
            Some(profile_index),
            self.renaming_profile,
            active_profile.as_deref(),
            &connected_outputs,
        );

        if renamed {
            self.mark_draft_changed();
        }
        match action {
            Some(SidebarAction::Select(index)) => self.select_profile(index),
            Some(SidebarAction::StartRename(index)) => {
                self.select_profile(index);
                self.renaming_profile = true;
            }
            Some(SidebarAction::FinishRename) => self.renaming_profile = false,
            Some(SidebarAction::Delete(index)) => self.pending_delete = Some(index),
            Some(SidebarAction::New) => self.create_profile(),
            None => {}
        }
    }

    fn show_inspector(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.selected_profile_index() {
                Some(profile_index) => self.show_profile_inspector(ui, profile_index),
                None => self.show_live_inspector(ui),
            });
    }

    fn show_profile_inspector(&mut self, ui: &mut egui::Ui, profile_index: usize) {
        let palette = theme::palette(ui.ctx());
        let connected_outputs = self.connected_outputs().to_vec();
        let daemon_running = self.daemon_running();
        let can_preview = daemon_running && !self.is_dirty();
        let Some(profile) = self
            .config
            .as_mut()
            .and_then(|config| config.profiles.get_mut(profile_index))
        else {
            return;
        };

        let mut changed = false;
        if profile.outputs.is_empty() {
            ui.label(RichText::new(&profile.name).heading().strong());
            ui.label(theme::weak(ui, "This profile has no outputs."));
        } else {
            let output_index = self
                .canvas
                .selected_output
                .filter(|index| *index < profile.outputs.len())
                .unwrap_or(0);
            self.canvas.selected_output = Some(output_index);
            let output = &mut profile.outputs[output_index];
            let connected = connected_outputs
                .iter()
                .find(|connected| matcher_matches_output(&output.matcher, connected));
            changed = show_output_settings(ui, output, connected);
        }

        if let Some(overlap) =
            first_overlap_message(&derive_output_rects(profile, &connected_outputs))
        {
            theme::section(ui, "Layout", |ui| {
                ui.colored_label(
                    palette.error,
                    format!("{overlap}. Move them apart: the daemon refuses overlapping layouts."),
                );
            });
        }

        let mut preview_clicked = false;
        theme::section(ui, "Plan", |ui| {
            ui.label(theme::weak(
                ui,
                "See what the daemon would change to apply this profile now.",
            ));
            preview_clicked = ui
                .add_enabled(can_preview, egui::Button::new("Preview plan"))
                .on_disabled_hover_text(if daemon_running {
                    "Save first: the daemon previews the saved config"
                } else {
                    "Start niri-monitorsd to preview plans"
                })
                .clicked();

            if let Some(preview) = self
                .preview_data
                .as_ref()
                .filter(|preview| preview.profile == profile.name)
            {
                ui.add_space(4.0);
                for warning in &preview.warnings {
                    ui.colored_label(palette.warning, warning);
                }
                egui::ScrollArea::vertical()
                    .id_salt("preview")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.monospace(&preview.dry_run_output);
                    });
            }
        });

        if changed {
            self.mark_draft_changed();
        }
        if preview_clicked {
            self.preview_selected_profile();
        }
    }

    fn show_live_inspector(&mut self, ui: &mut egui::Ui) {
        let outputs = self.connected_outputs().to_vec();
        let overlap = self.manual_layout.first_overlap_message(&outputs);
        let status = LiveLayoutStatus {
            has_outputs: !outputs.is_empty(),
            has_changes: !self.manual_layout.changed_placements(&outputs).is_empty(),
            overlap: overlap.as_deref(),
            daemon_running: self.daemon_running(),
        };
        let selected_index = self.runtime_canvas.selected_output;
        let selected = selected_index.and_then(|index| outputs.get(index));
        let mut settings =
            selected_index.and_then(|index| self.manual_layout.settings(&outputs, index));

        let action = show_live_layout(ui, selected, settings.as_mut(), &status);
        if let (Some(index), Some(settings)) = (selected_index, settings) {
            self.manual_layout.set_settings(&outputs, index, settings);
        }
        match action {
            Some(LiveLayoutAction::Apply) => self.apply_manual_layout(),
            Some(LiveLayoutAction::Reset) => self.manual_layout.reset(),
            None => {}
        }
    }

    fn show_monitor_area(&mut self, ui: &mut egui::Ui) {
        let connected_outputs = self.connected_outputs().to_vec();
        let Some(profile_index) = self.selected_profile_index() else {
            let empty_message = self.live_empty_message();
            show_editable_live_monitor_canvas(
                ui,
                &connected_outputs,
                &mut self.manual_layout,
                &mut self.runtime_canvas,
                &empty_message,
            );
            return;
        };

        if let Some(profile) = self
            .config
            .as_mut()
            .and_then(|config| config.profiles.get_mut(profile_index))
            && show_monitor_canvas(ui, profile, &connected_outputs, &mut self.canvas)
        {
            self.mark_draft_changed();
        }
    }

    fn live_empty_message(&self) -> String {
        match (&self.live_outputs, self.connected_outputs().is_empty()) {
            (LiveOutputsState::Offline { message }, true) => format!(
                "Cannot reach niri: {message}. Start this window from inside a niri session."
            ),
            (LiveOutputsState::Checking, true) => {
                "Asking niri for the connected monitors…".to_owned()
            }
            (_, true) => "niri reports no connected monitors.".to_owned(),
            (_, false) => "All connected monitors are off.".to_owned(),
        }
    }

    fn show_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(index) = self.pending_delete else {
            return;
        };
        let Some(profile_name) = self
            .config
            .as_ref()
            .and_then(|config| config.profiles.get(index))
            .map(|profile| profile.name.clone())
        else {
            self.pending_delete = None;
            return;
        };

        let mut confirmed = false;
        let mut cancelled = false;
        let palette = theme::palette(ctx);
        let frame = egui::Frame::popup(&ctx.style())
            .fill(palette.surface)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(Margin::same(16));
        let modal = egui::Modal::new(egui::Id::new("delete_profile"))
            .frame(frame)
            .show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new(format!("Delete “{profile_name}”?")).heading().strong());
            ui.add_space(4.0);
            ui.label(theme::weak(
                ui,
                "The profile is removed from the draft. Save to write the change to the config file.",
            ));
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                confirmed = ui.add(theme::danger_button(ui, "Delete")).clicked();
                cancelled = ui.button("Cancel").clicked();
            });
        });

        if confirmed {
            self.confirm_delete_profile(index);
        } else if cancelled || modal.should_close() {
            self.pending_delete = None;
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.refresh_daemon_status();
        self.refresh_live_outputs();

        let palette = theme::palette(ctx);
        let compact = ctx.screen_rect().width() < COMPACT_WIDTH;

        egui::TopBottomPanel::top("header")
            .frame(theme::panel_frame(palette).inner_margin(Margin::symmetric(16, 10)))
            .show(ctx, |ui| self.show_header(ui, frame));
        if self.load_error.is_some() || self.validation_error.is_some() {
            egui::TopBottomPanel::top("problems")
                .frame(
                    egui::Frame::new()
                        .fill(palette.window.lerp_to_gamma(palette.error, 0.15))
                        .inner_margin(Margin::symmetric(16, 8)),
                )
                .show(ctx, |ui| self.show_problems(ui));
        }
        egui::SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(if compact { 200.0 } else { 240.0 })
            .frame(theme::panel_frame(palette))
            .show(ctx, |ui| self.show_sidebar(ui));
        if compact {
            egui::TopBottomPanel::bottom("inspector")
                .resizable(true)
                .default_height(300.0)
                .height_range(160.0..=ctx.screen_rect().height() * 0.6)
                .frame(theme::panel_frame(palette))
                .show(ctx, |ui| self.show_inspector(ui));
        } else {
            egui::SidePanel::right("inspector")
                .resizable(false)
                .exact_width(320.0)
                .frame(theme::panel_frame(palette))
                .show(ctx, |ui| self.show_inspector(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.canvas))
            .show(ctx, |ui| self.show_monitor_area(ui));

        self.show_delete_dialog(ctx);
        self.toasts.show(ctx);

        let busy = self.daemon_poller.is_fetching()
            || self.live_outputs_poller.is_fetching()
            || !self.toasts.is_empty();
        ctx.request_repaint_after(if busy {
            Duration::from_millis(100)
        } else {
            DAEMON_POLL_INTERVAL
        });
    }

    /// Only the theme is persisted, through `theme::store`.
    fn persist_egui_memory(&self) -> bool {
        false
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
