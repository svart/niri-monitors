use crate::matching::matcher_matches_output;
use crate::model::{ModeSpec, Position, Profile, ProfileOutput, Transform};
use crate::niri::output::NiriOutput;

const FALLBACK_WIDTH: u32 = 1920;
const FALLBACK_HEIGHT: u32 = 1080;

// niri may round a fractional scale to a nearby multiple of 1/120, so the reported
// scale can differ slightly from the configured one. Within half a step counts as applied.
pub const SCALE_TOLERANCE: f64 = 1.0 / 240.0;

#[derive(Debug, Clone, PartialEq)]
pub struct OutputRect {
    pub output_index: usize,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
    pub connected: bool,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapPair {
    pub first_output_index: usize,
    pub second_output_index: usize,
    pub first_label: String,
    pub second_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapLine {
    pub axis: SnapAxis,
    pub position: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapResult {
    pub position: Position,
    pub snap_lines: Vec<SnapLine>,
}

pub fn derive_output_rects(profile: &Profile, connected_outputs: &[NiriOutput]) -> Vec<OutputRect> {
    profile
        .outputs
        .iter()
        .enumerate()
        .map(|(output_index, output)| derive_output_rect(output_index, output, connected_outputs))
        .collect()
}

pub fn derive_connected_output_rects(outputs: &[NiriOutput]) -> Vec<OutputRect> {
    outputs
        .iter()
        .enumerate()
        .filter_map(|(output_index, output)| {
            let logical = output.logical.as_ref()?;
            Some(placed_output_rect(
                output_index,
                output,
                Position {
                    x: logical.x,
                    y: logical.y,
                },
                (logical.width, logical.height),
            ))
        })
        .collect()
}

/// Rectangle for the enabled, connected `output` at `position` with logical `size`.
pub fn placed_output_rect(
    output_index: usize,
    output: &NiriOutput,
    position: Position,
    (width, height): (u32, u32),
) -> OutputRect {
    OutputRect {
        output_index,
        x: position.x,
        y: position.y,
        width,
        height,
        enabled: true,
        connected: true,
        label: output_label(output),
    }
}

/// Logical size `output` has once `profile_output` is applied. `None` when the mode or the
/// scale is unknown, which happens for an output that is off and whose profile entry does
/// not set both.
pub fn configured_size(output: &NiriOutput, profile_output: &ProfileOutput) -> Option<(u32, u32)> {
    size_after(
        output,
        profile_output.mode,
        profile_output.scale,
        profile_output.transform,
    )
}

/// Logical size `output` has after `mode`, `scale`, and `transform` are applied. A `None`
/// setting keeps its current value, and when nothing that affects the size changes, the
/// size niri reports is used as is. `None` when the mode or the scale is unknown.
pub fn size_after(
    output: &NiriOutput,
    mode: Option<ModeSpec>,
    scale: Option<f64>,
    transform: Option<Transform>,
) -> Option<(u32, u32)> {
    let current = output.logical.as_ref();
    let current_mode = output.active_mode();

    if let Some(logical) = current
        && mode.is_none_or(|mode| current_mode.is_some_and(|current| current.matches(mode)))
        && scale.is_none_or(|scale| (logical.scale - scale).abs() < SCALE_TOLERANCE)
        && transform.is_none_or(|transform| logical.transform == transform)
    {
        return Some((logical.width, logical.height));
    }

    let (width, height) = match mode {
        Some(mode) => (mode.width, mode.height),
        None => current_mode.map(|mode| (mode.width, mode.height))?,
    };
    let scale = scale.or(current.map(|logical| logical.scale))?;
    let transform = transform
        .or(current.map(|logical| logical.transform))
        .unwrap_or(Transform::Normal);
    Some(logical_size(
        u32::from(width),
        u32::from(height),
        scale,
        transform,
    ))
}

/// Logical size of an output showing a `width`x`height` mode at `scale` and `transform`.
pub fn logical_size(width: u32, height: u32, scale: f64, transform: Transform) -> (u32, u32) {
    if transform.swaps_axes() {
        scale_size(height, width, scale)
    } else {
        scale_size(width, height, scale)
    }
}

pub fn snap_position(
    moving: &OutputRect,
    proposed: Position,
    other_rects: &[OutputRect],
    threshold: i32,
) -> SnapResult {
    let horizontal = best_snap_delta(
        proposed.x,
        moving.width,
        other_rects.iter().map(|rect| (rect.x, rect.width)),
        threshold,
    );
    let vertical = best_snap_delta(
        proposed.y,
        moving.height,
        other_rects.iter().map(|rect| (rect.y, rect.height)),
        threshold,
    );

    let mut snap_lines = Vec::new();
    if let Some(snap) = horizontal {
        snap_lines.push(SnapLine {
            axis: SnapAxis::Vertical,
            position: snap.target,
        });
    }
    if let Some(snap) = vertical {
        snap_lines.push(SnapLine {
            axis: SnapAxis::Horizontal,
            position: snap.target,
        });
    }

    SnapResult {
        position: Position {
            x: (f64::from(proposed.x) + horizontal.map(|snap| snap.delta).unwrap_or(0.0)).round()
                as i32,
            y: (f64::from(proposed.y) + vertical.map(|snap| snap.delta).unwrap_or(0.0)).round()
                as i32,
        },
        snap_lines,
    }
}

pub fn overlapping_output_pairs(rects: &[OutputRect]) -> Vec<OverlapPair> {
    let mut overlaps = Vec::new();

    for (first_index, first) in rects.iter().enumerate() {
        if !first.enabled {
            continue;
        }

        for second in rects
            .iter()
            .skip(first_index + 1)
            .filter(|rect| rect.enabled)
        {
            if rects_overlap(first, second) {
                overlaps.push(OverlapPair {
                    first_output_index: first.output_index,
                    second_output_index: second.output_index,
                    first_label: short_output_label(first).to_owned(),
                    second_label: short_output_label(second).to_owned(),
                });
            }
        }
    }

    overlaps
}

pub fn first_overlap_message(rects: &[OutputRect]) -> Option<String> {
    overlapping_output_pairs(rects)
        .first()
        .map(|overlap| format!("{} overlaps {}", overlap.first_label, overlap.second_label))
}

pub fn normalize_output_rect_positions(rects: &mut [OutputRect]) {
    let Some(min_x) = rects
        .iter()
        .filter(|rect| rect.enabled)
        .map(|rect| rect.x)
        .min()
    else {
        return;
    };
    let min_y = rects
        .iter()
        .filter(|rect| rect.enabled)
        .map(|rect| rect.y)
        .min()
        .unwrap_or(0);

    for rect in rects.iter_mut().filter(|rect| rect.enabled) {
        rect.x -= min_x;
        rect.y -= min_y;
    }
}

fn derive_output_rect(
    output_index: usize,
    output: &ProfileOutput,
    connected_outputs: &[NiriOutput],
) -> OutputRect {
    let connected_output = connected_outputs
        .iter()
        .find(|connected_output| matcher_matches_output(&output.matcher, connected_output));
    let (width, height) = output_size(output, connected_output);
    let position = output.position.or_else(|| {
        connected_output
            .and_then(|connected_output| connected_output.logical.as_ref())
            .map(|logical| Position {
                x: logical.x,
                y: logical.y,
            })
    });

    OutputRect {
        output_index,
        x: position.map(|position| position.x).unwrap_or(0),
        y: position.map(|position| position.y).unwrap_or(0),
        width,
        height,
        enabled: output.enabled.unwrap_or(true),
        connected: connected_output.is_some(),
        label: connected_output.map_or_else(
            || matcher_label(&output.matcher, output_index),
            output_label,
        ),
    }
}

fn output_label(output: &NiriOutput) -> String {
    format!("{}\n{}", output.connector, output.description)
}

fn short_output_label(rect: &OutputRect) -> &str {
    rect.label.lines().next().unwrap_or(rect.label.as_str())
}

fn rects_overlap(first: &OutputRect, second: &OutputRect) -> bool {
    let first_left = i64::from(first.x);
    let first_top = i64::from(first.y);
    let first_right = first_left + i64::from(first.width);
    let first_bottom = first_top + i64::from(first.height);

    let second_left = i64::from(second.x);
    let second_top = i64::from(second.y);
    let second_right = second_left + i64::from(second.width);
    let second_bottom = second_top + i64::from(second.height);

    first_left < second_right
        && second_left < first_right
        && first_top < second_bottom
        && second_top < first_bottom
}

fn matcher_label(matcher: &crate::model::MonitorMatcher, output_index: usize) -> String {
    match (&matcher.connector, &matcher.description) {
        (Some(connector), Some(description)) => format!("{connector}\n{description}"),
        (Some(connector), None) => connector.clone(),
        (None, Some(description)) => description.clone(),
        (None, None) => matcher
            .make
            .clone()
            .or_else(|| matcher.model.clone())
            .or_else(|| matcher.serial.clone())
            .unwrap_or_else(|| format!("Output {}", output_index + 1)),
    }
}

fn output_size(output: &ProfileOutput, connected_output: Option<&NiriOutput>) -> (u32, u32) {
    if let Some(size) =
        connected_output.and_then(|connected_output| configured_size(connected_output, output))
    {
        return size;
    }

    // Off or not connected: estimate from the profile and the output's preferred mode.
    let mode = output
        .mode
        .map(|mode| (mode.width, mode.height))
        .or_else(|| {
            connected_output
                .and_then(|connected_output| {
                    connected_output.modes.iter().find(|mode| mode.is_preferred)
                })
                .map(|mode| (mode.width, mode.height))
        });
    match mode {
        Some((width, height)) => logical_size(
            u32::from(width),
            u32::from(height),
            output.scale.filter(|scale| *scale > 0.0).unwrap_or(1.0),
            output.transform.unwrap_or(Transform::Normal),
        ),
        None => (FALLBACK_WIDTH, FALLBACK_HEIGHT),
    }
}

fn scale_size(width: u32, height: u32, scale: f64) -> (u32, u32) {
    (
        (f64::from(width) / scale).round().max(1.0) as u32,
        (f64::from(height) / scale).round().max(1.0) as u32,
    )
}

#[derive(Debug, Clone, Copy)]
struct SnapDelta {
    delta: f64,
    target: f64,
}

fn best_snap_delta(
    proposed_start: i32,
    moving_size: u32,
    other_rects: impl Iterator<Item = (i32, u32)>,
    threshold: i32,
) -> Option<SnapDelta> {
    let moving_points = rect_points(f64::from(proposed_start), f64::from(moving_size));
    let mut best: Option<SnapDelta> = None;

    for (other_start, other_size) in other_rects {
        for target in rect_points(f64::from(other_start), f64::from(other_size)) {
            for moving_point in moving_points {
                let delta = target - moving_point;
                if delta.abs() <= f64::from(threshold)
                    && best.is_none_or(|best| delta.abs() < best.delta.abs())
                {
                    best = Some(SnapDelta { delta, target });
                }
            }
        }
    }

    best
}

fn rect_points(start: f64, size: f64) -> [f64; 3] {
    [start, start + (size / 2.0), start + size]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MonitorMatcher, ProfileCondition};
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn derives_rect_from_connected_logical_size_and_profile_position() {
        let profile = profile_with_output(ProfileOutput {
            matcher: matcher("Dell Display"),
            enabled: Some(true),
            mode: None,
            scale: None,
            transform: None,
            position: Some(Position { x: 20, y: 30 }),
            vrr: None,
        });

        let rects = derive_output_rects(&profile, &[connected_output("Dell Display", 0, 0)]);

        assert_eq!(rects[0].x, 20);
        assert_eq!(rects[0].y, 30);
        assert_eq!(rects[0].width, 2560);
        assert_eq!(rects[0].height, 1440);
        assert!(rects[0].connected);
    }

    #[test]
    fn derives_rect_from_profile_scale_when_it_differs_from_live() {
        let profile = profile_with_output(ProfileOutput {
            matcher: matcher("Dell Display"),
            enabled: Some(true),
            mode: None,
            scale: Some(2.0),
            transform: Some(Transform::Rotate90),
            position: None,
            vrr: None,
        });

        let rects = derive_output_rects(&profile, &[connected_output("Dell Display", 0, 0)]);

        assert_eq!((rects[0].width, rects[0].height), (1080, 1920));
    }

    #[test]
    fn derives_live_rects_from_enabled_connected_outputs() {
        let mut disabled = connected_output("Disabled Display", 0, 0);
        disabled.logical = None;

        let rects =
            derive_connected_output_rects(&[connected_output("Dell Display", 20, 30), disabled]);

        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].x, 20);
        assert_eq!(rects[0].y, 30);
        assert_eq!(rects[0].width, 2560);
        assert_eq!(rects[0].height, 1440);
        assert_eq!(rects[0].label, "DP-1\nDell Display");
    }

