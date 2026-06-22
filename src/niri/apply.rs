use crate::config::is_valid_transform;
use crate::matching::matcher_matches_output;
use crate::model::{Config, Position, Profile, ProfileOutput};
use crate::niri::ipc::{NiriClient, NiriError, OutputAction};
use crate::niri::output::{NiriMode, NiriOutput};
use crate::placement::{
    derive_connected_output_rects, first_overlap_message, normalize_output_rect_positions,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    #[error("profile {profile_id} output {output} has invalid scale {scale}")]
    InvalidScale {
        profile_id: String,
        output: String,
        scale: f64,
    },
    #[error("profile {profile_id} output {output} has invalid transform {transform}")]
    InvalidTransform {
        profile_id: String,
        output: String,
        transform: String,
    },
    #[error("profile {profile_id} output {output} has invalid mode {mode}: {reason}")]
    InvalidMode {
        profile_id: String,
        output: String,
        mode: String,
        reason: String,
    },
    #[error("refusing to disable every connected output for profile {0}")]
    AllOutputsDisabled(String),
    #[error("profile {profile_id} has overlapping output layout: {message}")]
    OverlappingLayout { profile_id: String, message: String },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModeSpec {
    width: u16,
    height: u16,
    refresh_millihz: Option<u32>,
}

pub fn build_apply_plan(
    config: &Config,
    profile: &Profile,
    outputs: &[NiriOutput],
) -> Result<ApplyPlan, ApplyPlanError> {
    let mut warnings = Vec::new();
    let mut targets = HashSet::new();
    let mut final_enabled: HashMap<String, bool> = outputs
        .iter()
        .map(|output| (output.connector.clone(), output.logical.is_some()))
        .collect();

    let mut enable_actions = Vec::new();
    let mut mode_actions = Vec::new();
    let mut scale_actions = Vec::new();
    let mut transform_actions = Vec::new();
    let mut vrr_actions = Vec::new();
    let mut position_actions = Vec::new();
    let mut disable_actions = Vec::new();

    for (output_index, profile_output) in profile.outputs.iter().enumerate() {
        let Some(output) = resolve_profile_output(
            profile,
            output_index,
            profile_output,
            outputs,
            &mut warnings,
        )?
        else {
            continue;
        };

        if !targets.insert(output.connector.clone()) {
            return Err(ApplyPlanError::DuplicateOutputTarget {
                profile_id: profile.id.clone(),
                output: output.connector.clone(),
            });
        }

        let output_name = output.connector.clone();
        if profile_output.enabled == Some(false) {
            final_enabled.insert(output_name.clone(), false);
            disable_actions.push(planned(output_name, OutputAction::Disable));
            continue;
        }

        let requires_enabled = profile_output.enabled == Some(true)
            || profile_output.mode.is_some()
            || profile_output.scale.is_some()
            || profile_output.transform.is_some()
            || profile_output.position.is_some()
            || profile_output.vrr.is_some();

        if requires_enabled {
            final_enabled.insert(output_name.clone(), true);
        }

        if profile_output.enabled == Some(true) || (requires_enabled && output.logical.is_none()) {
            enable_actions.push(planned(output_name.clone(), OutputAction::Enable));
        }

        if let Some(mode) = &profile_output.mode {
            let mode_spec = validate_mode(profile, &output_name, mode, &output.modes)?;
            mode_actions.push(planned(
                output_name.clone(),
                OutputAction::Mode {
                    width: mode_spec.width,
                    height: mode_spec.height,
                    refresh_millihz: mode_spec.refresh_millihz,
                },
            ));
        }

        if let Some(scale) = profile_output.scale {
            validate_scale(profile, &output_name, scale)?;
            scale_actions.push(planned(output_name.clone(), OutputAction::Scale(scale)));
        }

        if let Some(transform) = &profile_output.transform {
            validate_transform(profile, &output_name, transform)?;
            transform_actions.push(planned(
                output_name.clone(),
                OutputAction::Transform(transform.clone()),
            ));
        }

        if let Some(vrr) = profile_output.vrr {
            vrr_actions.push(planned(output_name.clone(), OutputAction::Vrr(vrr)));
        }

        if let Some(position) = profile_output.position {
            position_actions.push(planned(output_name, OutputAction::Position(position)));
        }
    }

    for output in outputs {
        if !targets.contains(&output.connector) {
            warnings.push(format!(
                "connected output {} ({}) is not mentioned by profile {}",
                output.connector, output.description, profile.id
            ));
        }
    }

    if config.daemon.prevent_disable_all
        && !outputs.is_empty()
        && final_enabled.values().all(|enabled| !enabled)
    {
        return Err(ApplyPlanError::AllOutputsDisabled(profile.id.clone()));
    }

    normalize_position_actions(outputs, &disable_actions, &mut position_actions);

    let mut actions = Vec::new();
    actions.extend(enable_actions);
    actions.extend(mode_actions);
    actions.extend(scale_actions);
    actions.extend(transform_actions);
    actions.extend(vrr_actions);
    actions.extend(position_actions);
    actions.extend(disable_actions);

    validate_non_overlapping_layout(profile, outputs, &actions)?;

    Ok(ApplyPlan {
        profile_id: profile.id.clone(),
        actions,
        warnings,
    })
}

fn normalize_position_actions(
    outputs: &[NiriOutput],
    disable_actions: &[PlannedOutputAction],
    position_actions: &mut Vec<PlannedOutputAction>,
) {
    let mut rects = derive_connected_output_rects(outputs);

    for disable_action in disable_actions {
        rects.retain(|rect| {
            outputs
                .get(rect.output_index)
                .is_none_or(|output| output.connector != disable_action.output)
        });
    }

    for position_action in position_actions.iter() {
        let OutputAction::Position(position) = &position_action.action else {
            continue;
        };

        if let Some(rect) = rects.iter_mut().find(|rect| {
            outputs
                .get(rect.output_index)
                .is_some_and(|output| output.connector == position_action.output)
        }) {
            rect.x = position.x;
            rect.y = position.y;
        }
    }

    normalize_output_rect_positions(&mut rects);

    for rect in rects {
        let Some(output) = outputs.get(rect.output_index) else {
            continue;
        };
        let normalized = Position {
            x: rect.x,
            y: rect.y,
        };

        if let Some(position_action) = position_actions
            .iter_mut()
            .find(|action| action.output == output.connector)
        {
            position_action.action = OutputAction::Position(normalized);
            continue;
        }

        let Some(current) = output.logical.as_ref() else {
            continue;
        };
        if current.x != normalized.x || current.y != normalized.y {
            position_actions.push(planned(
                output.connector.clone(),
                OutputAction::Position(normalized),
            ));
        }
    }
}

fn validate_non_overlapping_layout(
    profile: &Profile,
    outputs: &[NiriOutput],
    actions: &[PlannedOutputAction],
) -> Result<(), ApplyPlanError> {
    let mut rects = derive_connected_output_rects(outputs);

    for planned_action in actions {
        match &planned_action.action {
            OutputAction::Disable => {
                rects.retain(|rect| {
                    outputs
                        .get(rect.output_index)
                        .is_none_or(|output| output.connector != planned_action.output)
                });
            }
            OutputAction::Position(position) => {
                if let Some(rect) = rects.iter_mut().find(|rect| {
                    outputs
                        .get(rect.output_index)
                        .is_some_and(|output| output.connector == planned_action.output)
                }) {
                    rect.x = position.x;
                    rect.y = position.y;
                }
            }
            OutputAction::Enable
            | OutputAction::Mode { .. }
            | OutputAction::Scale(_)
            | OutputAction::Transform(_)
            | OutputAction::Vrr(_) => {}
        }
    }

    if let Some(message) = first_overlap_message(&rects) {
        return Err(ApplyPlanError::OverlappingLayout {
            profile_id: profile.id.clone(),
            message,
        });
    }

    Ok(())
}

fn resolve_profile_output<'a>(
    profile: &Profile,
    output_index: usize,
    profile_output: &ProfileOutput,
    outputs: &'a [NiriOutput],
    warnings: &mut Vec<String>,
) -> Result<Option<&'a NiriOutput>, ApplyPlanError> {
    let matches: Vec<&NiriOutput> = outputs
        .iter()
        .filter(|output| matcher_matches_output(&profile_output.matcher, output))
        .collect();

    match matches.as_slice() {
        [] => {
            warnings.push(format!(
                "profile {} output {} did not match any connected output",
                profile.id, output_index
            ));
            Ok(None)
        }
        [output] => Ok(Some(output)),
        matches => Err(ApplyPlanError::AmbiguousMatcher {
            profile_id: profile.id.clone(),
            output_index,
            matches: matches
                .iter()
                .map(|output| output.connector.clone())
                .collect(),
        }),
    }
}

