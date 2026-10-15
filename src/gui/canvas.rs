use crate::gui::format_scale;
use crate::gui::theme::{self, Palette};
use crate::matching::matcher_matches_output;
use crate::model::{ManualOutputPlacement, ModeSpec, Position, Profile, Transform};
use crate::niri::output::NiriOutput;
use crate::placement::{
    OutputRect, SCALE_TOLERANCE, SnapAxis, SnapLine, derive_connected_output_rects,
    derive_output_rects, first_overlap_message as placement_first_overlap_message,
    normalize_output_rect_positions, overlapping_output_pairs, size_after, snap_position,
};
use eframe::egui::{
    self, Align2, CornerRadius, FontId, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, Vec2,
};

const SNAP_THRESHOLD: i32 = 12;
const VIEW_PADDING: f32 = 120.0;
const GRID_SPACING: f32 = 24.0;
const TILE_RADIUS: u8 = 8;

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

/// Runtime-only edits to the live layout. Entries follow the order of
/// `derive_connected_output_rects`, so they cover enabled outputs only.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ManualLayoutDraft {
    output_keys: Vec<String>,
    current: Vec<LiveOutputSettings>,
    draft: Vec<LiveOutputSettings>,
}

/// The settings of one enabled output that the live layout can change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveOutputSettings {
    pub position: Position,
    /// `None` when niri does not report the current mode.
    pub mode: Option<ModeSpec>,
    pub scale: f64,
    pub transform: Transform,
}

impl LiveOutputSettings {
    /// What niri reports for `output`; `None` while it is off.
    pub fn current(output: &NiriOutput) -> Option<Self> {
        let logical = output.logical.as_ref()?;
        Some(Self {
            position: Position {
                x: logical.x,
                y: logical.y,
            },
            mode: output.active_mode().map(|mode| ModeSpec::from(*mode)),
            scale: logical.scale,
            transform: logical.transform,
        })
    }
}

impl ManualLayoutDraft {
    pub fn sync_from_outputs(&mut self, outputs: &[NiriOutput]) {
        let rects = derive_connected_output_rects(outputs);
        let output_keys = rects
            .iter()
            .filter_map(|rect| outputs.get(rect.output_index))
            .map(|output| output.connector.clone())
            .collect::<Vec<_>>();
        let current = rects
            .iter()
            .filter_map(|rect| {
                outputs
                    .get(rect.output_index)
                    .and_then(LiveOutputSettings::current)
            })
            .collect::<Vec<_>>();

        if self.output_keys != output_keys || !self.has_changes() || self.draft == current {
            self.output_keys = output_keys;
            self.current = current.clone();
            self.draft = current;
        } else {
            self.current = current;
        }
    }

    pub fn has_changes(&self) -> bool {
        self.current != self.draft
    }

    pub fn reset(&mut self) {
        self.draft = self.current.clone();
    }

    /// Outputs whose normalized draft position, mode, scale, or rotation differs from
    /// niri's. Only the settings that differ are set.
    pub fn changed_placements(&self, outputs: &[NiriOutput]) -> Vec<ManualOutputPlacement> {
        let mut rects = self.draft_rects(outputs);
        normalize_output_rect_positions(&mut rects);

        rects
            .iter()
            .zip(&self.draft)
            .filter_map(|(rect, draft)| {
                let output = outputs.get(rect.output_index)?;
                let current = LiveOutputSettings::current(output)?;
                let position = Position {
                    x: rect.x,
                    y: rect.y,
                };
                let mode = draft.mode.filter(|mode| {
                    !output
                        .active_mode()
                        .is_some_and(|active| active.matches(*mode))
                });
                let scale = Some(draft.scale)
                    .filter(|scale| (scale - current.scale).abs() >= SCALE_TOLERANCE);
                let transform =
                    Some(draft.transform).filter(|&transform| transform != current.transform);
                (position != current.position
                    || mode.is_some()
                    || scale.is_some()
                    || transform.is_some())
                .then(|| ManualOutputPlacement {
                    output: output.connector.clone(),
                    position,
                    mode,
                    scale,
                    transform,
                })
            })
            .collect()
    }

