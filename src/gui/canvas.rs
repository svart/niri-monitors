use crate::model::{Position, Profile};
use crate::niri::output::NiriOutput;
use crate::placement::{
    OutputRect, SnapAxis, SnapLine, derive_connected_output_rects, derive_output_rects,
    first_overlap_message as placement_first_overlap_message, normalize_output_rect_positions,
    overlapping_output_pairs, snap_position,
};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

const SNAP_THRESHOLD: i32 = 12;
const VIEW_PADDING: f32 = 120.0;
const CANVAS_BG: Color32 = Color32::from_rgb(36, 36, 36);
const MONITOR_FILL: Color32 = Color32::from_rgb(12, 12, 12);
const MONITOR_STROKE: Color32 = Color32::from_rgb(132, 132, 142);
const SELECTED_FILL: Color32 = Color32::from_rgb(18, 18, 24);
const SELECTED_STROKE: Color32 = Color32::from_rgb(172, 168, 213);
const SNAP_STROKE: Color32 = Color32::from_rgb(120, 170, 255);

#[derive(Debug, Default, Clone)]
pub struct CanvasState {
    pub selected_output: Option<usize>,
    pub dragging: Option<DragState>,
    pub snap_lines: Vec<SnapLine>,
    pub frozen_view: Option<ViewBox>,
}