fn validate_scale(profile: &Profile, output: &str, scale: f64) -> Result<(), ApplyPlanError> {
    if scale.is_finite() && scale > 0.0 {
        Ok(())
    } else {
        Err(ApplyPlanError::InvalidScale {
            profile_id: profile.id.clone(),
            output: output.to_string(),
            scale,
        })
    }
}

fn validate_transform(
    profile: &Profile,
    output: &str,
    transform: &str,
) -> Result<(), ApplyPlanError> {
    if is_valid_transform(transform) {
        Ok(())
    } else {
        Err(ApplyPlanError::InvalidTransform {
            profile_id: profile.id.clone(),
            output: output.to_string(),
            transform: transform.to_string(),
        })
    }
}

fn validate_mode(
    profile: &Profile,
    output: &str,
    mode: &str,
    modes: &[NiriMode],
) -> Result<ModeSpec, ApplyPlanError> {
    let mode_spec = parse_mode_spec(mode).map_err(|reason| ApplyPlanError::InvalidMode {
        profile_id: profile.id.clone(),
        output: output.to_string(),
        mode: mode.to_string(),
        reason,
    })?;

    if modes.iter().any(|available| {
        available.width == mode_spec.width
            && available.height == mode_spec.height
            && mode_spec
                .refresh_millihz
                .is_none_or(|refresh| available.refresh_millihz == refresh)
    }) {
        Ok(mode_spec)
    } else {
        Err(ApplyPlanError::InvalidMode {
            profile_id: profile.id.clone(),
            output: output.to_string(),
            mode: mode.to_string(),
            reason: "mode is not available on this output".to_string(),
        })
    }
}