    pub fn draft_rects(&self, outputs: &[NiriOutput]) -> Vec<OutputRect> {
        let mut rects = derive_connected_output_rects(outputs);
        for (rect, draft) in rects.iter_mut().zip(&self.draft) {
            rect.x = draft.position.x;
            rect.y = draft.position.y;
            if let Some((width, height)) = outputs.get(rect.output_index).and_then(|output| {
                size_after(output, draft.mode, Some(draft.scale), Some(draft.transform))
            }) {
                rect.width = width;
                rect.height = height;
            }
        }
        rects
    }

    pub fn first_overlap_message(&self, outputs: &[NiriOutput]) -> Option<String> {
        placement_first_overlap_message(&self.draft_rects(outputs))
    }

    /// The draft settings of `output_index`; `None` while it is off.
    pub fn settings(
        &self,
        outputs: &[NiriOutput],
        output_index: usize,
    ) -> Option<LiveOutputSettings> {
        self.draft.get(draft_index(outputs, output_index)?).copied()
    }

    /// Sets the draft settings of `output_index`. Returns whether they changed.
    pub fn set_settings(
        &mut self,
        outputs: &[NiriOutput],
        output_index: usize,
        settings: LiveOutputSettings,
    ) -> bool {
        match draft_index(outputs, output_index).and_then(|index| self.draft.get_mut(index)) {
            Some(draft) if *draft != settings => {
                *draft = settings;
                true
            }
            _ => false,
        }
    }

    /// Sets the draft position of `output_index`. Returns whether it changed.
    pub fn move_output(
        &mut self,
        outputs: &[NiriOutput],
        output_index: usize,
        position: Position,
    ) -> bool {
        self.settings(outputs, output_index)
            .is_some_and(|settings| {
                self.set_settings(
                    outputs,
                    output_index,
                    LiveOutputSettings {
                        position,
                        ..settings
                    },
                )
            })
    }

    /// Moves the draft position of `output_index` by `(dx, dy)`.
    pub fn nudge(
        &mut self,
        outputs: &[NiriOutput],
        output_index: usize,
        (dx, dy): (i32, i32),
    ) -> bool {
        let Some(rect) = self
            .draft_rects(outputs)
            .into_iter()
            .find(|rect| rect.output_index == output_index)
        else {
            return false;
        };
        self.move_output(
            outputs,
            output_index,
            Position {
                x: rect.x + dx,
                y: rect.y + dy,
            },
        )
    }
}

/// Index in the draft of the enabled output `output_index`.
fn draft_index(outputs: &[NiriOutput], output_index: usize) -> Option<usize> {
    derive_connected_output_rects(outputs)
        .iter()
        .position(|rect| rect.output_index == output_index)
}

/// Moves a profile output by `(dx, dy)` from where the canvas draws it.
pub fn nudge_profile_output(
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
    output_index: usize,
    (dx, dy): (i32, i32),
) -> bool {
    let Some(rect) = derive_output_rects(profile, connected_outputs)
        .into_iter()
        .find(|rect| rect.output_index == output_index)
    else {
        return false;
    };
    let Some(output) = profile.outputs.get_mut(output_index) else {
        return false;
    };
    output.position = Some(Position {
        x: rect.x + dx,
        y: rect.y + dy,
    });
    true
}

/// An output as the canvas draws it.
struct Tile {
    rect: OutputRect,
    title: String,
    subtitle: String,
    detail: String,
    /// Off in the profile, or not connected.
    off: bool,
}

pub fn show_monitor_canvas(
    ui: &mut egui::Ui,
    profile: &mut Profile,
    connected_outputs: &[NiriOutput],
    state: &mut CanvasState,
) -> bool {
    let tiles = profile_tiles(profile, connected_outputs);
    if state
        .selected_output
        .is_some_and(|index| index >= profile.outputs.len())
    {
        state.selected_output = None;
    }

    if tiles.is_empty() {
        show_empty_canvas(
            ui,
            "This profile has no outputs",
            "Create a new profile to capture the monitors connected now.",
        );
        return false;
    }

    let Some((output_index, position)) = show_interactive_canvas(ui, &tiles, state) else {
        return false;
    };
    let Some(output) = profile.outputs.get_mut(output_index) else {
        return false;
    };
    output.position = Some(position);
    true
}

