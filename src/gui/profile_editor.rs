use crate::matching::{matcher_matches_any_output, profile_matches_outputs};
use crate::model::{MonitorMatcher, Profile};
use crate::niri::output::NiriOutput;
use egui::{Color32, RichText};

const OK_COLOR: Color32 = Color32::from_rgb(72, 143, 92);
const WARNING_COLOR: Color32 = Color32::from_rgb(184, 111, 36);
const ERROR_COLOR: Color32 = Color32::from_rgb(184, 67, 67);

pub fn show_condition_editor(
    ui: &mut egui::Ui,
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
) -> bool {
    let mut changed = false;

    ui.heading("Conditions");
    show_match_diagnostics(ui, profile, connected_outputs);
    ui.add_space(8.0);

    changed |= show_matcher_group(
        ui,
        "All Connected",
        "Every matcher in this group must match a connected output.",
        &mut profile.condition.all_connected,
        connected_outputs,
    );
    changed |= show_matcher_group(
        ui,
        "Any Connected",
        "At least one matcher in this group must match. Empty means no requirement.",
        &mut profile.condition.any_connected,
        connected_outputs,
    );
    changed |= show_matcher_group(
        ui,
        "None Connected",
        "No matcher in this group may match a connected output.",
        &mut profile.condition.none_connected,
        connected_outputs,
    );

    changed
}

fn show_match_diagnostics(ui: &mut egui::Ui, profile: &Profile, connected_outputs: &[NiriOutput]) {
    if connected_outputs.is_empty() {
        ui.colored_label(
            WARNING_COLOR,
            "No daemon outputs available for match diagnostics.",
        );
        return;
    }

    if profile_matches_outputs(profile, connected_outputs) {
        ui.colored_label(OK_COLOR, "This profile matches the current daemon outputs.");
    } else {
        ui.colored_label(
            WARNING_COLOR,
            "This profile does not match the current daemon outputs.",
        );
    }
}

fn show_matcher_group(
    ui: &mut egui::Ui,
    title: &str,
    help: &str,
    matchers: &mut Vec<MonitorMatcher>,
    connected_outputs: &[NiriOutput],
) -> bool {
    let mut changed = false;
    ui.separator();
    ui.collapsing(title, |ui| {
        ui.label(help);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add Empty Matcher").clicked() {
                matchers.push(MonitorMatcher::default());
                changed = true;
            }
            for output in connected_outputs {
                if ui
                    .button(format!("Add {}", output.connector))
                    .on_hover_text(&output.description)
                    .clicked()
                {
                    matchers.push(matcher_from_output(output));
                    changed = true;
                }
            }
        });

        if matchers.is_empty() {
            ui.label("No matchers in this group.");
            return;
        }

        let mut remove_index = None;
        for (index, matcher) in matchers.iter_mut().enumerate() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Matcher {}", index + 1)).strong());
                    if ui.button("Remove").clicked() {
                        remove_index = Some(index);
                    }
                });

                changed |= show_matcher_fields(ui, matcher);
                show_matcher_diagnostic(ui, matcher, connected_outputs);
            });
        }

        if let Some(index) = remove_index {
            matchers.remove(index);
            changed = true;
        }
    });

    changed
}

fn show_matcher_fields(ui: &mut egui::Ui, matcher: &mut MonitorMatcher) -> bool {
    let mut changed = false;
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            changed |= optional_text_edit(ui, "Description", &mut matcher.description);
            changed |= optional_text_edit(ui, "Connector", &mut matcher.connector);
            changed |= optional_text_edit(ui, "Make", &mut matcher.make);
            changed |= optional_text_edit(ui, "Model", &mut matcher.model);
            changed |= optional_text_edit(ui, "Serial", &mut matcher.serial);
        });
    changed
}

fn optional_text_edit(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) -> bool {
    ui.label(label);
    let mut text = value.clone().unwrap_or_default();
    let changed = ui.text_edit_singleline(&mut text).changed();
    if changed {
        let trimmed = text.trim();
        *value = (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    ui.end_row();
    changed
}

fn show_matcher_diagnostic(
    ui: &mut egui::Ui,
    matcher: &MonitorMatcher,
    connected_outputs: &[NiriOutput],
) {
    if !matcher_has_field(matcher) {
        ui.colored_label(ERROR_COLOR, "Matcher is empty and cannot be saved.");
    } else if connected_outputs.is_empty() {
        ui.label("No daemon outputs available to test this matcher.");
    } else if matcher_matches_any_output(matcher, connected_outputs) {
        ui.colored_label(OK_COLOR, "Matches a connected output.");
    } else {
        ui.colored_label(WARNING_COLOR, "Does not match any connected output.");
    }
}

fn matcher_from_output(output: &NiriOutput) -> MonitorMatcher {
    MonitorMatcher {
        connector: None,
        description: Some(output.description.clone()),
        make: None,
        model: None,
        serial: None,
    }
}

fn matcher_has_field(matcher: &MonitorMatcher) -> bool {
    [
        matcher.connector.as_ref(),
        matcher.description.as_ref(),
        matcher.make.as_ref(),
        matcher.model.as_ref(),
        matcher.serial.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}
