use crate::gui::canvas::LiveOutputSettings;
use crate::gui::format_scale;
use crate::gui::theme::{self, Tone};
use crate::model::{ModeSpec, MonitorMatcher, Position, ProfileOutput, Transform};
use crate::niri::output::{NiriMode, NiriOutput};
use eframe::egui::{self, RichText};
use std::cmp::Reverse;

const SCALE_PRESETS: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];
const MIN_SCALE: f64 = 0.25;
const MAX_SCALE: f64 = 4.0;
const SCALE_STEP: f64 = 0.25;
const ROTATIONS: [u16; 4] = [0, 90, 180, 270];

/// Edits one profile output. A setting left unset keeps the output's current value when
/// the profile is applied; each setting that is set has a reset button that unsets it.
pub fn show_output_settings(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected: Option<&NiriOutput>,
) -> bool {
    output_heading(ui, output, connected);
    ui.add_space(4.0);

    let mut changed = false;
    theme::section(ui, "Display", |ui| {
        let mut enabled = output.enabled.unwrap_or(true);
        if ui.checkbox(&mut enabled, "Turn on").changed() {
            output.enabled = Some(enabled);
            changed = true;
        }
        if output.enabled == Some(false) {
            ui.label(theme::weak(
                ui,
                "The profile turns this output off, so its other settings are not applied.",
            ));
        }
        ui.add_enabled_ui(output.enabled != Some(false), |ui| {
            ui.add_space(4.0);
            changed |= edit_mode(ui, &mut output.mode, connected);
            ui.add_space(4.0);
            changed |= edit_vrr(ui, &mut output.vrr, connected);
        });
    });

    ui.add_enabled_ui(output.enabled != Some(false), |ui| {
        theme::card(ui, |ui| {
            changed |= edit_scale(ui, &mut output.scale, connected);
        });
        theme::card(ui, |ui| {
            changed |= edit_rotation(ui, &mut output.transform, connected);
        });
        theme::card(ui, |ui| {
            changed |= edit_position(ui, &mut output.position, connected);
        });
    });
    changed
}

fn output_heading(ui: &mut egui::Ui, output: &ProfileOutput, connected: Option<&NiriOutput>) {
    let title = connected.map_or_else(
        || matcher_summary(&output.matcher),
        |connected| connected.connector.clone(),
    );
    ui.label(RichText::new(title).heading().strong());
    match connected {
        Some(connected) => {
            ui.label(theme::weak(ui, connected.display_name()));
        }
        None => {
            theme::pill(ui, Tone::Warning, "Not connected").on_hover_text(
                "No connected output matches this entry. Its settings apply when one does.",
            );
        }
    }
}

/// A setting's title, a hint with the current value while it is unset, and a reset button
/// while it is set. Returns whether reset was clicked.
fn setting_header(ui: &mut egui::Ui, title: &str, is_set: bool, current: Option<String>) -> bool {
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        if !is_set && let Some(current) = current {
            ui.label(theme::weak(ui, format!("keeps {current}")));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_set
                && ui
                    .small_button("Reset")
                    .on_hover_text("Unset this, so the output keeps its current value")
                    .clicked()
            {
                reset = true;
            }
        });
    });
    reset
}

fn edit_mode(
    ui: &mut egui::Ui,
    mode: &mut Option<ModeSpec>,
    connected: Option<&NiriOutput>,
) -> bool {
    let current = connected
        .and_then(NiriOutput::active_mode)
        .map(|current| ModeSpec::from(*current).to_string());
    if setting_header(ui, "Mode", mode.is_some(), current) {
        *mode = None;
        return true;
    }

    let Some(connected) = connected else {
        return edit_mode_text(ui, mode);
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt("output_mode")
        .width(ui.available_width())
        .selected_text(mode.map_or_else(|| "Keep current".to_owned(), |mode| mode.to_string()))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(mode.is_none(), "Keep current")
                .clicked()
            {
                *mode = None;
                changed = true;
            }
            for &available in &connected.modes {
                let available = ModeSpec::from(available);
                if ui
                    .selectable_label(*mode == Some(available), available.to_string())
                    .clicked()
                {
                    *mode = Some(available);
                    changed = true;
                }
            }
        });

    if let Some(mode) = *mode
        && !connected
            .modes
            .iter()
            .any(|available| available.matches(mode))
    {
        ui.colored_label(
            theme::palette(ui.ctx()).warning,
            "The connected output does not offer this mode.",
        );
    }
    changed
}