pub fn show_editable_live_monitor_canvas(
    ui: &mut egui::Ui,
    outputs: &[NiriOutput],
    draft: &mut ManualLayoutDraft,
    state: &mut CanvasState,
    empty_message: &str,
) -> bool {
    draft.sync_from_outputs(outputs);
    if state
        .selected_output
        .is_some_and(|selected| selected >= outputs.len())
    {
        state.selected_output = None;
    }

    let tiles = live_tiles(draft, outputs);
    if tiles.is_empty() {
        show_empty_canvas(ui, "No monitors to arrange", empty_message);
        return false;
    }

    let Some((output_index, position)) = show_interactive_canvas(ui, &tiles, state) else {
        return false;
    };
    draft.move_output(outputs, output_index, position)
}

fn profile_tiles(profile: &Profile, connected_outputs: &[NiriOutput]) -> Vec<Tile> {
    derive_output_rects(profile, connected_outputs)
        .into_iter()
        .map(|rect| {
            let profile_output = &profile.outputs[rect.output_index];
            let connected = connected_outputs
                .iter()
                .find(|output| matcher_matches_output(&profile_output.matcher, output));
            let scale = profile_output.scale.or_else(|| {
                connected
                    .and_then(|output| output.logical.as_ref())
                    .map(|logical| logical.scale)
            });
            let refresh = profile_output
                .mode
                .and_then(|mode| mode.refresh_millihz)
                .or_else(|| {
                    connected
                        .and_then(NiriOutput::active_mode)
                        .map(|mode| mode.refresh_millihz)
                });
            let subtitle = if !rect.enabled {
                "Off".to_owned()
            } else {
                connected.map_or_else(|| "Not connected".to_owned(), NiriOutput::display_name)
            };
            Tile {
                title: rect.label.lines().next().unwrap_or_default().to_owned(),
                subtitle,
                detail: tile_detail(rect.width, rect.height, scale, refresh),
                off: !rect.enabled || !rect.connected,
                rect,
            }
        })
        .collect()
}

fn live_tiles(draft: &ManualLayoutDraft, outputs: &[NiriOutput]) -> Vec<Tile> {
    draft
        .draft_rects(outputs)
        .into_iter()
        .filter_map(|rect| {
            let output = outputs.get(rect.output_index)?;
            let settings = draft.settings(outputs, rect.output_index);
            Some(Tile {
                title: output.connector.clone(),
                subtitle: output.display_name(),
                detail: tile_detail(
                    rect.width,
                    rect.height,
                    settings.map(|settings| settings.scale),
                    settings
                        .and_then(|settings| settings.mode)
                        .and_then(|mode| mode.refresh_millihz),
                ),
                off: false,
                rect,
            })
        })
        .collect()
}

/// Logical size, scale, and refresh rate, for example `2560×1707 · 1.5× · 60 Hz`.
fn tile_detail(
    width: u32,
    height: u32,
    scale: Option<f64>,
    refresh_millihz: Option<u32>,
) -> String {
    let mut parts = vec![format!("{width}×{height}")];
    parts.extend(scale.map(format_scale));
    parts.extend(
        refresh_millihz.map(|millihz| format!("{} Hz", (f64::from(millihz) / 1000.0).round())),
    );
    parts.join(" · ")
}

/// Draws the tiles and lets the user select one and drag it with snapping. Returns the
/// dragged output and its snapped position on frames where a drag moves it.
fn show_interactive_canvas(
    ui: &mut egui::Ui,
    tiles: &[Tile],
    state: &mut CanvasState,
) -> Option<(usize, Position)> {
    let rects: Vec<OutputRect> = tiles.iter().map(|tile| tile.rect.clone()).collect();
    let desired_size = ui.available_size().max(Vec2::splat(200.0));
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

    let mut moved = None;
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
        moved = Some((dragging.output_index, snapped.position));
    }

    if response.drag_stopped() {
        state.dragging = None;
        state.snap_lines.clear();
        state.frozen_view = None;
    }

    draw_canvas(ui, canvas_rect, view, tiles, &rects, state);
    moved
}

