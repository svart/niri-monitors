use crate::matching::matcher_matches_output;
use crate::model::{MonitorMatcher, Position, Profile, ProfileOutput};
use crate::niri::output::{NiriMode, NiriOutput};
use egui::{Color32, RichText};

const ERROR_COLOR: Color32 = Color32::from_rgb(184, 67, 67);
const WARNING_COLOR: Color32 = Color32::from_rgb(184, 111, 36);
const MUTED_TEXT: Color32 = Color32::from_rgb(146, 148, 160);

const TRANSFORMS: &[&str] = &[
    "normal",
    "90",
    "180",
    "270",
    "flipped",
    "flipped-90",
    "flipped-180",
    "flipped-270",
];

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
        changed |= edit_transform_inline(ui, output, connected_output, 150.0);
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
        let selected = output.mode.clone().unwrap_or_else(|| "Default".to_owned());
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
                for mode in &connected_output.modes {
                    let mode_text = format_mode(*mode);
                    if ui
                        .selectable_label(
                            output.mode.as_deref() == Some(mode_text.as_str()),
                            &mode_text,
                        )
                        .clicked()
                    {
                        output.mode = Some(mode_text);
                        changed = true;
                    }
                }
            });
    } else {
        let mut text = output.mode.clone().unwrap_or_default();
        changed = ui
            .add_sized(
                [width, ui.spacing().interact_size.y],
                egui::TextEdit::singleline(&mut text),
            )
            .changed();
        if changed {
            let trimmed = text.trim();
            output.mode = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
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

fn edit_transform_inline(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
    width: f32,
) -> bool {
    ui.label(RichText::new("Transform").color(MUTED_TEXT));
    let mut changed = false;
    let selected = output
        .transform
        .as_deref()
        .map(transform_label)
        .or_else(|| {
            connected_output
                .and_then(|output| output.logical.as_ref())
                .map(|_| "None")
        })
        .unwrap_or("None");

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
            for transform in TRANSFORMS {
                let label = transform_label(transform);
                if ui
                    .selectable_label(output.transform.as_deref() == Some(*transform), label)
                    .clicked()
                {
                    output.transform = Some((*transform).to_owned());
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

fn transform_label(transform: &str) -> &str {
    if transform == "normal" {
        "Normal"
    } else {
        transform
    }
}

pub fn show_output_editor(
    ui: &mut egui::Ui,
    selected_output: &mut Option<usize>,
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
) -> bool {
    ui.heading("Output Settings");

    if profile.outputs.is_empty() {
        ui.label("This profile does not have output settings yet.");
        return false;
    }

    if selected_output.is_none_or(|index| index >= profile.outputs.len()) {
        *selected_output = Some(0);
    }

    ui.horizontal_wrapped(|ui| {
        ui.label("Output:");
        for (index, output) in profile.outputs.iter().enumerate() {
            let selected = *selected_output == Some(index);
            if ui
                .selectable_label(selected, matcher_summary(&output.matcher))
                .clicked()
            {
                *selected_output = Some(index);
            }
        }
    });

    let Some(output_index) = *selected_output else {
        ui.label("Select an output to edit settings.");
        return false;
    };

    let Some(output) = profile.outputs.get_mut(output_index) else {
        ui.colored_label(WARNING_COLOR, "Selected output no longer exists.");
        return false;
    };

    let connected_output = connected_outputs
        .iter()
        .find(|connected_output| matcher_matches_output(&output.matcher, connected_output));
    let mut changed = false;

    ui.label(format!("Matcher: {}", matcher_summary(&output.matcher)));
    if let Some(connected_output) = connected_output {
        ui.label(format!(
            "Connected: {} ({})",
            connected_output.connector, connected_output.description
        ));
    } else {
        ui.colored_label(
            WARNING_COLOR,
            "No connected daemon output matches this profile output.",
        );
    }

    ui.add_space(8.0);
    egui::Grid::new("selected_output_editor")
        .num_columns(2)
        .spacing([20.0, 8.0])
        .striped(true)
        .show(ui, |ui| {
            changed |= edit_enabled(ui, output);
            changed |= edit_mode(ui, output, connected_output);
            changed |= edit_scale(ui, output, connected_output);
            changed |= edit_transform(ui, output, connected_output);
            changed |= edit_position(ui, output, connected_output);
            changed |= edit_vrr(ui, output, connected_output);
        });

    show_output_warnings(ui, output, connected_output);
    changed
}

fn edit_enabled(ui: &mut egui::Ui, output: &mut ProfileOutput) -> bool {
    ui.label("Enabled");
    let mut enabled = output.enabled.unwrap_or(true);
    let changed = ui.checkbox(&mut enabled, "").changed();
    if changed {
        output.enabled = Some(enabled);
    }
    ui.end_row();
    changed
}

fn edit_mode(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    ui.label("Mode");
    let mut changed = false;
    if let Some(connected_output) = connected_output {
        let selected = output
            .mode
            .clone()
            .unwrap_or_else(|| "use current".to_owned());
        egui::ComboBox::from_id_salt("selected_output_mode")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(output.mode.is_none(), "use current")
                    .clicked()
                {
                    output.mode = None;
                    changed = true;
                }
                for mode in &connected_output.modes {
                    let mode_text = format_mode(*mode);
                    if ui
                        .selectable_label(
                            output.mode.as_deref() == Some(mode_text.as_str()),
                            &mode_text,
                        )
                        .clicked()
                    {
                        output.mode = Some(mode_text);
                        changed = true;
                    }
                }
            });
    } else {
        changed = optional_text_edit(ui, &mut output.mode);
    }
    ui.end_row();
    changed
}

fn edit_scale(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    ui.label("Scale");
    let mut scale = output
        .scale
        .or_else(|| {
            connected_output.and_then(|output| output.logical.as_ref().map(|logical| logical.scale))
        })
        .unwrap_or(1.0);
    let changed = ui
        .add(egui::DragValue::new(&mut scale).speed(0.05))
        .changed();
    if changed {
        output.scale = Some(scale);
    }
    ui.end_row();
    changed
}

fn edit_transform(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    ui.label("Transform");
    let mut changed = false;
    let selected = output
        .transform
        .clone()
        .or_else(|| {
            connected_output.and_then(|output| {
                output
                    .logical
                    .as_ref()
                    .map(|logical| logical.transform.clone())
            })
        })
        .unwrap_or_else(|| "normal".to_owned());
    egui::ComboBox::from_id_salt("selected_output_transform")
        .selected_text(&selected)
        .show_ui(ui, |ui| {
            for transform in TRANSFORMS {
                if ui
                    .selectable_label(
                        output.transform.as_deref().unwrap_or(selected.as_str()) == *transform,
                        *transform,
                    )
                    .clicked()
                {
                    output.transform = Some((*transform).to_owned());
                    changed = true;
                }
            }
        });
    ui.end_row();
    changed
}

fn edit_position(
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

    ui.label("Position X");
    let x_changed = ui
        .add(egui::DragValue::new(&mut position.x).speed(1))
        .changed();
    ui.end_row();

    ui.label("Position Y");
    let y_changed = ui
        .add(egui::DragValue::new(&mut position.y).speed(1))
        .changed();
    ui.end_row();

    if x_changed || y_changed {
        output.position = Some(position);
        true
    } else {
        false
    }
}

fn edit_vrr(
    ui: &mut egui::Ui,
    output: &mut ProfileOutput,
    connected_output: Option<&NiriOutput>,
) -> bool {
    ui.label("VRR");
    let supported = connected_output.is_some_and(|output| output.vrr_supported);
    let mut vrr = output
        .vrr
        .unwrap_or_else(|| connected_output.is_some_and(|output| output.vrr_enabled));
    let response = ui.add_enabled(
        supported || output.vrr.is_some(),
        egui::Checkbox::new(&mut vrr, ""),
    );
    if response.changed() {
        output.vrr = Some(vrr);
        ui.end_row();
        true
    } else {
        ui.end_row();
        false
    }
}

fn optional_text_edit(ui: &mut egui::Ui, value: &mut Option<String>) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.text_edit_singleline(&mut text).changed();
    if changed {
        let trimmed = text.trim();
        *value = (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    changed
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

    if let Some(transform) = &output.transform
        && !TRANSFORMS.contains(&transform.as_str())
    {
        ui.colored_label(ERROR_COLOR, format!("Unsupported transform: {transform}"));
    }

    if let (Some(mode), Some(connected_output)) = (&output.mode, connected_output) {
        let mode_supported = connected_output
            .modes
            .iter()
            .any(|connected_mode| format_mode(*connected_mode) == *mode);
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

fn format_mode(mode: NiriMode) -> String {
    let mut refresh = format!("{:.3}", mode.refresh_hz());
    while refresh.contains('.') && refresh.ends_with('0') {
        refresh.pop();
    }
    if refresh.ends_with('.') {
        refresh.pop();
    }
    format!("{}x{}@{}", mode.width, mode.height, refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_modes_without_hz_suffix() {
        let mode = NiriMode {
            width: 3440,
            height: 1440,
            refresh_millihz: 59973,
            is_preferred: true,
        };

        assert_eq!(format_mode(mode), "3440x1440@59.973");
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