#[derive(Debug, Clone, Copy)]
pub struct DragState {
    pub output_index: usize,
    pub grab_offset_x: f32,
    pub grab_offset_y: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewBox {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManualLayoutDraft {
    output_keys: Vec<String>,
    current_positions: Vec<Position>,
    draft_positions: Vec<Position>,
}

impl ManualLayoutDraft {
    pub fn sync_from_outputs(&mut self, outputs: &[NiriOutput]) {
        let rects = derive_connected_output_rects(outputs);
        let output_keys = rects
            .iter()
            .filter_map(|rect| outputs.get(rect.output_index))
            .map(|output| output.connector.clone())
            .collect::<Vec<_>>();
        let current_positions = rects
            .iter()
            .map(|rect| Position {
                x: rect.x,
                y: rect.y,
            })
            .collect::<Vec<_>>();

        if self.output_keys != output_keys
            || !self.has_changes()
            || self.draft_positions == current_positions
        {
            self.output_keys = output_keys;
            self.current_positions = current_positions.clone();
            self.draft_positions = current_positions;
        } else {
            self.current_positions = current_positions;
        }
    }

    pub fn has_changes(&self) -> bool {
        self.current_positions != self.draft_positions
    }

    pub fn reset(&mut self) {
        self.draft_positions = self.current_positions.clone();
    }

    pub fn changed_positions(&self) -> Vec<(String, Position)> {
        self.output_keys
            .iter()
            .zip(self.current_positions.iter())
            .zip(self.draft_positions.iter())
            .filter_map(|((output, current), draft)| {
                (current != draft).then_some((output.clone(), *draft))
            })
            .collect()
    }

    pub fn normalized_changed_positions(&self, outputs: &[NiriOutput]) -> Vec<(String, Position)> {
        let mut rects = self.draft_rects(outputs);
        normalize_output_rect_positions(&mut rects);

        rects
            .iter()
            .filter_map(|rect| {
                let output = outputs.get(rect.output_index)?;
                let current = output.logical.as_ref()?;
                let normalized = Position {
                    x: rect.x,
                    y: rect.y,
                };
                (normalized.x != current.x || normalized.y != current.y)
                    .then_some((output.connector.clone(), normalized))
            })
            .collect()
    }

    pub fn draft_rects(&self, outputs: &[NiriOutput]) -> Vec<OutputRect> {
        let mut rects = derive_connected_output_rects(outputs);
        for (draft_index, rect) in rects.iter_mut().enumerate() {
            if let Some(position) = self.draft_positions.get(draft_index) {
                rect.x = position.x;
                rect.y = position.y;
            }
        }
        rects
    }

    pub fn first_overlap_message(&self, outputs: &[NiriOutput]) -> Option<String> {
        placement_first_overlap_message(&self.draft_rects(outputs))
    }
}

pub fn show_monitor_canvas(
    ui: &mut egui::Ui,
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
    state: &mut CanvasState,
) -> bool {
    let mut changed = false;
    let rects = derive_output_rects(profile, connected_outputs);
    if state
        .selected_output
        .is_some_and(|index| index >= profile.outputs.len())
    {
        state.selected_output = None;
    }

    if rects.is_empty() {
        ui.label("No profile outputs to place yet.");
        return false;
    }

    let available_width = ui.available_width().max(320.0);
    let available_height = ui.available_height().max(320.0);
    let desired_size = Vec2::new(available_width, available_height);
    let (canvas_rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
    let view = state
        .frozen_view
        .unwrap_or_else(|| ViewBox::from_rects(&rects));

    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.selected_output = hit_test(pointer, &rects, view, canvas_rect);
        state.dragging = state.selected_output.and_then(|output_index| {
            rects
                .iter()
                .find(|rect| rect.output_index == output_index)
                .map(|rect| {
                    let pointer = screen_to_logical(pointer, view, canvas_rect);
                    DragState {
                        output_index,
                        grab_offset_x: pointer.x - rect.x as f32,
                        grab_offset_y: pointer.y - rect.y as f32,
                    }
                })
        });
        state.frozen_view = Some(view);
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.selected_output = hit_test(pointer, &rects, view, canvas_rect);
    }

    if response.dragged()
        && let (Some(pointer), Some(dragging)) = (response.interact_pointer_pos(), state.dragging)
        && let Some(moving) = rects
            .iter()
            .find(|rect| rect.output_index == dragging.output_index)
    {
        let pointer = screen_to_logical(pointer, view, canvas_rect);
        let proposed = Position {
            x: (pointer.x - dragging.grab_offset_x).round() as i32,
            y: (pointer.y - dragging.grab_offset_y).round() as i32,
        };
        let other_rects = rects
            .iter()
            .filter(|rect| rect.output_index != dragging.output_index)
            .cloned()
            .collect::<Vec<_>>();
        let snapped = snap_position(moving, proposed, &other_rects, SNAP_THRESHOLD);
        state.snap_lines = snapped.snap_lines;
        if let Some(output) = profile.outputs.get_mut(dragging.output_index) {
            output.position = Some(snapped.position);
            changed = true;
        }
    }

    if response.drag_stopped() {
        state.dragging = None;
        state.snap_lines.clear();
        state.frozen_view = None;
    }

    draw_canvas(ui, canvas_rect, view, &rects, state);
    changed
}

pub fn show_editable_live_monitor_canvas(
    ui: &mut egui::Ui,
    outputs: &[NiriOutput],
    draft: &mut ManualLayoutDraft,
    state: &mut CanvasState,
) -> bool {
    draft.sync_from_outputs(outputs);
    let mut changed = false;
    let rects = draft.draft_rects(outputs);

    if state
        .selected_output
        .is_some_and(|selected| !rects.iter().any(|rect| rect.output_index == selected))
    {
        state.selected_output = None;
    }

    if rects.is_empty() {
        if outputs.is_empty() {
            ui.label("No connected outputs reported by niri.");
        } else {
            ui.label("No enabled outputs have logical positions to display.");
        }
        return false;
    }

    let available_width = ui.available_width().max(320.0);
    let available_height = ui.available_height().max(360.0);
    let desired_size = Vec2::new(available_width, available_height);
    let (canvas_rect, response) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
    let view = state
        .frozen_view
        .unwrap_or_else(|| ViewBox::from_rects(&rects));

    if response.drag_started()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.selected_output = hit_test(pointer, &rects, view, canvas_rect);
        state.dragging = state.selected_output.and_then(|output_index| {
            rects
                .iter()
                .find(|rect| rect.output_index == output_index)
                .map(|rect| {
                    let pointer = screen_to_logical(pointer, view, canvas_rect);
                    DragState {
                        output_index,
                        grab_offset_x: pointer.x - rect.x as f32,
                        grab_offset_y: pointer.y - rect.y as f32,
                    }
                })
        });
        state.frozen_view = Some(view);
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.selected_output = hit_test(pointer, &rects, view, canvas_rect);
    }

    if response.dragged()
        && let (Some(pointer), Some(dragging)) = (response.interact_pointer_pos(), state.dragging)
        && let Some(moving) = rects
            .iter()
            .find(|rect| rect.output_index == dragging.output_index)
    {
        let pointer = screen_to_logical(pointer, view, canvas_rect);
        let proposed = Position {
            x: (pointer.x - dragging.grab_offset_x).round() as i32,
            y: (pointer.y - dragging.grab_offset_y).round() as i32,
        };
        let other_rects = rects
            .iter()
            .filter(|rect| rect.output_index != dragging.output_index)
            .cloned()
            .collect::<Vec<_>>();
        let snapped = snap_position(moving, proposed, &other_rects, SNAP_THRESHOLD);
        state.snap_lines = snapped.snap_lines;

        if let Some(draft_index) = rects
            .iter()
            .position(|rect| rect.output_index == dragging.output_index)
            && let Some(position) = draft.draft_positions.get_mut(draft_index)
            && *position != snapped.position
        {
            *position = snapped.position;
            changed = true;
        }
    }

    if response.drag_stopped() {
        state.dragging = None;
        state.snap_lines.clear();
        state.frozen_view = None;
    }

    draw_canvas(ui, canvas_rect, view, &rects, state);
    changed
}

pub fn show_live_monitor_canvas(ui: &mut egui::Ui, outputs: &[NiriOutput]) {
    let rects = derive_connected_output_rects(outputs);
    if rects.is_empty() {
        if outputs.is_empty() {
            ui.label("No connected outputs reported by daemon.");
        } else {
            ui.label("No enabled outputs have logical positions to display.");
        }
        return;
    }

    let available_width = ui.available_width().max(320.0);
    let available_height = ui.available_height().max(320.0);
    let desired_size = Vec2::new(available_width, available_height);
    let (canvas_rect, _) = ui.allocate_exact_size(desired_size, Sense::hover());
    let view = ViewBox::from_rects(&rects);
    let state = CanvasState::default();

    draw_canvas(ui, canvas_rect, view, &rects, &state);
}

fn draw_canvas(
    ui: &egui::Ui,
    canvas_rect: Rect,
    view: ViewBox,
    rects: &[OutputRect],
    state: &CanvasState,
) {
    let painter = ui.painter_at(canvas_rect);
    let overlap_pairs = overlapping_output_pairs(rects);
    painter.rect_filled(canvas_rect, 0.0, CANVAS_BG);

    for line in &state.snap_lines {
        match line.axis {
            SnapAxis::Vertical => {
                let x = logical_to_screen(
                    Pos2::new(line.position as f32, view.min_y),
                    view,
                    canvas_rect,
                )
                .x;
                paint_dashed_line(
                    &painter,
                    Pos2::new(x, canvas_rect.top()),
                    Pos2::new(x, canvas_rect.bottom()),
                    Stroke::new(2.0, SNAP_STROKE),
                );
            }
            SnapAxis::Horizontal => {
                let y = logical_to_screen(
                    Pos2::new(view.min_x, line.position as f32),
                    view,
                    canvas_rect,
                )
                .y;
                paint_dashed_line(
                    &painter,
                    Pos2::new(canvas_rect.left(), y),
                    Pos2::new(canvas_rect.right(), y),
                    Stroke::new(2.0, SNAP_STROKE),
                );
            }
        }
    }

    for rect in rects {
        let screen_rect = output_screen_rect(rect, view, canvas_rect);
        let selected = state.selected_output == Some(rect.output_index);
        let overlapping = overlap_pairs.iter().any(|overlap| {
            overlap.first_output_index == rect.output_index
                || overlap.second_output_index == rect.output_index
        });
        let fill = if overlapping {
            Color32::from_rgb(54, 24, 18)
        } else if selected {
            SELECTED_FILL
        } else if !rect.enabled {
            Color32::from_rgb(48, 48, 48)
        } else if rect.connected {
            MONITOR_FILL
        } else {
            Color32::from_rgb(28, 23, 17)
        };
        let stroke = if overlapping {
            Stroke::new(2.0, Color32::from_rgb(230, 107, 73))
        } else if selected {
            Stroke::new(2.0, SELECTED_STROKE)
        } else if rect.connected {
            Stroke::new(1.5, MONITOR_STROKE)
        } else {
            Stroke::new(1.5, Color32::from_rgb(154, 111, 72))
        };

        painter.rect_filled(screen_rect, 5.0, fill);
        painter.rect_stroke(screen_rect, 5.0, stroke, StrokeKind::Inside);
        paint_output_label(&painter, screen_rect, rect);
    }
}

fn paint_dashed_line(painter: &egui::Painter, start: Pos2, end: Pos2, stroke: Stroke) {
    let delta = end - start;
    let length = delta.length();
    if length <= 0.0 {
        return;
    }

    let direction = delta / length;
    let dash = 14.0;
    let gap = 12.0;
    let mut offset = 0.0;
    while offset < length {
        let dash_end = (offset + dash).min(length);
        painter.line_segment(
            [start + direction * offset, start + direction * dash_end],
            stroke,
        );
        offset += dash + gap;
    }
}

fn paint_output_label(painter: &egui::Painter, screen_rect: Rect, rect: &OutputRect) {
    let min_side = screen_rect.width().min(screen_rect.height()).max(1.0);
    let name_size = (min_side * 0.12).clamp(14.0, 42.0);
    let size_size = (name_size * 0.64).clamp(11.0, 28.0);
    let center = screen_rect.center();
    let name = rect.label.lines().next().unwrap_or(rect.label.as_str());
    let resolution = format!("{}x{}", rect.width, rect.height);

    painter.text(
        center - Vec2::new(0.0, size_size * 0.65),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(name_size),
        Color32::from_rgb(236, 236, 240),
    );
    painter.text(
        center + Vec2::new(0.0, name_size * 0.55),
        egui::Align2::CENTER_CENTER,
        resolution,
        egui::FontId::proportional(size_size),
        Color32::from_rgb(214, 214, 220),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn manual_layout_draft_tracks_changed_positions() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0)]);

        assert!(!draft.has_changes());

        draft.draft_positions[0] = Position { x: 120, y: 80 };

        assert!(draft.has_changes());
        assert_eq!(
            draft.changed_positions(),
            vec![("DP-1".to_string(), Position { x: 120, y: 80 })]
        );
    }

    #[test]
    fn manual_layout_draft_resets_when_live_position_matches_draft() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0)]);
        draft.draft_positions[0] = Position { x: 120, y: 80 };

        draft.sync_from_outputs(&[output("DP-1", 120, 80)]);

        assert!(!draft.has_changes());
        assert!(draft.changed_positions().is_empty());
    }

    #[test]
    fn manual_layout_draft_reports_overlap_message() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0), output("DP-2", 4_000, 0)]);
        draft.draft_positions[1] = Position { x: 10, y: 10 };

        assert_eq!(
            draft.first_overlap_message(&[output("DP-1", 0, 0), output("DP-2", 4_000, 0)]),
            Some("DP-1 overlaps DP-2".to_string())
        );
    }

    #[test]
    fn manual_layout_apply_positions_are_normalized() {
        let mut draft = ManualLayoutDraft::default();
        let outputs = [output("DP-1", 0, 0), output("DP-2", 4_000, 0)];
        draft.sync_from_outputs(&outputs);
        draft.draft_positions[0] = Position { x: -100, y: -50 };
        draft.draft_positions[1] = Position { x: 3_340, y: -50 };

        assert_eq!(
            draft.normalized_changed_positions(&outputs),
            vec![("DP-2".to_string(), Position { x: 3_440, y: 0 })]
        );
    }

    fn output(connector: &str, x: i32, y: i32) -> NiriOutput {
        NiriOutput {
            connector: connector.to_string(),
            make: "Dell Inc.".to_string(),
            model: "DELL U3419W".to_string(),
            serial: Some("7VK66T2".to_string()),
            description: "Dell Inc. DELL U3419W 7VK66T2".to_string(),
            modes: vec![NiriMode {
                width: 3440,
                height: 1440,
                refresh_millihz: 59973,
                is_preferred: true,
            }],
            current_mode: Some(0),
            logical: Some(LogicalOutput {
                x,
                y,
                width: 3440,
                height: 1440,
                scale: 1.0,
                transform: "normal".to_string(),
            }),
            vrr_supported: true,
            vrr_enabled: false,
        }
    }
}