fn show_empty_canvas(ui: &mut egui::Ui, title: &str, body: &str) {
    let palette = theme::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    paint_background(&painter, rect, palette);

    let title = painter.layout_no_wrap(title.to_owned(), FontId::proportional(18.0), palette.text);
    let body = painter.layout(
        body.to_owned(),
        FontId::proportional(14.0),
        palette.text_weak,
        (rect.width() - 48.0).clamp(120.0, 440.0),
    );
    let top = rect.center().y - (title.size().y + 8.0 + body.size().y) / 2.0;
    let title_height = title.size().y;
    painter.galley(
        Pos2::new(rect.center().x - title.size().x / 2.0, top),
        title,
        palette.text,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - body.size().x / 2.0,
            top + title_height + 8.0,
        ),
        body,
        palette.text_weak,
    );
}

fn draw_canvas(
    ui: &egui::Ui,
    canvas_rect: Rect,
    view: ViewBox,
    tiles: &[Tile],
    rects: &[OutputRect],
    state: &CanvasState,
) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter_at(canvas_rect);
    paint_background(&painter, canvas_rect, palette);

    let snap_stroke = Stroke::new(1.5_f32, palette.accent);
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
                    snap_stroke,
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
                    snap_stroke,
                );
            }
        }
    }

    let overlaps = overlapping_output_pairs(rects);
    for tile in tiles {
        let index = tile.rect.output_index;
        let overlapping = overlaps.iter().any(|overlap| {
            overlap.first_output_index == index || overlap.second_output_index == index
        });
        paint_tile(
            &painter,
            palette,
            tile,
            output_screen_rect(&tile.rect, view, canvas_rect),
            state.selected_output == Some(index),
            overlapping,
        );
    }
    paint_origin(
        &painter,
        palette,
        logical_to_screen(Pos2::ZERO, view, canvas_rect),
    );
}

/// Canvas fill with a faint dot grid.
fn paint_background(painter: &egui::Painter, rect: Rect, palette: &Palette) {
    painter.rect_filled(rect, 0.0, palette.canvas);
    let mut y = rect.top() + GRID_SPACING / 2.0;
    while y < rect.bottom() {
        let mut x = rect.left() + GRID_SPACING / 2.0;
        while x < rect.right() {
            painter.circle_filled(Pos2::new(x, y), 1.0, palette.grid);
            x += GRID_SPACING;
        }
        y += GRID_SPACING;
    }
}

/// Small crosshair at logical 0,0, where niri's layout starts.
fn paint_origin(painter: &egui::Painter, palette: &Palette, origin: Pos2) {
    let stroke = Stroke::new(1.0_f32, palette.text_weak);
    painter.line_segment([origin - Vec2::X * 6.0, origin + Vec2::X * 6.0], stroke);
    painter.line_segment([origin - Vec2::Y * 6.0, origin + Vec2::Y * 6.0], stroke);
    painter.text(
        origin + Vec2::new(4.0, -4.0),
        Align2::LEFT_BOTTOM,
        "0, 0",
        FontId::proportional(11.0),
        palette.text_weak,
    );
}

fn paint_tile(
    painter: &egui::Painter,
    palette: &Palette,
    tile: &Tile,
    rect: Rect,
    selected: bool,
    overlapping: bool,
) {
    let radius = CornerRadius::same(TILE_RADIUS);
    if selected {
        painter.add(
            Shadow {
                offset: [0, 6],
                blur: 18,
                spread: 0,
                color: palette.shadow,
            }
            .as_shape(rect, radius),
        );
    }

    let fill = if overlapping {
        palette.tile.lerp_to_gamma(palette.error, 0.22)
    } else if tile.off {
        palette.tile_off
    } else {
        palette.tile
    };
    painter.rect_filled(rect, radius, fill);

    // Selection keeps its outline when the tile also overlaps; the fill shows the overlap.
    let stroke = if selected {
        Stroke::new(2.0_f32, palette.accent)
    } else if overlapping {
        Stroke::new(1.5_f32, palette.error)
    } else {
        Stroke::new(1.0_f32, palette.tile_border)
    };
    if tile.off && !selected && !overlapping {
        paint_dashed_rect(painter, rect.shrink(0.5), stroke);
    } else {
        painter.rect_stroke(rect, radius, stroke, StrokeKind::Inside);
    }
    paint_tile_labels(painter, palette, tile, rect);
}

