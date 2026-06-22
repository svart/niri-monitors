use crate::matching::select_profile;
use crate::model::Config;
use crate::niri::apply::{
    ApplyExecutionError, ApplyPlan, ApplyPlanError, build_apply_plan, dry_run_apply_plan,
    execute_apply_plan, format_dry_run,
};
use crate::niri::ipc::{NiriClient, NiriError};
use crate::niri::output::NiriOutput;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub config: Config,
    pub outputs: Vec<NiriOutput>,
    pub selected_profile: Option<String>,
    pub last_applied_fingerprint: Option<String>,
    pub manual_profile: Option<String>,
    pub manual_layout: bool,
    pub auto_apply: bool,
    pub last_apply: Option<LastApply>,
}

impl DaemonState {
    pub fn new(config: Config) -> Self {
        Self {
            auto_apply: config.daemon.auto_apply,
            config,
            outputs: Vec::new(),
            selected_profile: None,
            last_applied_fingerprint: None,
            manual_profile: None,
            manual_layout: false,
            last_apply: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastApply {
    pub profile_id: String,
    pub success: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub profile_id: Option<String>,
    pub applied: bool,
    pub dry_run: bool,
    pub skipped: bool,
    pub warnings: Vec<String>,
    pub dry_run_output: Option<String>,
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("niri IPC error: {0}")]
    Niri(#[from] NiriError),
    #[error("failed to build apply plan: {0}")]
    Plan(#[from] ApplyPlanError),
    #[error("failed to apply plan: {0}")]
    Apply(#[from] ApplyExecutionError),
    #[error("daemon state lock poisoned")]
    StateLock,
}

pub fn reconcile_once(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    dry_run: bool,
) -> Result<ReconcileOutcome, DaemonError> {
    let outputs = client.outputs()?;
    reconcile_outputs(client, state, outputs, dry_run)
}

pub fn reconcile_outputs(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    outputs: Vec<NiriOutput>,
    dry_run: bool,
) -> Result<ReconcileOutcome, DaemonError> {
    reconcile_outputs_with_mode(client, state, outputs, dry_run, false)
}

pub fn reconcile_outputs_for_manual_apply(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    outputs: Vec<NiriOutput>,
) -> Result<ReconcileOutcome, DaemonError> {
    reconcile_outputs_with_mode(client, state, outputs, false, true)
}

fn reconcile_outputs_with_mode(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    outputs: Vec<NiriOutput>,
    dry_run: bool,
    force_apply: bool,
) -> Result<ReconcileOutcome, DaemonError> {
    let output_fingerprint = output_fingerprint(&outputs);
    state.outputs = outputs;

    if state.manual_layout && !state.auto_apply && !force_apply {
        state.selected_profile = None;
        debug!("manual layout active; skipping automatic profile selection");
        return Ok(ReconcileOutcome {
            profile_id: None,
            applied: false,
            dry_run,
            skipped: true,
            warnings: Vec::new(),
            dry_run_output: None,
        });
    }

    let selected = select_profile(
        &state.config,
        &state.outputs,
        state.manual_profile.as_deref(),
    );
    let Some(profile) = selected else {
        state.selected_profile = None;
        debug!("no matching monitor profile");
        return Ok(ReconcileOutcome {
            profile_id: None,
            applied: false,
            dry_run,
            skipped: false,
            warnings: Vec::new(),
            dry_run_output: None,
        });
    };

    state.selected_profile = Some(profile.id.clone());
    let plan = build_apply_plan(&state.config, profile, &state.outputs)?;
    let reconcile_fingerprint = reconcile_fingerprint(&plan, &output_fingerprint);
    let warnings = plan.warnings.clone();

    if dry_run {
        let report = dry_run_apply_plan(&plan);
        let dry_run_output = format_dry_run(&plan);
        state.last_apply = Some(LastApply {
            profile_id: report.profile_id.clone(),
            success: true,
            warnings: report.warnings,
        });
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: true,
            skipped: false,
            warnings,
            dry_run_output: Some(dry_run_output),
        });
    }

    if !state.auto_apply && !force_apply {
        info!(profile_id = %plan.profile_id, "auto-apply disabled; selected profile only");
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: false,
            skipped: true,
            warnings,
            dry_run_output: None,
        });
    }

    if state.last_applied_fingerprint.as_deref() == Some(&reconcile_fingerprint) {
        debug!(profile_id = %plan.profile_id, "skipping unchanged monitor state");
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: false,
            skipped: true,
            warnings,
            dry_run_output: None,
        });
    }

    match execute_apply_plan(client, &plan) {
        Ok(report) => {
            info!(profile_id = %report.profile_id, actions = report.applied_actions, "applied monitor profile");
            state.last_applied_fingerprint = Some(reconcile_fingerprint);
            state.last_apply = Some(LastApply {
                profile_id: report.profile_id.clone(),
                success: true,
                warnings: report.warnings.clone(),
            });
            Ok(ReconcileOutcome {
                profile_id: Some(report.profile_id),
                applied: true,
                dry_run: false,
                skipped: false,
                warnings: report.warnings,
                dry_run_output: None,
            })
        }
        Err(error) => {
            state.last_apply = Some(LastApply {
                profile_id: plan.profile_id,
                success: false,
                warnings,
            });
            Err(DaemonError::Apply(error))
        }
    }
}

pub fn run_loop(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    dry_run: bool,
) -> Result<(), DaemonError> {
    reconcile_once(client, state, dry_run)?;

    loop {
        let previous_fingerprint = output_fingerprint(&state.outputs);
        thread::sleep(Duration::from_millis(state.config.daemon.poll_interval_ms));

        let outputs = client.outputs()?;
        let next_fingerprint = output_fingerprint(&outputs);
        if next_fingerprint == previous_fingerprint {
            continue;
        }

        info!("detected output state change; debouncing before reconcile");
        thread::sleep(Duration::from_millis(state.config.daemon.debounce_ms));
        reconcile_once(client, state, dry_run)?;
    }
}