    #[test]
    fn derives_rect_from_current_mode_and_profile_scale_when_logical_missing() {
        let profile = profile_with_output(ProfileOutput {
            matcher: matcher("Dell Display"),
            enabled: Some(true),
            mode: None,
            scale: Some(2.0),
            transform: None,
            position: None,
            vrr: None,
        });
        let mut output = connected_output("Dell Display", 0, 0);
        output.logical = None;

        let rects = derive_output_rects(&profile, &[output]);

        assert_eq!(rects[0].width, 1920);
        assert_eq!(rects[0].height, 1080);
        assert_eq!(rects[0].x, 0);
        assert_eq!(rects[0].y, 0);
    }

    #[test]
    fn derives_rect_from_profile_mode_when_output_is_disconnected() {
        let profile = profile_with_output(ProfileOutput {
            matcher: matcher("Missing Display"),
            enabled: Some(true),
            mode: Some("3000x2000@60".parse().unwrap()),
            scale: Some(1.5),
            transform: None,
            position: None,
            vrr: None,
        });

        let rects = derive_output_rects(&profile, &[]);

        assert_eq!(rects[0].width, 2000);
        assert_eq!(rects[0].height, 1333);
        assert!(!rects[0].connected);
    }

    #[test]
    fn size_after_keeps_the_settings_left_unset() {
        let mut output = connected_output("Dell Display", 0, 0);
        let mode = |text: &str| Some(text.parse().unwrap());

        assert_eq!(
            size_after(&output, mode("3840x2160@60"), None, None),
            Some((2560, 1440))
        );
        assert_eq!(
            size_after(&output, mode("1920x1080@60"), None, None),
            Some((1280, 720))
        );
        assert_eq!(
            size_after(&output, None, Some(2.0), Some(Transform::Rotate90)),
            Some((1080, 1920))
        );

        output.logical.as_mut().unwrap().transform = Transform::Rotate90;
        assert_eq!(
            size_after(&output, mode("1920x1080@60"), None, None),
            Some((720, 1280))
        );

        output.logical = None;
        assert_eq!(size_after(&output, mode("1920x1080@60"), None, None), None);
    }