/// Free-text mode field for an output that is not connected. A half-typed mode cannot be
/// stored in the profile, so the typed text is kept in egui memory while the field has
/// focus and the profile changes only when the text parses or is cleared.
fn edit_mode_text(ui: &mut egui::Ui, mode: &mut Option<ModeSpec>) -> bool {
    let id = ui.id().with("mode_text");
    let mut text = ui
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| mode.map(|mode| mode.to_string()).unwrap_or_default());
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("WIDTHxHEIGHT@HZ")
            .desired_width(f32::INFINITY),
    );

    let trimmed = text.trim();
    let parsed = if trimmed.is_empty() {
        Some(None)
    } else {
        trimmed.parse::<ModeSpec>().ok().map(Some)
    };
    let changed = response.changed() && parsed.is_some_and(|parsed| parsed != *mode);
    if changed && let Some(parsed) = parsed {
        *mode = parsed;
    }

    if response.has_focus() {
        if parsed.is_none() {
            ui.colored_label(
                theme::palette(ui.ctx()).error,
                "Not a mode yet, for example 2560x1440@60",
            );
        }
        ui.data_mut(|data| data.insert_temp(id, text));
    } else {
        ui.data_mut(|data| data.remove::<String>(id));
    }
    changed
}

fn edit_vrr(ui: &mut egui::Ui, vrr: &mut Option<bool>, connected: Option<&NiriOutput>) -> bool {
    let supported = connected.is_some_and(|connected| connected.vrr_supported);
    let current = connected.map(|connected| if connected.vrr_enabled { "on" } else { "off" });
    if setting_header(
        ui,
        "Variable refresh rate",
        vrr.is_some(),
        current.map(str::to_owned),
    ) {
        *vrr = None;
        return true;
    }

    let mut enabled =
        vrr.unwrap_or_else(|| connected.is_some_and(|connected| connected.vrr_enabled));
    let response = ui
        .add_enabled(
            supported || vrr.is_some(),
            egui::Checkbox::new(&mut enabled, "Use VRR"),
        )
        .on_disabled_hover_text("The connected output does not report VRR support");
    if response.changed() {
        *vrr = Some(enabled);
    }
    if *vrr == Some(true) && !supported {
        ui.colored_label(
            theme::palette(ui.ctx()).warning,
            "The connected output does not report VRR support.",
        );
    }
    response.changed()
}

fn edit_scale(ui: &mut egui::Ui, scale: &mut Option<f64>, connected: Option<&NiriOutput>) -> bool {
    let current = connected
        .and_then(|connected| connected.logical.as_ref())
        .map(|logical| logical.scale);
    if setting_header(ui, "Scale", scale.is_some(), current.map(format_scale)) {
        *scale = None;
        return true;
    }

    let effective = scale.or(current);
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for preset in SCALE_PRESETS {
            let selected = effective.is_some_and(|value| (value - preset).abs() < 0.001);
            if ui
                .selectable_label(selected, format_scale(preset))
                .clicked()
            {
                *scale = Some(preset);
                changed = true;
            }
        }
    });
    let mut value = effective.unwrap_or(1.0);
    ui.horizontal(|ui| {
        ui.label(theme::weak(ui, "Custom"));
        if ui
            .add(
                egui::DragValue::new(&mut value)
                    .range(MIN_SCALE..=MAX_SCALE)
                    .speed(0.01)
                    .max_decimals(3)
                    .suffix("×"),
            )
            .changed()
        {
            *scale = Some(value);
            changed = true;
        }
    });
    changed
}

fn edit_rotation(
    ui: &mut egui::Ui,
    transform: &mut Option<Transform>,
    connected: Option<&NiriOutput>,
) -> bool {
    let current = connected
        .and_then(|connected| connected.logical.as_ref())
        .map(|logical| logical.transform);
    if setting_header(
        ui,
        "Rotation",
        transform.is_some(),
        current.map(describe_transform),
    ) {
        *transform = None;
        return true;
    }

    let (degrees, flipped) = rotation(transform.or(current).unwrap_or(Transform::Normal));
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for option in ROTATIONS {
            if ui
                .selectable_label(degrees == option, format!("{option}°"))
                .clicked()
            {
                *transform = Some(transform_from(option, flipped));
                changed = true;
            }
        }
    });
    let mut mirrored = flipped;
    if ui
        .checkbox(&mut mirrored, "Mirrored")
        .on_hover_text("Flip the output horizontally")
        .changed()
    {
        *transform = Some(transform_from(degrees, mirrored));
        changed = true;
    }
    changed
}