fn hit_test(
    pointer: Pos2,
    rects: &[OutputRect],
    view: ViewBox,
    canvas_rect: Rect,
) -> Option<usize> {
    rects
        .iter()
        .rev()
        .find(|rect| output_screen_rect(rect, view, canvas_rect).contains(pointer))
        .map(|rect| rect.output_index)
}

fn output_screen_rect(rect: &OutputRect, view: ViewBox, canvas_rect: Rect) -> Rect {
    Rect::from_min_size(
        logical_to_screen(Pos2::new(rect.x as f32, rect.y as f32), view, canvas_rect),
        logical_size_to_screen(
            Vec2::new(rect.width as f32, rect.height as f32),
            view,
            canvas_rect,
        ),
    )
}

fn logical_to_screen(point: Pos2, view: ViewBox, canvas_rect: Rect) -> Pos2 {
    let scale = view.scale(canvas_rect);
    let used_size = Vec2::new(view.width * scale, view.height * scale);
    let origin = canvas_rect.min + ((canvas_rect.size() - used_size) / 2.0);
    Pos2::new(
        origin.x + ((point.x - view.min_x) * scale),
        origin.y + ((point.y - view.min_y) * scale),
    )
}

fn screen_to_logical(point: Pos2, view: ViewBox, canvas_rect: Rect) -> Pos2 {
    let scale = view.scale(canvas_rect);
    let used_size = Vec2::new(view.width * scale, view.height * scale);
    let origin = canvas_rect.min + ((canvas_rect.size() - used_size) / 2.0);
    Pos2::new(
        ((point.x - origin.x) / scale) + view.min_x,
        ((point.y - origin.y) / scale) + view.min_y,
    )
}

fn logical_size_to_screen(size: Vec2, view: ViewBox, canvas_rect: Rect) -> Vec2 {
    size * view.scale(canvas_rect)
}

impl ViewBox {
    fn from_rects(rects: &[OutputRect]) -> Self {
        let min_x = rects.iter().map(|rect| rect.x).min().unwrap_or(0) as f32 - VIEW_PADDING;
        let min_y = rects.iter().map(|rect| rect.y).min().unwrap_or(0) as f32 - VIEW_PADDING;
        let max_x = rects
            .iter()
            .map(|rect| rect.x as f32 + rect.width as f32)
            .fold(1920.0, f32::max)
            + VIEW_PADDING;
        let max_y = rects
            .iter()
            .map(|rect| rect.y as f32 + rect.height as f32)
            .fold(1080.0, f32::max)
            + VIEW_PADDING;

        Self {
            min_x,
            min_y,
            width: (max_x - min_x).max(1.0),
            height: (max_y - min_y).max(1.0),
        }
    }

    fn scale(self, canvas_rect: Rect) -> f32 {
        (canvas_rect.width() / self.width)
            .min(canvas_rect.height() / self.height)
            .max(0.001)
    }
}
