use crate::matching::matcher_matches_output;
use crate::model::{Config, ManualOutputPlacement, ModeSpec, Position, Profile, ProfileOutput};
use crate::niri::ipc::{NiriClient, NiriError, OutputAction};
use crate::niri::output::NiriOutput;
use crate::placement::{
    OutputRect, SCALE_TOLERANCE, configured_size, first_overlap_message,
    normalize_output_rect_positions, placed_output_rect,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyPlan {
    pub profile_id: String,
    pub actions: Vec<PlannedOutputAction>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedOutputAction {
    pub output: String,
    pub action: OutputAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyExecutionReport {
    pub profile_id: String,
    pub applied_actions: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error)]
#[error("failed to apply action {action_index} to output {output}: {source}")]
pub struct ApplyExecutionError {
    pub action_index: usize,
    pub output: String,
    pub action: OutputAction,
    #[source]
    pub source: NiriError,
}

#[derive(Debug, Error)]
pub enum ApplyPlanError {
    #[error(
        "profile {profile_id} output {output_index} matcher is ambiguous; matched: {matches:?}"
    )]
    AmbiguousMatcher {
        profile_id: String,
        output_index: usize,
        matches: Vec<String>,
    },
    #[error("profile {profile_id} maps multiple output entries to connected output {output}")]
    DuplicateOutputTarget { profile_id: String, output: String },
    #[error(
        "profile {profile_id} output {output} has mode {mode}, which the output does not offer"
    )]
    UnavailableMode {
        profile_id: String,
        output: String,
        mode: ModeSpec,
    },
    #[error("refusing to disable every connected output for profile {0}")]
    AllOutputsDisabled(String),
    #[error("profile {profile_id} has overlapping output layout: {message}")]
    OverlappingLayout { profile_id: String, message: String },
}

#[derive(Debug, Error)]
pub enum ManualLayoutError {
    #[error("manual layout has no placements")]
    Empty,
    #[error("duplicate manual layout placement for {0}")]
    DuplicateOutput(String),
    #[error("manual layout output is not connected: {0}")]
    NotConnected(String),
    #[error("manual layout output is disabled: {0}")]
    Disabled(String),
    #[error("manual layout has overlapping outputs: {0}")]
    Overlap(String),
}

pub fn execute_apply_plan(
    client: &mut impl NiriClient,
    plan: &ApplyPlan,
) -> Result<ApplyExecutionReport, ApplyExecutionError> {
    for (action_index, planned_action) in plan.actions.iter().enumerate() {
        client
            .apply_output_action(&planned_action.output, planned_action.action.clone())
            .map_err(|source| ApplyExecutionError {
                action_index,
                output: planned_action.output.clone(),
                action: planned_action.action.clone(),
                source,
            })?;
    }

    Ok(ApplyExecutionReport {
        profile_id: plan.profile_id.clone(),
        applied_actions: plan.actions.len(),
        warnings: plan.warnings.clone(),
    })
}

pub fn dry_run_apply_plan(plan: &ApplyPlan) -> ApplyExecutionReport {
    ApplyExecutionReport {
        profile_id: plan.profile_id.clone(),
        applied_actions: 0,
        warnings: plan.warnings.clone(),
    }
}

pub fn format_dry_run(plan: &ApplyPlan) -> String {
    let mut lines = vec![format!("profile: {}", plan.profile_id)];

    if !plan.warnings.is_empty() {
        lines.push("warnings:".to_string());
        lines.extend(plan.warnings.iter().map(|warning| format!("- {warning}")));
    }

    if plan.actions.is_empty() {
        lines.push("actions: none".to_string());
    } else {
        lines.push("actions:".to_string());
        lines.extend(plan.actions.iter().enumerate().map(|(index, action)| {
            format!(
                "{}. {}: {}",
                index + 1,
                action.output,
                describe_output_action(&action.action)
            )
        }));
    }

    lines.join("\n")
}

/// Where an output that is enabled after the plan ends up. A `None` position means niri
/// places the output itself; a `None` size means it is only known once niri applies it.
#[derive(Debug, Clone, Copy)]
struct Placement {
    position: Option<Position>,
    size: Option<(u32, u32)>,
}