    #[test]
    fn logical_size_divides_by_scale_and_swaps_rotated_outputs() {
        assert_eq!(
            logical_size(2560, 1440, 1.5, Transform::Normal),
            (1707, 960)
        );
        assert_eq!(
            logical_size(2560, 1440, 1.0, Transform::Rotate90),
            (1440, 2560)
        );
        assert_eq!(
            logical_size(2560, 1440, 2.0, Transform::Flipped270),
            (720, 1280)
        );
        assert_eq!(
            logical_size(2560, 1440, 1.0, Transform::Flipped180),
            (2560, 1440)
        );
    }

    #[test]
    fn snaps_edges_to_nearby_rects() {
        let moving = rect(0, 0, 100, 100, 0);
        let other = rect(104, 0, 100, 100, 1);

        let result = snap_position(&moving, Position { x: 3, y: 0 }, &[other], 12);

        assert_eq!(result.position.x, 4);
        assert!(
            result
                .snap_lines
                .iter()
                .any(|line| line.axis == SnapAxis::Vertical)
        );
    }

    #[test]
    fn snaps_centers_to_nearby_rects() {
        let moving = rect(0, 0, 100, 80, 0);
        let other = rect(200, 200, 200, 100, 1);

        let result = snap_position(&moving, Position { x: 250, y: 211 }, &[other], 8);

        assert_eq!(result.position.y, 210);
    }