fn parse_mode_spec(mode: &str) -> Result<ModeSpec, String> {
    let (dimensions, refresh) = mode
        .split_once('@')
        .map_or((mode, None), |(dimensions, refresh)| {
            (dimensions, Some(refresh))
        });
    let (width, height) = dimensions
        .split_once('x')
        .ok_or_else(|| "expected WIDTHxHEIGHT[@HZ]".to_string())?;
    let width = width
        .parse::<u16>()
        .map_err(|_| "width must fit u16".to_string())?;
    let height = height
        .parse::<u16>()
        .map_err(|_| "height must fit u16".to_string())?;
    let refresh_millihz = refresh.map(parse_refresh_millihz).transpose()?;

    Ok(ModeSpec {
        width,
        height,
        refresh_millihz,
    })
}

fn parse_refresh_millihz(refresh: &str) -> Result<u32, String> {
    let refresh = refresh
        .parse::<f64>()
        .map_err(|_| "refresh must be a number".to_string())?;
    if !refresh.is_finite() || refresh <= 0.0 {
        return Err("refresh must be finite and greater than zero".to_string());
    }

    let millihz = (refresh * 1000.0).round();
    if millihz > f64::from(u32::MAX) {
        return Err("refresh must fit u32 millihertz".to_string());
    }

    Ok(millihz as u32)
}

fn planned(output: String, action: OutputAction) -> PlannedOutputAction {
    PlannedOutputAction { output, action }
}

fn describe_output_action(action: &OutputAction) -> String {
    match action {
        OutputAction::Enable => "enable".to_string(),
        OutputAction::Disable => "disable".to_string(),
        OutputAction::Mode {
            width,
            height,
            refresh_millihz,
        } => refresh_millihz.map_or_else(
            || format!("mode {width}x{height}"),
            |refresh| format!("mode {width}x{height}@{:.3}", f64::from(refresh) / 1000.0),
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
    use crate::model::{DaemonConfig, MonitorMatcher, Position, ProfileCondition};
    use crate::niri::output::LogicalOutput;

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
                    mode: Some("3440x1440@59.973".to_string()),
                    scale: Some(1.25),
                    transform: Some("normal".to_string()),
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
            ],
        };
        let outputs = vec![
            output("DP-1", true),
            output("HDMI-A-1", true),
            output("DP-2", true),
        ];

        let plan = build_apply_plan(&config, &profile, &outputs).expect("plan should build");

        assert_eq!(
            plan.actions,
            vec![
                planned("DP-1".to_string(), OutputAction::Enable),
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
                    OutputAction::Transform("normal".to_string()),
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
                mode: Some("1920x1080@60".to_string()),
                scale: None,
                transform: None,
                position: None,
                vrr: None,
            }],
        };
        let outputs = vec![output("DP-1", true)];

        let error = build_apply_plan(&config, &profile, &outputs).expect_err("plan should fail");

        assert!(matches!(error, ApplyPlanError::InvalidMode { .. }));
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

        assert!(plan.actions.contains(&planned(
            "DP-1".to_string(),
            OutputAction::Position(Position { x: 0, y: 0 })
        )));
        assert!(plan.actions.contains(&planned(
            "DP-2".to_string(),
            OutputAction::Position(Position { x: 3_440, y: 0 })
        )));
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
                transform: "normal".to_string(),
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
                return Err(NiriError::CommandFailed("boom".to_string()));
            }

            Ok(())
        }
    }
}