impl Placement {
    fn current(output: &NiriOutput) -> Option<Self> {
        let logical = output.logical.as_ref()?;
        Some(Self {
            position: Some(Position {
                x: logical.x,
                y: logical.y,
            }),
            size: Some((logical.width, logical.height)),
        })
    }
}

/// Builds the actions that move the connected outputs to `profile`.
///
/// Only settings that differ from the current output state produce actions, so a plan
/// for outputs that already match the profile is empty.
pub fn build_apply_plan(
    config: &Config,
    profile: &Profile,
    outputs: &[NiriOutput],
) -> Result<ApplyPlan, ApplyPlanError> {
    let mut warnings = Vec::new();
    let mut mentioned = vec![false; outputs.len()];
    // Placement of each connected output after the plan; `None` means disabled.
    let mut placements: Vec<Option<Placement>> = outputs.iter().map(Placement::current).collect();

    let mut enable_actions = Vec::new();
    let mut mode_actions = Vec::new();
    let mut scale_actions = Vec::new();
    let mut transform_actions = Vec::new();
    let mut vrr_actions = Vec::new();
    let mut disable_actions = Vec::new();

    for (output_index, profile_output) in profile.outputs.iter().enumerate() {
        let Some(index) = resolve_profile_output(
            profile,
            output_index,
            profile_output,
            outputs,
            &mut warnings,
        )?
        else {
            continue;
        };
        let output = &outputs[index];

        if std::mem::replace(&mut mentioned[index], true) {
            return Err(ApplyPlanError::DuplicateOutputTarget {
                profile_id: profile.id.clone(),
                output: output.connector.clone(),
            });
        }

        let output_name = output.connector.clone();
        let current = output.logical.as_ref();
        if profile_output.enabled == Some(false) {
            placements[index] = None;
            if current.is_some() {
                disable_actions.push(planned(output_name, OutputAction::Disable));
            }
            continue;
        }

        let requires_enabled = profile_output.enabled == Some(true)
            || profile_output.mode.is_some()
            || profile_output.scale.is_some()
            || profile_output.transform.is_some()
            || profile_output.position.is_some()
            || profile_output.vrr.is_some();
        if !requires_enabled {
            continue;
        }

        if current.is_none() {
            enable_actions.push(planned(output_name.clone(), OutputAction::Enable));
        }

        if let Some(mode) = profile_output.mode {
            if !output.modes.iter().any(|available| available.matches(mode)) {
                return Err(ApplyPlanError::UnavailableMode {
                    profile_id: profile.id.clone(),
                    output: output_name,
                    mode,
                });
            }
            if !output
                .active_mode()
                .is_some_and(|current_mode| current_mode.matches(mode))
            {
                mode_actions.push(planned(
                    output_name.clone(),
                    OutputAction::Mode {
                        width: mode.width,
                        height: mode.height,
                        refresh_millihz: mode.refresh_millihz,
                    },
                ));
            }
        }

        if let Some(scale) = profile_output.scale
            && !current.is_some_and(|logical| (logical.scale - scale).abs() < SCALE_TOLERANCE)
        {
            scale_actions.push(planned(output_name.clone(), OutputAction::Scale(scale)));
        }

        if let Some(transform) = profile_output.transform
            && current.is_none_or(|logical| logical.transform != transform)
        {
            transform_actions.push(planned(
                output_name.clone(),
                OutputAction::Transform(transform),
            ));
        }

        if let Some(vrr) = profile_output.vrr
            && (current.is_none() || output.vrr_enabled != vrr)
        {
            vrr_actions.push(planned(output_name, OutputAction::Vrr(vrr)));
        }

        let current_placement = Placement::current(output);
        placements[index] = Some(Placement {
            position: profile_output
                .position
                .or(current_placement.and_then(|placement| placement.position)),
            size: configured_size(output, profile_output),
        });
    }

    for (output, mentioned) in outputs.iter().zip(&mentioned) {
        if !mentioned {
            warnings.push(format!(
                "connected output {} ({}) is not mentioned by profile {}",
                output.connector, output.description, profile.id
            ));
        }
    }

    if config.daemon.prevent_disable_all
        && !outputs.is_empty()
        && placements.iter().all(Option::is_none)
    {
        return Err(ApplyPlanError::AllOutputsDisabled(profile.id.clone()));
    }

    let position_actions = plan_positions(outputs, &placements).map_err(|message| {
        ApplyPlanError::OverlappingLayout {
            profile_id: profile.id.clone(),
            message,
        }
    })?;

    let mut actions = Vec::new();
    actions.extend(enable_actions);
    actions.extend(mode_actions);
    actions.extend(scale_actions);
    actions.extend(transform_actions);
    actions.extend(vrr_actions);
    actions.extend(position_actions);
    actions.extend(disable_actions);

    Ok(ApplyPlan {
        profile_id: profile.id.clone(),
        actions,
        warnings,
    })
}

