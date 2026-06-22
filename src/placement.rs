use crate::matching::matcher_matches_output;
use crate::model::{Position, Profile, ProfileOutput};
use crate::niri::output::NiriOutput;

const FALLBACK_WIDTH: u32 = 1920;
const FALLBACK_HEIGHT: u32 = 1080;

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
            Some(OutputRect {
                output_index,
                x: logical.x,
                y: logical.y,
                width: logical.width,
                height: logical.height,
                enabled: true,
                connected: true,
                label: output_label(output),
            })
        })
        .collect()
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
    if let Some(logical) =
        connected_output.and_then(|connected_output| connected_output.logical.as_ref())
    {
        return (logical.width, logical.height);
    }

    let scale = output.scale.filter(|scale| *scale > 0.0).unwrap_or(1.0);
    if let Some((width, height)) = connected_output
        .and_then(|connected_output| {
            connected_output
                .current_mode
                .and_then(|index| connected_output.modes.get(index))
        })
        .map(|mode| (u32::from(mode.width), u32::from(mode.height)))
        .or_else(|| output.mode.as_deref().and_then(parse_mode_size))
    {
        return scale_size(width, height, scale);
    }

    (FALLBACK_WIDTH, FALLBACK_HEIGHT)
}

fn parse_mode_size(mode: &str) -> Option<(u32, u32)> {
    let size = mode.split_once('@').map(|(size, _)| size).unwrap_or(mode);
    let (width, height) = size.split_once('x')?;
    Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
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
            mode: Some("3000x2000@60".to_owned()),
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
            id: "profile".to_owned(),
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
                transform: "normal".to_owned(),
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