pub fn run_loop_shared(
    client: &mut impl NiriClient,
    state: Arc<Mutex<DaemonState>>,
    dry_run: bool,
) -> Result<(), DaemonError> {
    {
        let mut state = state.lock().map_err(|_| DaemonError::StateLock)?;
        reconcile_once(client, &mut state, dry_run)?;
    }

    loop {
        let (previous_fingerprint, poll_interval_ms, debounce_ms) = {
            let state = state.lock().map_err(|_| DaemonError::StateLock)?;
            (
                output_fingerprint(&state.outputs),
                state.config.daemon.poll_interval_ms,
                state.config.daemon.debounce_ms,
            )
        };
        thread::sleep(Duration::from_millis(poll_interval_ms));

        let outputs = client.outputs()?;
        let next_fingerprint = output_fingerprint(&outputs);
        if next_fingerprint == previous_fingerprint {
            continue;
        }

        info!("detected output state change; debouncing before reconcile");
        thread::sleep(Duration::from_millis(debounce_ms));
        let mut state = state.lock().map_err(|_| DaemonError::StateLock)?;
        reconcile_once(client, &mut state, dry_run)?;
    }
}

pub fn output_fingerprint(outputs: &[NiriOutput]) -> String {
    let mut parts: Vec<String> = outputs
        .iter()
        .map(|output| {
            let logical = output.logical.as_ref().map_or_else(
                || "disabled".to_string(),
                |logical| {
                    format!(
                        "enabled:{}:{}:{}:{}:{}:{}",
                        logical.x,
                        logical.y,
                        logical.width,
                        logical.height,
                        logical.scale,
                        logical.transform
                    )
                },
            );
            format!(
                "{}|{}|{}|{}|{:?}|{}|{}|{}",
                output.connector,
                output.make,
                output.model,
                output.serial.as_deref().unwrap_or("Unknown"),
                output.current_mode,
                output.vrr_supported,
                output.vrr_enabled,
                logical
            )
        })
        .collect();
    parts.sort();
    parts.join("\n")
}

fn reconcile_fingerprint(plan: &ApplyPlan, output_fingerprint: &str) -> String {
    format!(
        "profile:{}\noutputs:{}\nactions:{:?}",
        plan.profile_id, output_fingerprint, plan.actions
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        DaemonConfig, MonitorMatcher, Position, Profile, ProfileCondition, ProfileOutput,
    };
    use crate::niri::ipc::OutputAction;
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn output_fingerprint_is_stable_across_ordering() {
        let first = vec![output("DP-1", 0), output("HDMI-A-1", 100)];
        let second = vec![output("HDMI-A-1", 100), output("DP-1", 0)];

        assert_eq!(output_fingerprint(&first), output_fingerprint(&second));
    }

    #[test]
    fn applies_selected_profile_on_startup() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("reconcile should pass");

        assert_eq!(outcome.profile_id.as_deref(), Some("home"));
        assert!(outcome.applied);
        assert_eq!(client.actions.len(), 3);
    }

    #[test]
    fn does_not_reapply_unchanged_output_state() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        reconcile_once(&mut client, &mut state, false).expect("first reconcile should pass");
        reconcile_once(&mut client, &mut state, false).expect("second reconcile should pass");

        assert_eq!(client.actions.len(), 3);
    }

    #[test]
    fn reapplies_after_output_state_changes() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        reconcile_once(&mut client, &mut state, false).expect("first reconcile should pass");
        client.outputs = vec![output("DP-1", 50)];
        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("changed reconcile should pass");

        assert!(outcome.applied);
        assert_eq!(client.actions.len(), 6);
    }

    #[test]
    fn dry_run_does_not_apply_actions() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        let outcome = reconcile_once(&mut client, &mut state, true).expect("dry run should pass");

        assert!(!outcome.applied);
        assert!(outcome.dry_run_output.unwrap().contains("profile: home"));
        assert!(client.actions.is_empty());
    }

    fn config() -> Config {
        Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![Profile {
                id: "home".to_string(),
                name: "Home".to_string(),
                priority: 100,
                enabled: true,
                condition: ProfileCondition {
                    all_connected: vec![MonitorMatcher {
                        connector: Some("DP-1".to_string()),
                        ..MonitorMatcher::default()
                    }],
                    ..ProfileCondition::default()
                },
                outputs: vec![ProfileOutput {
                    matcher: MonitorMatcher {
                        connector: Some("DP-1".to_string()),
                        ..MonitorMatcher::default()
                    },
                    enabled: Some(true),
                    mode: Some("3440x1440@59.973".to_string()),
                    scale: None,
                    transform: None,
                    position: Some(Position { x: 0, y: 0 }),
                    vrr: None,
                }],
            }],
        }
    }

    fn output(connector: &str, x: i32) -> NiriOutput {
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

    struct FakeClient {
        outputs: Vec<NiriOutput>,
        actions: Vec<(String, OutputAction)>,
    }

    impl FakeClient {
        fn new(outputs: Vec<NiriOutput>) -> Self {
            Self {
                outputs,
                actions: Vec::new(),
            }
        }
    }

    impl NiriClient for FakeClient {
        fn outputs(&mut self) -> Result<Vec<NiriOutput>, NiriError> {
            Ok(self.outputs.clone())
        }

        fn apply_output_action(
            &mut self,
            output: &str,
            action: OutputAction,
        ) -> Result<(), NiriError> {
            self.actions.push((output.to_string(), action));
            Ok(())
        }
    }
}
