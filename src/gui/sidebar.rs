use crate::gui::theme::{self, Palette};
use crate::matching::profile_matches_outputs;
use crate::model::Profile;
use crate::niri::output::NiriOutput;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Stroke};

const ROW_HEIGHT: f32 = 46.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    Select(usize),
    StartRename(usize),
    FinishRename,
    Delete(usize),
    New,
}

/// Lists profiles with whether each matches the connected monitors and which one the
/// daemon has active. Returns what the user did and whether a name was edited.
pub fn show_profiles(
    ui: &mut egui::Ui,
    profiles: &mut [Profile],
    selected: Option<usize>,
    renaming: bool,
    active_profile: Option<&str>,
    connected_outputs: &[NiriOutput],
) -> (Option<SidebarAction>, bool) {
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    let mut renamed = false;

    heading(ui, "Profiles");
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .max_height(ui.available_height() - 44.0)
        .show(ui, |ui| {
            for (index, profile) in profiles.iter_mut().enumerate() {
                let is_selected = selected == Some(index);
                if is_selected && renaming {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut profile.name)
                            .desired_width(f32::INFINITY)
                            .min_size(egui::vec2(0.0, ROW_HEIGHT - 12.0)),
                    );
                    renamed |= response.changed();
                    let done = ui.input(|input| {
                        input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Escape)
                    });
                    if done || response.lost_focus() {
                        action = Some(SidebarAction::FinishRename);
                    } else {
                        response.request_focus();
                    }
                    continue;
                }

                let matches = if connected_outputs.is_empty() {
                    None
                } else {
                    Some(profile_matches_outputs(profile, connected_outputs))
                };
                let (dot, status) = match (profile.enabled, matches) {
                    (false, _) => (palette.text_weak, "turned off"),
                    (true, Some(true)) => (palette.ok, "matches"),
                    (true, Some(false)) => (palette.text_weak, "does not match"),
                    (true, None) => (palette.text_weak, "monitors unknown"),
                };
                let is_active = active_profile == Some(profile.id.as_str());
                let response = row(
                    ui,
                    palette,
                    is_selected,
                    dot,
                    &profile.name,
                    &format!("Priority {} · {status}", profile.priority),
                    is_active.then_some(("Active", palette.accent)),
                )
                .on_hover_text("Double-click to rename. Right-click for more.");

                if response.double_clicked() {
                    action = Some(SidebarAction::StartRename(index));
                } else if response.clicked() {
                    action = Some(SidebarAction::Select(index));
                }
                response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        action = Some(SidebarAction::StartRename(index));
                        ui.close_menu();
                    }
                    if ui.button("Delete…").clicked() {
                        action = Some(SidebarAction::Delete(index));
                        ui.close_menu();
                    }
                });
            }
        });

    if new_button(ui, "+ New profile")
        .on_hover_text("Create a profile from the monitors connected now")
        .clicked()
    {
        action = Some(SidebarAction::New);
    }
    (action, renamed)
}

/// Lists the outputs niri reports, including outputs that are off, for the live layout.
pub fn show_outputs(
    ui: &mut egui::Ui,
    outputs: &[NiriOutput],
    selected: Option<usize>,
) -> Option<SidebarAction> {
    let palette = theme::palette(ui.ctx());
    let mut action = None;

    heading(ui, "Outputs");
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .max_height(ui.available_height() - 88.0)
        .show(ui, |ui| {
            for (index, output) in outputs.iter().enumerate() {
                let on = output.logical.is_some();
                if row(
                    ui,
                    palette,
                    selected == Some(index),
                    if on { palette.ok } else { palette.text_weak },
                    &output.connector,
                    &output.display_name(),
                    (!on).then_some(("Off", palette.text_weak)),
                )
                .clicked()
                {
                    action = Some(SidebarAction::Select(index));
                }
            }
        });

    ui.label(theme::weak(
        ui,
        "No profiles yet. Arrange the monitors live, or save them as a profile.",
    ));
    ui.add_space(4.0);
    if new_button(ui, "+ Save as profile")
        .on_hover_text("Create a profile from the monitors connected now")
        .clicked()
    {
        action = Some(SidebarAction::New);
    }
    action
}

fn heading(ui: &mut egui::Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .small()
            .strong()
            .color(palette.text_weak),
    );
    ui.add_space(4.0);
}

fn new_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(
        egui::vec2(ui.available_width(), 30.0),
        egui::Button::new(text),
    )
}

/// One clickable list entry: a status dot, a title, a detail line, and an optional badge.
fn row(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected: bool,
    dot: Color32,
    title: &str,
    detail: &str,
    badge: Option<(&str, Color32)>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let fill = if selected {
        palette.control_active
    } else if response.hovered() {
        palette.control_hover
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            CornerRadius::same(2),
            palette.accent,
        );
    }

    let text_left = rect.left() + 26.0;
    painter.circle_filled(egui::pos2(rect.left() + 13.0, rect.top() + 16.0), 4.0, dot);
    let badge_width = badge.map_or(0.0, |(badge, color)| {
        let galley = painter.layout_no_wrap(badge.to_owned(), FontId::proportional(11.0), color);
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(rect.right() - galley.size().x - 16.0, rect.top() + 7.0),
            galley.size() + egui::vec2(10.0, 4.0),
        );
        painter.rect_stroke(
            badge_rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, color),
            egui::StrokeKind::Inside,
        );
        painter.galley(badge_rect.min + egui::vec2(5.0, 2.0), galley, color);
        badge_rect.width() + 8.0
    });

    let text_painter = painter.with_clip_rect(egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - badge_width - 4.0, rect.bottom()),
    ));
    text_painter.text(
        egui::pos2(text_left, rect.top() + 16.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(14.0),
        palette.text,
    );
    text_painter.text(
        egui::pos2(text_left, rect.top() + 33.0),
        Align2::LEFT_CENTER,
        detail,
        FontId::proportional(11.5),
        palette.text_weak,
    );
    response
}