fn edit_position(
    ui: &mut egui::Ui,
    position: &mut Option<Position>,
    connected: Option<&NiriOutput>,
) -> bool {
    let current = connected
        .and_then(|connected| connected.logical.as_ref())
        .map(|logical| Position {
            x: logical.x,
            y: logical.y,
        });
    if setting_header(
        ui,
        "Position",
        position.is_some(),
        current.map(|current| format!("{}, {}", current.x, current.y)),
    ) {
        *position = None;
        return true;
    }

    let mut value = position.or(current).unwrap_or(Position { x: 0, y: 0 });
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(theme::weak(ui, "x"));
        changed |= ui.add(egui::DragValue::new(&mut value.x)).changed();
        ui.add_space(8.0);
        ui.label(theme::weak(ui, "y"));
        changed |= ui.add(egui::DragValue::new(&mut value.y)).changed();
    });
    if changed {
        *position = Some(value);
    }
    changed
}

pub enum LiveLayoutAction {
    Apply,
    Reset,
}

pub struct LiveLayoutStatus<'a> {
    pub has_outputs: bool,
    pub has_changes: bool,
    pub overlap: Option<&'a str>,
    pub daemon_running: bool,
}

/// Shows the selected live output with its editable parameters, and the controls that
/// apply or discard the draft layout. `settings` are the selected output's draft settings,
/// `None` while it is off; they are edited in place.
pub fn show_live_layout(
    ui: &mut egui::Ui,
    selected: Option<&NiriOutput>,
    settings: Option<&mut LiveOutputSettings>,
    status: &LiveLayoutStatus<'_>,
) -> Option<LiveLayoutAction> {
    let palette = theme::palette(ui.ctx());
    match selected {
        Some(output) => {
            ui.label(RichText::new(&output.connector).heading().strong());
            ui.label(theme::weak(ui, output.display_name()));
            ui.add_space(4.0);
            theme::section(ui, "Parameters", |ui| match settings {
                Some(settings) => edit_live_settings(ui, &output.modes, settings),
                None => {
                    ui.label(theme::weak(
                        ui,
                        "This output is off. Save a profile that turns it on to use it.",
                    ));
                }
            });
        }
        None => {
            ui.label(RichText::new("Live layout").heading().strong());
            ui.label(theme::weak(
                ui,
                "Select a monitor on the canvas or in the list.",
            ));
        }
    }

    let mut action = None;
    theme::section(ui, "Layout", |ui| {
        let (tone, text) = if status.overlap.is_some() {
            (Tone::Error, "Monitors overlap")
        } else if status.has_changes {
            (Tone::Warning, "Not applied yet")
        } else {
            (Tone::Ok, "Matches the monitors")
        };
        if status.has_outputs {
            theme::pill(ui, tone, text);
        }
        if let Some(overlap) = status.overlap {
            ui.colored_label(
                palette.error,
                format!("{overlap}. Move them apart to apply."),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let apply = ui
                .add_enabled(
                    status.has_outputs && status.has_changes && status.overlap.is_none(),
                    theme::primary_button(ui, "Apply"),
                )
                .on_disabled_hover_text(if status.overlap.is_some() {
                    "Move overlapping monitors apart first"
                } else {
                    "Change a parameter or drag a monitor first"
                });
            if apply.clicked() {
                action = Some(LiveLayoutAction::Apply);
            }
            if ui
                .add_enabled(status.has_changes, egui::Button::new("Reset"))
                .on_hover_text("Discard all changes")
                .clicked()
            {
                action = Some(LiveLayoutAction::Reset);
            }
        });
        ui.label(theme::weak(
            ui,
            if status.daemon_running {
                "Applying through the daemon pauses automatic profiles until you save the \
                 config or turn automatic mode back on."
            } else {
                "The daemon is not running, so the layout is applied directly through niri."
            },
        ));
    });
    ui.label(theme::weak(
        ui,
        "Drag monitors to arrange them. Arrow keys move the selected monitor by 1 px, \
         Shift+arrow by 10 px.",
    ));
    action
}