fn paint_tile_labels(painter: &egui::Painter, palette: &Palette, tile: &Tile, rect: Rect) {
    let painter = painter.with_clip_rect(rect.shrink(6.0));
    let title_size = (rect.width().min(rect.height()) * 0.12).clamp(13.0, 26.0);
    let small_size = (title_size * 0.55).clamp(11.0, 14.0);
    let line_gap = 4.0;
    let lines = if rect.height() > title_size + 2.0 * (small_size + line_gap) + 16.0 {
        3
    } else if rect.height() > title_size + small_size + line_gap + 12.0 {
        2
    } else {
        1
    };

    let text_color = if tile.off {
        palette.text_weak
    } else {
        palette.text
    };
    let block_height = title_size + (lines - 1) as f32 * (small_size + line_gap);
    let mut y = rect.center().y - block_height / 2.0 + title_size / 2.0;
    painter.text(
        Pos2::new(rect.center().x, y),
        Align2::CENTER_CENTER,
        &tile.title,
        FontId::proportional(title_size),
        text_color,
    );
    y += title_size / 2.0 + line_gap + small_size / 2.0;
    for text in [&tile.subtitle, &tile.detail].into_iter().take(lines - 1) {
        painter.text(
            Pos2::new(rect.center().x, y),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(small_size),
            palette.text_weak,
        );
        y += small_size + line_gap;
    }
}

fn paint_dashed_rect(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    for [start, end] in [
        [rect.left_top(), rect.right_top()],
        [rect.right_top(), rect.right_bottom()],
        [rect.right_bottom(), rect.left_bottom()],
        [rect.left_bottom(), rect.left_top()],
    ] {
        paint_dashed_line(painter, start, end, stroke);
    }
}