    #[test]
    fn detects_positive_area_overlap() {
        let first = rect(0, 0, 100, 100, 0);
        let second = rect(99, 0, 100, 100, 1);

        let overlaps = overlapping_output_pairs(&[first, second]);

        assert_eq!(overlaps.len(), 1);
        assert_eq!(overlaps[0].first_label, "Output 0");
        assert_eq!(overlaps[0].second_label, "Output 1");
    }

    #[test]
    fn allows_edge_touching_rects() {
        let first = rect(0, 0, 100, 100, 0);
        let second = rect(100, 0, 100, 100, 1);

        assert!(overlapping_output_pairs(&[first, second]).is_empty());
    }

    #[test]
    fn normalizes_layout_origin_to_minimum_x_and_y() {
        let mut rects = vec![rect(-100, 50, 100, 100, 0), rect(200, -40, 100, 100, 1)];

        normalize_output_rect_positions(&mut rects);

        assert_eq!((rects[0].x, rects[0].y), (0, 90));
        assert_eq!((rects[1].x, rects[1].y), (300, 0));
    }

    fn profile_with_output(output: ProfileOutput) -> Profile {
        Profile {
            name: "Profile".to_owned(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![output],
        }
    }

    fn matcher(description: &str) -> MonitorMatcher {
        MonitorMatcher {
            description: Some(description.to_owned()),
            ..MonitorMatcher::default()
        }
    }

    fn connected_output(description: &str, x: i32, y: i32) -> NiriOutput {
        NiriOutput {
            connector: "DP-1".to_owned(),
            make: "Dell".to_owned(),
            model: "Display".to_owned(),
            serial: Some("123".to_owned()),
            description: description.to_owned(),
            modes: vec![NiriMode {
                width: 3840,
                height: 2160,
                refresh_millihz: 60000,
                is_preferred: true,
            }],
            current_mode: Some(0),
            logical: Some(LogicalOutput {
                x,
                y,
                width: 2560,
                height: 1440,
                scale: 1.5,
                transform: Transform::Normal,
            }),
            vrr_supported: false,
            vrr_enabled: false,
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32, output_index: usize) -> OutputRect {
        OutputRect {
            output_index,
            x,
            y,
            width,
            height,
            enabled: true,
            connected: true,
            label: format!("Output {output_index}"),
        }
    }
}