/// Builds the position actions that move enabled outputs to `placements`.
///
/// The resulting layout is normalized to start at 0,0 like a profile layout, so outputs
/// without a placement can move too. Outputs already in place get no action.
pub fn build_manual_layout_plan(
    outputs: &[NiriOutput],
    placements: &[ManualOutputPlacement],
) -> Result<ApplyPlan, ManualLayoutError> {
    if placements.is_empty() {
        return Err(ManualLayoutError::Empty);
    }

    let mut layout: Vec<Option<Placement>> = outputs.iter().map(Placement::current).collect();
    let mut placed = HashSet::new();
    for placement in placements {
        let name = &placement.output;
        if !placed.insert(name) {
            return Err(ManualLayoutError::DuplicateOutput(name.clone()));
        }
        let index = outputs
            .iter()
            .position(|output| output.connector == *name)
            .ok_or_else(|| ManualLayoutError::NotConnected(name.clone()))?;
        let current = layout[index]
            .as_mut()
            .ok_or_else(|| ManualLayoutError::Disabled(name.clone()))?;
        current.position = Some(placement.position);
    }

    Ok(ApplyPlan {
        profile_id: "manual-layout".to_owned(),
        actions: plan_positions(outputs, &layout).map_err(ManualLayoutError::Overlap)?,
        warnings: Vec::new(),
    })
}

/// Normalizes the layout the outputs have after a plan, rejects overlaps, and returns
/// the position actions needed to reach it. The error is the overlap message.
///
/// Outputs that niri places itself (enabled without a position) are left out. An output
/// with an unknown size is checked by its top-left corner only, because it covers at least
/// that point.
fn plan_positions(
    outputs: &[NiriOutput],
    placements: &[Option<Placement>],
) -> Result<Vec<PlannedOutputAction>, String> {
    let mut rects: Vec<OutputRect> = outputs
        .iter()
        .zip(placements)
        .enumerate()
        .filter_map(|(index, (output, placement))| {
            let placement = (*placement)?;
            Some(placed_output_rect(
                index,
                output,
                placement.position?,
                placement.size.unwrap_or((1, 1)),
            ))
        })
        .collect();
    normalize_output_rect_positions(&mut rects);

    if let Some(message) = first_overlap_message(&rects) {
        return Err(message);
    }

    Ok(rects
        .iter()
        .filter_map(|rect| {
            let output = &outputs[rect.output_index];
            let position = Position {
                x: rect.x,
                y: rect.y,
            };
            let unchanged = output
                .logical
                .as_ref()
                .is_some_and(|logical| logical.x == position.x && logical.y == position.y);
            (!unchanged)
                .then(|| planned(output.connector.clone(), OutputAction::Position(position)))
        })
        .collect())
}

/// Index in `outputs` of the one connected output that `profile_output` matches.
fn resolve_profile_output(
    profile: &Profile,
    output_index: usize,
    profile_output: &ProfileOutput,
    outputs: &[NiriOutput],
    warnings: &mut Vec<String>,
) -> Result<Option<usize>, ApplyPlanError> {
    let matches: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| matcher_matches_output(&profile_output.matcher, output))
        .map(|(index, _)| index)
        .collect();

    match matches.as_slice() {
        [] => {
            warnings.push(format!(
                "profile {} output {} did not match any connected output",
                profile.id, output_index
            ));
            Ok(None)
        }
        [index] => Ok(Some(*index)),
        matches => Err(ApplyPlanError::AmbiguousMatcher {
            profile_id: profile.id.clone(),
            output_index,
            matches: matches
                .iter()
                .map(|&index| outputs[index].connector.clone())
                .collect(),
        }),
    }
}