fn paint_dashed_line(painter: &egui::Painter, start: Pos2, end: Pos2, stroke: Stroke) {
    let delta = end - start;
    let length = delta.length();
    if length <= 0.0 {
        return;
    }

    let direction = delta / length;
    let dash = 8.0;
    let gap = 6.0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MonitorMatcher, ProfileCondition, ProfileOutput, Transform};
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn tile_detail_lists_size_scale_and_rounded_refresh() {
        assert_eq!(
            tile_detail(2560, 1707, Some(1.5), Some(59_984)),
            "2560×1707 · 1.5× · 60 Hz"
        );
        assert_eq!(tile_detail(1920, 1080, None, None), "1920×1080");
    }

    #[test]
    fn nudging_a_profile_output_starts_from_where_it_is_drawn() {
        let mut profile = Profile {
            id: "home".to_string(),
            name: "Home".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![ProfileOutput {
                matcher: MonitorMatcher {
                    connector: Some("DP-1".to_string()),
                    ..MonitorMatcher::default()
                },
                enabled: None,
                mode: None,
                scale: None,
                transform: None,
                position: None,
                vrr: None,
            }],
        };

        assert!(nudge_profile_output(
            &mut profile,
            &[output("DP-1", 100, 50)],
            0,
            (10, -1)
        ));
        assert_eq!(
            profile.outputs[0].position,
            Some(Position { x: 110, y: 49 })
        );
    }

    #[test]
    fn nudging_the_draft_moves_only_that_output() {
        let outputs = [output("DP-1", 0, 0), output("DP-2", 4_000, 0)];
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&outputs);

        assert!(draft.nudge(&outputs, 1, (-10, 0)));

        let rects = draft.draft_rects(&outputs);
        assert_eq!((rects[0].x, rects[1].x), (0, 3_990));
    }

    #[test]
    fn manual_layout_draft_tracks_changed_positions() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0)]);
        assert!(!draft.has_changes());

        draft.draft[0].position = Position { x: 120, y: 80 };

        assert!(draft.has_changes());
        let rects = draft.draft_rects(&[output("DP-1", 0, 0)]);
        assert_eq!((rects[0].x, rects[0].y), (120, 80));
    }

    #[test]
    fn manual_layout_draft_resets_when_live_position_matches_draft() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0)]);
        draft.draft[0].position = Position { x: 120, y: 80 };

        draft.sync_from_outputs(&[output("DP-1", 120, 80)]);

        assert!(!draft.has_changes());
    }

    #[test]
    fn manual_layout_draft_reports_overlap_message() {
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&[output("DP-1", 0, 0), output("DP-2", 4_000, 0)]);
        draft.draft[1].position = Position { x: 10, y: 10 };

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
        draft.draft[0].position = Position { x: -100, y: -50 };
        draft.draft[1].position = Position { x: 3_340, y: -50 };

        assert_eq!(
            draft.changed_placements(&outputs),
            vec![ManualOutputPlacement {
                output: "DP-2".to_string(),
                position: Position { x: 3_440, y: 0 },
                mode: None,
                scale: None,
                transform: None,
            }]
        );
    }

    #[test]
    fn changing_the_draft_mode_resizes_the_output_and_is_applied() {
        let outputs = [two_mode_output(0), output("DP-2", 3_440, 0)];
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&outputs);
        let current = draft.settings(&outputs, 0).unwrap();
        let smaller = LiveOutputSettings {
            mode: Some("2560x1440@59.951".parse().unwrap()),
            ..current
        };

        assert!(draft.set_settings(&outputs, 0, smaller));
        assert!(!draft.set_settings(&outputs, 0, smaller));

        assert_eq!(draft.draft_rects(&outputs)[0].width, 2560);
        assert_eq!(
            draft.changed_placements(&outputs),
            vec![ManualOutputPlacement {
                output: "DP-1".to_string(),
                position: Position { x: 0, y: 0 },
                mode: smaller.mode,
                scale: None,
                transform: None,
            }]
        );

        draft.reset();
        assert!(draft.changed_placements(&outputs).is_empty());
        assert_eq!(draft.settings(&outputs, 0), Some(current));
    }

    #[test]
    fn changing_the_draft_scale_and_rotation_resizes_the_output() {
        let outputs = [output("DP-1", 0, 0)];
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&outputs);
        let current = draft.settings(&outputs, 0).unwrap();

        draft.set_settings(
            &outputs,
            0,
            LiveOutputSettings {
                scale: 2.0,
                transform: Transform::Rotate90,
                ..current
            },
        );

        let rects = draft.draft_rects(&outputs);
        assert_eq!((rects[0].width, rects[0].height), (720, 1720));
        assert_eq!(
            draft.changed_placements(&outputs),
            vec![ManualOutputPlacement {
                output: "DP-1".to_string(),
                position: Position { x: 0, y: 0 },
                mode: None,
                scale: Some(2.0),
                transform: Some(Transform::Rotate90),
            }]
        );
    }

    #[test]
    fn scale_within_niri_rounding_is_not_a_change() {
        let outputs = [output("DP-1", 0, 0)];
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&outputs);
        let current = draft.settings(&outputs, 0).unwrap();

        draft.set_settings(
            &outputs,
            0,
            LiveOutputSettings {
                scale: 1.001,
                ..current
            },
        );

        assert!(draft.changed_placements(&outputs).is_empty());
    }

    #[test]
    fn manual_layout_draft_resets_when_live_mode_matches_draft() {
        let before = [two_mode_output(0)];
        let mut draft = ManualLayoutDraft::default();
        draft.sync_from_outputs(&before);
        let current = draft.settings(&before, 0).unwrap();
        draft.set_settings(
            &before,
            0,
            LiveOutputSettings {
                mode: Some("2560x1440@59.951".parse().unwrap()),
                ..current
            },
        );

        let mut after = two_mode_output(0);
        after.current_mode = Some(1);
        after.logical.as_mut().unwrap().width = 2560;
        draft.sync_from_outputs(&[after]);

        assert!(!draft.has_changes());
    }

    fn two_mode_output(x: i32) -> NiriOutput {
        let mut output = output("DP-1", x, 0);
        output.modes.push(NiriMode {
            width: 2560,
            height: 1440,
            refresh_millihz: 59951,
            is_preferred: false,
        });
        output
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
                transform: Transform::Normal,
            }),
            vrr_supported: true,
            vrr_enabled: false,
        }
    }
}
