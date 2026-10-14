use crate::gui::{ERROR_COLOR, WARNING_COLOR};
use crate::matching::matcher_matches_output;
use crate::model::{ModeSpec, MonitorMatcher, Position, Profile, ProfileOutput, Transform};
use crate::niri::output::NiriOutput;
use eframe::egui;
use eframe::egui::{Color32, RichText};

const MUTED_TEXT: Color32 = Color32::from_rgb(146, 148, 160);

pub fn show_output_strip(
    ui: &mut egui::Ui,
    selected_output: &mut Option<usize>,
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
) -> bool {
    if profile.outputs.is_empty() {
        ui.label("This profile does not have output settings yet.");
        return false;
    }

    if selected_output.is_none_or(|index| index >= profile.outputs.len()) {
        *selected_output = Some(0);
    }

    let Some(output_index) = *selected_output else {
        ui.label("Select an output to edit settings.");
        return false;
    };

    let summary = matcher_summary(&profile.outputs[output_index].matcher);
    let connected_output = connected_outputs.iter().find(|connected_output| {
        matcher_matches_output(&profile.outputs[output_index].matcher, connected_output)
    });
    let Some(output) = profile.outputs.get_mut(output_index) else {
        ui.colored_label(WARNING_COLOR, "Selected output no longer exists.");
        return false;
    };

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&summary).strong().size(18.0));
        if let Some(connected_output) = connected_output {
            ui.label(RichText::new(&connected_output.description).color(MUTED_TEXT));
        } else {
            ui.colored_label(WARNING_COLOR, "not connected");
        }
    });

    let mut changed = false;
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        changed |= edit_enabled_inline(ui, output);
        ui.add_space(12.0);
        changed |= edit_mode_inline(ui, output, connected_output, 260.0);
        ui.add_space(12.0);
        changed |= edit_scale_inline(ui, output, connected_output);
        ui.add_space(12.0);
        changed |= edit_transform_inline(ui, output, 150.0);
        ui.add_space(12.0);
        changed |= edit_position_inline(ui, output, connected_output);
    });

    show_output_warnings(ui, output, connected_output);
    changed
}

fn edit_enabled_inline(ui: &mut egui::Ui, output: &mut ProfileOutput) -> bool {
    let mut enabled = output.enabled.unwrap_or(true);
    let changed = ui.checkbox(&mut enabled, "Enabled").changed();
    if changed {
        output.enabled = Some(enabled);
    }
    changed
}

fn edit_mode_inline(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
    width: f32,
) -> bool {
    ui.label(RichText::new("Mode").color(MUTED_TEXT));
    let mut changed = false;

    if let Some(connected_output) = connected_output {
        let selected = output
            .mode
            .map_or_else(|| "Default".to_owned(), |mode| mode.to_string());
        egui::ComboBox::from_id_salt("selected_output_mode_strip")
            .selected_text(selected)
            .width(width)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(output.mode.is_none(), "Default")
                    .clicked()
                {
                    output.mode = None;
                    changed = true;
                }
                for &mode in &connected_output.modes {
                    let mode = ModeSpec::from(mode);
                    if ui
                        .selectable_label(output.mode == Some(mode), mode.to_string())
                        .clicked()
                    {
                        output.mode = Some(mode);
                        changed = true;
                    }
                }
            });
    } else {
        changed = edit_mode_text(ui, &mut output.mode, width);
    }

    changed
}

/// Free-text mode field for an output that is not connected. A half-typed mode cannot be
/// stored in the profile, so the typed text is kept in egui memory while the field has
/// focus and the profile changes only when the text parses or is cleared.
fn edit_mode_text(ui: &mut egui::Ui, mode: &mut Option<ModeSpec>, width: f32) -> bool {
    let id = ui.id().with("mode_text");
    let mut text = ui
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| mode.map(|mode| mode.to_string()).unwrap_or_default());
    let response = ui.add_sized(
        [width, ui.spacing().interact_size.y],
        egui::TextEdit::singleline(&mut text).hint_text("WIDTHxHEIGHT@HZ"),
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
            ui.colored_label(ERROR_COLOR, "not a mode");
        }
        ui.data_mut(|data| data.insert_temp(id, text));
    } else {
        ui.data_mut(|data| data.remove::<String>(id));
    }
    changed
}