fn edit_live_settings(ui: &mut egui::Ui, modes: &[NiriMode], settings: &mut LiveOutputSettings) {
    egui::Grid::new("live_settings")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(theme::weak(ui, "Mode"));
            edit_live_mode(ui, modes, &mut settings.mode);
            ui.end_row();

            ui.label(theme::weak(ui, "Scale"));
            edit_live_scale(ui, &mut settings.scale);
            ui.end_row();

            ui.label(theme::weak(ui, "Rotation"));
            edit_live_rotation(ui, &mut settings.transform);
            ui.end_row();

            ui.label(theme::weak(ui, "Position"));
            ui.horizontal(|ui| {
                ui.label(theme::weak(ui, "x"));
                ui.add(egui::DragValue::new(&mut settings.position.x));
                ui.add_space(4.0);
                ui.label(theme::weak(ui, "y"));
                ui.add(egui::DragValue::new(&mut settings.position.y));
            });
            ui.end_row();
        });
}

fn edit_live_mode(ui: &mut egui::Ui, modes: &[NiriMode], mode: &mut Option<ModeSpec>) {
    egui::ComboBox::from_id_salt("live_mode")
        .width(ui.available_width())
        .selected_text(mode.map_or_else(|| "Unknown".to_owned(), format_mode))
        .show_ui(ui, |ui| {
            for available in sorted_modes(modes) {
                ui.selectable_value(mode, Some(available), format_mode(available));
            }
        });
}

/// `−` and `+` step to the previous or next multiple of `SCALE_STEP`; the value itself can
/// be dragged or typed.
fn edit_live_scale(ui: &mut egui::Ui, scale: &mut f64) {
    ui.horizontal(|ui| {
        let smaller = step_scale(*scale, false);
        if ui
            .add_enabled(smaller < *scale, egui::Button::new("−"))
            .on_hover_text(format!("Set to {}", format_scale(smaller)))
            .clicked()
        {
            *scale = smaller;
        }
        ui.add(
            egui::DragValue::new(scale)
                .range(MIN_SCALE..=MAX_SCALE)
                .clamp_existing_to_range(false)
                .speed(0.01)
                .max_decimals(3)
                .suffix("×"),
        )
        .on_hover_text("Drag, or click and type a scale");
        let larger = step_scale(*scale, true);
        if ui
            .add_enabled(larger > *scale, egui::Button::new("+"))
            .on_hover_text(format!("Set to {}", format_scale(larger)))
            .clicked()
        {
            *scale = larger;
        }
    });
}

/// The next multiple of `SCALE_STEP` above or below `scale`, within the scale range.
fn step_scale(scale: f64, up: bool) -> f64 {
    // A scale within rounding error of a step counts as on that step.
    let steps = scale / SCALE_STEP;
    let next = if up {
        (steps + 1e-6).floor() + 1.0
    } else {
        (steps - 1e-6).ceil() - 1.0
    };
    (next * SCALE_STEP).clamp(MIN_SCALE, MAX_SCALE)
}

/// `↱` on the left turns the output 90° clockwise, `↰` on the right turns it 90°
/// counterclockwise. niri counts angles counterclockwise, so `↰` adds 90°. A mirrored
/// output stays mirrored.
fn edit_live_rotation(ui: &mut egui::Ui, transform: &mut Transform) {
    let (degrees, flipped) = rotation(*transform);
    let counterclockwise = (degrees + 90) % 360;
    let clockwise = (degrees + 270) % 360;
    ui.horizontal(|ui| {
        // Of egui's default fonts, only the monospace one has these arrows.
        if ui
            .button(RichText::new("↱").monospace())
            .on_hover_text(format!("Rotate clockwise, to {clockwise}°"))
            .clicked()
        {
            *transform = transform_from(clockwise, flipped);
        }
        // A fixed width keeps the arrows in place while the angle changes.
        ui.add_sized(
            [40.0, ui.spacing().interact_size.y],
            egui::Label::new(format!("{degrees}°")),
        );
        if ui
            .button(RichText::new("↰").monospace())
            .on_hover_text(format!("Rotate counterclockwise, to {counterclockwise}°"))
            .clicked()
        {
            *transform = transform_from(counterclockwise, flipped);
        }
        if flipped {
            ui.label(theme::weak(ui, "mirrored"));
        }
    });
}