fn planned(output: String, action: OutputAction) -> PlannedOutputAction {
    PlannedOutputAction { output, action }
}

fn describe_output_action(action: &OutputAction) -> String {
    match action {
        OutputAction::Enable => "enable".to_string(),
        OutputAction::Disable => "disable".to_string(),
        &OutputAction::Mode {
            width,
            height,
            refresh_millihz,
        } => format!(
            "mode {}",
            ModeSpec {
                width,
                height,
                refresh_millihz,
            }
        ),
        OutputAction::Scale(scale) => format!("scale {scale}"),
        OutputAction::Transform(transform) => format!("transform {transform}"),
        OutputAction::Position(position) => format!("position {},{}", position.x, position.y),
        OutputAction::Vrr(true) => "vrr on".to_string(),
        OutputAction::Vrr(false) => "vrr off".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transform;
    use crate::model::{DaemonConfig, MonitorMatcher, Position, ProfileCondition};
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn builds_apply_plan_in_safe_order() {
        let config = config(true);
        let profile = Profile {
            id: "home".to_string(),
            name: "Home".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![
                ProfileOutput {
                    matcher: matcher("DP-1"),
                    enabled: Some(true),
                    mode: Some("3440x1440@59.973".parse().unwrap()),
                    scale: Some(1.25),
                    transform: Some(Transform::Normal),
                    position: Some(Position { x: 0, y: 0 }),
                    vrr: Some(true),
                },
                ProfileOutput {
                    matcher: matcher("HDMI-A-1"),
                    enabled: Some(false),
                    mode: None,
                    scale: None,
                    transform: None,
                    position: None,
                    vrr: None,
                },
                ProfileOutput {
                    matcher: matcher("DP-2"),
                    enabled: Some(true),
                    mode: None,
                    scale: None,
                    transform: None,
                    position: None,
                    vrr: None,
                },
            ],
        };
        let mut dp1 = output("DP-1", true);
        dp1.modes.push(NiriMode {
            width: 2560,
            height: 1080,
            refresh_millihz: 60000,
            is_preferred: false,
        });
        dp1.current_mode = Some(1);
        let logical = dp1.logical.as_mut().expect("DP-1 should be enabled");
        logical.x = 100;
        logical.transform = Transform::Rotate90;
        let outputs = vec![
            dp1,
            output("HDMI-A-1", true),
            output("DP-2", false),
            output("DP-3", true),
        ];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(
            plan.actions,
            vec![
                planned("DP-2".to_string(), OutputAction::Enable),
                planned(
                    "DP-1".to_string(),
                    OutputAction::Mode {
                        width: 3440,
                        height: 1440,
                        refresh_millihz: Some(59973),
                    },
                ),
                planned("DP-1".to_string(), OutputAction::Scale(1.25)),
                planned(
                    "DP-1".to_string(),
                    OutputAction::Transform(Transform::Normal),
                ),
                planned("DP-1".to_string(), OutputAction::Vrr(true)),
                planned(
                    "DP-1".to_string(),
                    OutputAction::Position(Position { x: 0, y: 0 }),
                ),
                planned("HDMI-A-1".to_string(), OutputAction::Disable),
            ]
        );
        assert_eq!(plan.warnings.len(), 1);
    }

    #[test]
    fn enabling_an_output_keeps_the_side_by_side_layout() {
        let config = config(true);
        let profile = profile_with_outputs(vec![
            ProfileOutput {
                matcher: matcher("eDP-1"),
                enabled: Some(true),
                mode: Some("3440x1440@59.973".parse().unwrap()),
                scale: Some(1.0),
                transform: None,
                position: Some(Position { x: 0, y: 0 }),
                vrr: None,
            },
            ProfileOutput {
                matcher: matcher("DP-1"),
                enabled: Some(true),
                mode: None,
                scale: None,
                transform: None,
                position: Some(Position { x: 3_440, y: 0 }),
                vrr: None,
            },
        ]);
        let mut dp1 = output("DP-1", true);
        dp1.logical.as_mut().expect("DP-1 should be enabled").x = 3_440;
        let outputs = vec![dp1, output("eDP-1", false)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(
            plan.actions,
            vec![
                planned("eDP-1".to_string(), OutputAction::Enable),
                planned(
                    "eDP-1".to_string(),
                    OutputAction::Mode {
                        width: 3440,
                        height: 1440,
                        refresh_millihz: Some(59973),
                    },
                ),
                planned("eDP-1".to_string(), OutputAction::Scale(1.0)),
                planned(
                    "eDP-1".to_string(),
                    OutputAction::Position(Position { x: 0, y: 0 }),
                ),
            ]
        );
    }

    #[test]
    fn rejects_overlap_with_an_output_being_enabled() {
        let config = config(true);
        let profile = profile_with_outputs(vec![
            ProfileOutput {
                matcher: matcher("eDP-1"),
                enabled: Some(true),
                mode: Some("3440x1440@59.973".parse().unwrap()),
                scale: Some(1.0),
                transform: None,
                position: Some(Position { x: 0, y: 0 }),
                vrr: None,
            },
            ProfileOutput {
                matcher: matcher("DP-1"),
                enabled: Some(true),
                mode: None,
                scale: None,
                transform: None,
                position: Some(Position { x: 1_000, y: 0 }),
                vrr: None,
            },
        ]);
        let outputs = vec![output("DP-1", true), output("eDP-1", false)];

        let error = build_apply_plan(&config, &profile, &outputs).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::OverlappingLayout { .. }));
    }

    #[test]
    fn checks_only_the_corner_of_an_enabled_output_with_unknown_size() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("eDP-1"),
            enabled: Some(true),
            mode: None,
            scale: None,
            transform: None,
            position: Some(Position { x: 100, y: 100 }),
            vrr: None,
        }]);
        let inside = vec![output("DP-1", true), output("eDP-1", false)];
        let mut dp1_right = output("DP-1", true);
        dp1_right
            .logical
            .as_mut()
            .expect("DP-1 should be enabled")
            .x = 101;
        let next_to = vec![dp1_right, output("eDP-1", false)];

        let error = build_apply_plan(&config, &profile, &inside)
            .expect_err("corner inside DP-1 should overlap");
        let plan = build_apply_plan(&config, &profile, &next_to)
            .expect("corner outside DP-1 should not overlap");

        assert!(matches!(error, ApplyPlanError::OverlappingLayout { .. }));
        assert!(plan.actions.contains(&planned(
            "eDP-1".to_string(),
            OutputAction::Position(Position { x: 0, y: 100 })
        )));
    }

    #[test]
    fn overlap_check_uses_the_size_after_a_scale_change() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("DP-1"),
            enabled: None,
            mode: None,
            scale: Some(1.0),
            transform: None,
            position: None,
            vrr: None,
        }]);
        let mut dp1 = output("DP-1", true);
        let logical = dp1.logical.as_mut().expect("DP-1 should be enabled");
        logical.scale = 2.0;
        logical.width = 1_720;
        logical.height = 720;
        let mut dp2 = output("DP-2", true);
        dp2.logical.as_mut().expect("DP-2 should be enabled").x = 1_720;

        let error = build_apply_plan(&config, &profile, &[dp1, dp2]).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::OverlappingLayout { .. }));
    }

    #[test]
    fn plan_is_empty_when_outputs_already_match_profile() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("DP-1"),
            enabled: Some(true),
            mode: Some("3440x1440@59.973".parse().unwrap()),
            scale: Some(1.0),
            transform: Some(Transform::Normal),
            position: Some(Position { x: 0, y: 0 }),
            vrr: Some(false),
        }]);
        let outputs = vec![output("DP-1", true)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(plan.actions, Vec::new());
    }

    #[test]
    fn plan_contains_only_settings_that_differ() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("DP-1"),
            enabled: Some(true),
            mode: Some("3440x1440@59.973".parse().unwrap()),
            scale: Some(1.25),
            transform: Some(Transform::Normal),
            position: Some(Position { x: 0, y: 0 }),
            vrr: Some(false),
        }]);
        let outputs = vec![output("DP-1", true)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(
            plan.actions,
            vec![planned("DP-1".to_string(), OutputAction::Scale(1.25))]
        );
    }

    #[test]
    fn plan_does_not_disable_an_already_disabled_output() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("HDMI-A-1"),
            enabled: Some(false),
            mode: None,
            scale: None,
            transform: None,
            position: None,
            vrr: None,
        }]);
        let outputs = vec![output("DP-1", true), output("HDMI-A-1", false)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(plan.actions, Vec::new());
    }

    #[test]
    fn mode_without_refresh_matches_any_refresh_at_that_size() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("DP-1"),
            enabled: None,
            mode: Some("3440x1440".parse().unwrap()),
            scale: None,
            transform: None,
            position: None,
            vrr: None,
        }]);
        let outputs = vec![output("DP-1", true)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(plan.actions, Vec::new());
    }

    #[test]
    fn scale_within_niri_rounding_is_not_reapplied() {
        let config = config(true);
        let profile = profile_with_outputs(vec![ProfileOutput {
            matcher: matcher("DP-1"),
            enabled: None,
            mode: None,
            scale: Some(1.26),
            transform: None,
            position: None,
            vrr: None,
        }]);
        let mut dp1 = output("DP-1", true);
        dp1.logical.as_mut().expect("DP-1 should be enabled").scale = 151.0 / 120.0;

        let plan = build_apply_plan(&config, &profile, &[dp1]).expect("plan should build");

        assert_eq!(plan.actions, Vec::new());
    }

    #[test]
    fn refuses_all_disabled_plan_by_default() {
        let config = config(true);
        let profile = Profile {
            id: "off".to_string(),
            name: "Off".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![ProfileOutput {
                matcher: matcher("DP-1"),
                enabled: Some(false),
                mode: None,
                scale: None,
                transform: None,
                position: None,
                vrr: None,
            }],
        };
        let outputs = vec![output("DP-1", true)];

        let error = build_apply_plan(&config, &profile, &outputs).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::AllOutputsDisabled(id) if id == "off"));
    }

    #[test]
    fn rejects_unavailable_modes_before_planning_actions() {
        let config = config(true);
        let profile = Profile {
            id: "bad".to_string(),
            name: "Bad".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![ProfileOutput {
                matcher: matcher("DP-1"),
                enabled: None,
                mode: Some("1920x1080@60".parse().unwrap()),
                scale: None,
                transform: None,
                position: None,
                vrr: None,
            }],
        };
        let outputs = vec![output("DP-1", true)];

        let error = build_apply_plan(&config, &profile, &outputs).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::UnavailableMode { .. }));
    }

    #[test]
    fn rejects_overlapping_profile_layouts() {
        let config = config(true);
        let profile = Profile {
            id: "bad-layout".to_string(),
            name: "Bad Layout".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![
                ProfileOutput {
                    matcher: matcher("DP-1"),
                    enabled: None,
                    mode: None,
                    scale: None,
                    transform: None,
                    position: Some(Position { x: 0, y: 0 }),
                    vrr: None,
                },
                ProfileOutput {
                    matcher: matcher("DP-2"),
                    enabled: None,
                    mode: None,
                    scale: None,
                    transform: None,
                    position: Some(Position { x: 10, y: 10 }),
                    vrr: None,
                },
            ],
        };
        let outputs = vec![output("DP-1", true), output("DP-2", true)];

        let error = build_apply_plan(&config, &profile, &outputs).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::OverlappingLayout { .. }));
    }

    #[test]
    fn normalizes_profile_positions_before_planning() {
        let config = config(true);
        let profile = Profile {
            id: "normalized".to_string(),
            name: "Normalized".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: vec![
                ProfileOutput {
                    matcher: matcher("DP-1"),
                    enabled: None,
                    mode: None,
                    scale: None,
                    transform: None,
                    position: Some(Position { x: -100, y: -50 }),
                    vrr: None,
                },
                ProfileOutput {
                    matcher: matcher("DP-2"),
                    enabled: None,
                    mode: None,
                    scale: None,
                    transform: None,
                    position: Some(Position { x: 3_340, y: -50 }),
                    vrr: None,
                },
            ],
        };
        let outputs = vec![output("DP-1", true), output("DP-2", true)];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(
            plan.actions,
            vec![planned(
                "DP-2".to_string(),
                OutputAction::Position(Position { x: 3_440, y: 0 })
            )]
        );
    }

    #[test]
    fn executes_plan_actions_in_order() {
        let plan = ApplyPlan {
            profile_id: "home".to_string(),
            actions: vec![
                planned("DP-1".to_string(), OutputAction::Enable),
                planned("DP-1".to_string(), OutputAction::Scale(1.25)),
            ],
            warnings: vec!["unused output".to_string()],
        };
        let mut client = FakeClient::default();

        let report = execute_apply_plan(&mut client, &plan).expect("plan should execute");

        assert_eq!(report.applied_actions, 2);
        assert_eq!(report.warnings, vec!["unused output"]);
        assert_eq!(
            client.actions,
            vec![
                ("DP-1".to_string(), OutputAction::Enable),
                ("DP-1".to_string(), OutputAction::Scale(1.25)),
            ]
        );
    }

    #[test]
    fn failed_action_stops_plan() {
        let plan = ApplyPlan {
            profile_id: "home".to_string(),
            actions: vec![
                planned("DP-1".to_string(), OutputAction::Enable),
                planned("DP-2".to_string(), OutputAction::Disable),
                planned("DP-3".to_string(), OutputAction::Scale(1.0)),
            ],
            warnings: Vec::new(),
        };
        let mut client = FakeClient {
            fail_at: Some(1),
            ..FakeClient::default()
        };

        let error = execute_apply_plan(&mut client, &plan).expect_err("plan should fail");

        assert_eq!(error.action_index, 1);
        assert_eq!(client.actions.len(), 2);
    }

    #[test]
    fn dry_run_formats_plan_without_applying() {
        let plan = ApplyPlan {
            profile_id: "home".to_string(),
            actions: vec![
                planned(
                    "DP-1".to_string(),
                    OutputAction::Mode {
                        width: 3440,
                        height: 1440,
                        refresh_millihz: Some(59973),
                    },
                ),
                planned(
                    "DP-1".to_string(),
                    OutputAction::Position(Position { x: 0, y: 288 }),
                ),
            ],
            warnings: vec!["connected output DP-2 is not mentioned".to_string()],
        };

        let report = dry_run_apply_plan(&plan);
        let output = format_dry_run(&plan);

        assert_eq!(report.applied_actions, 0);
        assert!(output.contains("profile: home"));
        assert!(output.contains("mode 3440x1440@59.973"));
        assert!(output.contains("position 0,288"));
    }

    fn config(prevent_disable_all: bool) -> Config {
        Config {
            version: 1,
            daemon: DaemonConfig {
                prevent_disable_all,
                ..DaemonConfig::default()
            },
            profiles: Vec::new(),
        }
    }

    fn profile_with_outputs(outputs: Vec<ProfileOutput>) -> Profile {
        Profile {
            id: "home".to_string(),
            name: "Home".to_string(),
            priority: 0,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs,
        }
    }

    fn matcher(connector: &str) -> MonitorMatcher {
        MonitorMatcher {
            connector: Some(connector.to_string()),
            ..MonitorMatcher::default()
        }
    }

    fn output(connector: &str, enabled: bool) -> NiriOutput {
        let x = match connector {
            "DP-1" => 0,
            "HDMI-A-1" => 4_000,
            "DP-2" => 8_000,
            _ => 12_000,
        };

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
            current_mode: enabled.then_some(0),
            logical: enabled.then_some(LogicalOutput {
                x,
                y: 0,
                width: 3440,
                height: 1440,
                scale: 1.0,
                transform: Transform::Normal,
            }),
            vrr_supported: true,
            vrr_enabled: false,
        }
    }

    #[derive(Default)]
    struct FakeClient {
        actions: Vec<(String, OutputAction)>,
        fail_at: Option<usize>,
    }

    impl NiriClient for FakeClient {
        fn outputs(&mut self) -> Result<Vec<NiriOutput>, NiriError> {
            Ok(Vec::new())
        }

        fn apply_output_action(
            &mut self,
            output: &str,
            action: OutputAction,
        ) -> Result<(), NiriError> {
            self.actions.push((output.to_string(), action));
            if self.fail_at == Some(self.actions.len() - 1) {
                return Err(NiriError::NiriReply("boom".to_string()));
            }

            Ok(())
        }
    }
}