fn edit_scale_inline(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    ui.label(RichText::new("Scale").color(MUTED_TEXT));
    let mut scale = output
        .scale
        .or_else(|| {
            connected_output.and_then(|output| output.logical.as_ref().map(|logical| logical.scale))
        })
        .unwrap_or(1.0);
    let changed = ui
        .add_sized(
            [86.0, ui.spacing().interact_size.y],
            egui::DragValue::new(&mut scale).speed(0.05),
        )
        .changed();
    if changed {
        output.scale = Some(scale);
    }
    changed
}

fn edit_transform_inline(ui: &mut egui::Ui, output: &mut ProfileOutput, width: f32) -> bool {
    ui.label(RichText::new("Transform").color(MUTED_TEXT));
    let mut changed = false;
    let selected = output.transform.map_or("None", transform_label);

    egui::ComboBox::from_id_salt("selected_output_transform_strip")
        .selected_text(selected)
        .width(width)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(output.transform.is_none(), "None")
                .clicked()
            {
                output.transform = None;
                changed = true;
            }
            for transform in Transform::ALL {
                if ui
                    .selectable_label(
                        output.transform == Some(transform),
                        transform_label(transform),
                    )
                    .clicked()
                {
                    output.transform = Some(transform);
                    changed = true;
                }
            }
        });

    changed
}

fn edit_position_inline(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    let mut position = output
        .position
        .or_else(|| {
            connected_output
                .and_then(|output| output.logical.as_ref())
                .map(|logical| Position {
                    x: logical.x,
                    y: logical.y,
                })
        })
        .unwrap_or(Position { x: 0, y: 0 });

    ui.label(RichText::new("Position").color(MUTED_TEXT));
    ui.label(RichText::new("x").color(MUTED_TEXT));
    let x_changed = ui
        .add_sized(
            [84.0, ui.spacing().interact_size.y],
            egui::DragValue::new(&mut position.x).speed(1),
        )
        .changed();
    ui.label(RichText::new("y").color(MUTED_TEXT));
    let y_changed = ui
        .add_sized(
            [84.0, ui.spacing().interact_size.y],
            egui::DragValue::new(&mut position.y).speed(1),
        )
        .changed();

    if x_changed || y_changed {
        output.position = Some(position);
        true
    } else {
        false
    }
}

fn transform_label(transform: Transform) -> &'static str {
    match transform {
        Transform::Normal => "Normal",
        transform => transform.as_str(),
    }
}

fn show_output_warnings(
    ui: &mut egui::Ui,
    output: &ProfileOutput,
    connected_output: Option<&NiriOutput>,
) {
    if output
        .scale
        .is_some_and(|scale| !scale.is_finite() || scale <= 0.0)
    {
        ui.colored_label(ERROR_COLOR, "Scale must be finite and greater than zero.");
    }

    if let (Some(mode), Some(connected_output)) = (output.mode, connected_output) {
        let mode_supported = connected_output
            .modes
            .iter()
            .any(|connected_mode| connected_mode.matches(mode));
        if !mode_supported {
            ui.colored_label(
                WARNING_COLOR,
                "Configured mode is not reported by the connected output.",
            );
        }
    }

    if output.vrr == Some(true) && !connected_output.is_some_and(|output| output.vrr_supported) {
        ui.colored_label(
            WARNING_COLOR,
            "VRR is enabled but the connected output does not report VRR support.",
        );
    }
}

fn matcher_summary(matcher: &MonitorMatcher) -> String {
    matcher
        .description
        .clone()
        .or_else(|| matcher.connector.clone())
        .or_else(|| matcher.make.clone())
        .or_else(|| matcher.model.clone())
        .or_else(|| matcher.serial.clone())
        .unwrap_or_else(|| "empty matcher".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