/// For example `2560×1440 @ 59.951 Hz`.
fn format_mode(mode: ModeSpec) -> String {
    let size = format!("{}×{}", mode.width, mode.height);
    match mode.refresh_millihz {
        Some(millihz) => {
            let hz = format!("{}.{:03}", millihz / 1000, millihz % 1000);
            format!(
                "{size} @ {} Hz",
                hz.trim_end_matches('0').trim_end_matches('.')
            )
        }
        None => size,
    }
}

/// The distinct modes in `modes`, largest resolution first, then fastest refresh rate.
fn sorted_modes(modes: &[NiriMode]) -> Vec<ModeSpec> {
    let mut sorted: Vec<ModeSpec> = modes.iter().map(|mode| ModeSpec::from(*mode)).collect();
    sorted.sort_by_key(|mode| {
        Reverse((
            u32::from(mode.width) * u32::from(mode.height),
            mode.width,
            mode.refresh_millihz,
        ))
    });
    sorted.dedup();
    sorted
}

pub fn matcher_summary(matcher: &MonitorMatcher) -> String {
    matcher
        .description
        .clone()
        .or_else(|| matcher.connector.clone())
        .or_else(|| matcher.make.clone())
        .or_else(|| matcher.model.clone())
        .or_else(|| matcher.serial.clone())
        .unwrap_or_else(|| "empty matcher".to_owned())
}

fn describe_transform(transform: Transform) -> String {
    match rotation(transform) {
        (degrees, false) => format!("{degrees}°"),
        (degrees, true) => format!("{degrees}°, mirrored"),
    }
}

/// Splits a transform into its counterclockwise rotation in degrees, as Wayland and niri
/// count it, and whether it is flipped.
fn rotation(transform: Transform) -> (u16, bool) {
    match transform {
        Transform::Normal => (0, false),
        Transform::Rotate90 => (90, false),
        Transform::Rotate180 => (180, false),
        Transform::Rotate270 => (270, false),
        Transform::Flipped => (0, true),
        Transform::Flipped90 => (90, true),
        Transform::Flipped180 => (180, true),
        Transform::Flipped270 => (270, true),
    }
}

fn transform_from(degrees: u16, flipped: bool) -> Transform {
    match (degrees, flipped) {
        (90, false) => Transform::Rotate90,
        (180, false) => Transform::Rotate180,
        (270, false) => Transform::Rotate270,
        (0, true) => Transform::Flipped,
        (90, true) => Transform::Flipped90,
        (180, true) => Transform::Flipped180,
        (270, true) => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_flip_round_trip_every_transform() {
        for transform in Transform::ALL {
            let (degrees, flipped) = rotation(transform);
            assert_eq!(transform_from(degrees, flipped), transform);
        }
    }

    #[test]
    fn modes_are_distinct_and_largest_first() {
        let modes = [
            mode(1920, 1080, 60_000),
            mode(3440, 1440, 49_987),
            mode(1920, 1080, 60_000),
            mode(2560, 1440, 59_951),
            mode(3440, 1440, 59_973),
        ];

        let sorted: Vec<String> = sorted_modes(&modes).into_iter().map(format_mode).collect();

        assert_eq!(
            sorted,
            [
                "3440×1440 @ 59.973 Hz",
                "3440×1440 @ 49.987 Hz",
                "2560×1440 @ 59.951 Hz",
                "1920×1080 @ 60 Hz",
            ]
        );
    }

    #[test]
    fn scale_steps_go_to_the_next_quarter_within_range() {
        assert_eq!(step_scale(1.0, true), 1.25);
        assert_eq!(step_scale(1.0, false), 0.75);
        assert_eq!(step_scale(1.1, true), 1.25);
        assert_eq!(step_scale(1.1, false), 1.0);
        assert_eq!(step_scale(1.250_000_01, true), 1.5);
        assert_eq!(step_scale(0.25, false), 0.25);
        assert_eq!(step_scale(4.0, true), 4.0);
    }

    fn mode(width: u16, height: u16, refresh_millihz: u32) -> NiriMode {
        NiriMode {
            width,
            height,
            refresh_millihz,
            is_preferred: false,
        }
    }

    #[test]
    fn matcher_summary_prefers_description() {
        let matcher = MonitorMatcher {
            connector: Some("DP-1".to_owned()),
            description: Some("Dell Display".to_owned()),
            ..MonitorMatcher::default()
        };

        assert_eq!(matcher_summary(&matcher), "Dell Display");
    }
}
