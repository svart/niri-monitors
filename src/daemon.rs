use crate::matching::select_profile;
use crate::model::{Config, ManualOutputPlacement};
use crate::niri::apply::{
    ApplyExecutionError, ApplyExecutionReport, ApplyPlan, ApplyPlanError, ManualLayoutError,
    build_apply_plan, build_manual_layout_plan, dry_run_apply_plan, execute_apply_plan,
    format_dry_run,
};
use crate::niri::ipc::{NiriClient, NiriError};
use crate::niri::output::NiriOutput;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info};

/// How the daemon reacts to output changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Select the best matching profile and apply it.
    Auto,
    /// Select the best matching profile without applying it.
    Paused,
    /// Like `Paused`, but this profile is selected while it is enabled and matches.
    /// It is applied once, when it is activated.
    ManualProfile(String),
    /// A layout from the control socket is active; profiles are not selected or applied.
    ManualLayout,
}

impl Mode {
    /// The mode the daemon starts in and returns to when the config is reloaded.
    pub fn from_config(config: &Config) -> Self {
        if config.daemon.auto_apply {
            Self::Auto
        } else {
            Self::Paused
        }
    }

    pub fn manual_profile(&self) -> Option<&str> {
        match self {
            Self::ManualProfile(profile_id) => Some(profile_id),
            Self::Auto | Self::Paused | Self::ManualLayout => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DaemonState {
    pub config: Config,
    pub mode: Mode,
    pub outputs: Vec<NiriOutput>,
    pub selected_profile: Option<String>,
    pub last_apply: Option<LastApply>,
}

impl DaemonState {
    pub fn new(config: Config) -> Self {
        Self {
            mode: Mode::from_config(&config),
            config,
            outputs: Vec::new(),
            selected_profile: None,
            last_apply: None,
        }
    }

    /// Replaces the config and returns to the mode it configures, which ends a manual
    /// profile or manual layout.
    pub fn reload(&mut self, config: Config) {
        self.mode = Mode::from_config(&config);
        self.config = config;
    }

    /// Stops applying profiles automatically. A manual profile or layout stays active.
    pub fn pause(&mut self) {
        if self.mode == Mode::Auto {
            self.mode = Mode::Paused;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastApply {
    pub profile_id: String,
    pub success: bool,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LastApply {
    pub fn succeeded(profile_id: String, warnings: Vec<String>) -> Self {
        Self {
            profile_id,
            success: true,
            warnings,
            error: None,
        }
    }

    pub fn failed(profile_id: String, warnings: Vec<String>, error: impl Display) -> Self {
        Self {
            profile_id,
            success: false,
            warnings,
            error: Some(error.to_string()),
        }
    }
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
    #[error(transparent)]
    ManualLayout(#[from] ManualLayoutError),
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
    reconcile_outputs_with(client, state, outputs, dry_run, false)
}

/// Reconciles and applies the selected profile even when automatic loading is paused.
pub fn reconcile_outputs_for_manual_apply(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    outputs: Vec<NiriOutput>,
) -> Result<ReconcileOutcome, DaemonError> {
    reconcile_outputs_with(client, state, outputs, false, true)
}

fn reconcile_outputs_with(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    outputs: Vec<NiriOutput>,
    dry_run: bool,
    force_apply: bool,
) -> Result<ReconcileOutcome, DaemonError> {
    state.outputs = outputs;

    if state.mode == Mode::ManualLayout && !force_apply {
        state.selected_profile = None;
        debug!("manual layout active; skipping profile selection");
        return Ok(ReconcileOutcome {
            profile_id: None,
            applied: false,
            dry_run,
            skipped: true,
            warnings: Vec::new(),
            dry_run_output: None,
        });
    }

    let selected = select_profile(&state.config, &state.outputs, state.mode.manual_profile());
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
    let plan = match build_apply_plan(&state.config, profile, &state.outputs) {
        Ok(plan) => plan,
        Err(error) => {
            state.last_apply = Some(LastApply::failed(profile.id.clone(), Vec::new(), &error));
            return Err(error.into());
        }
    };
    let warnings = plan.warnings.clone();

    if dry_run {
        let report = dry_run_apply_plan(&plan);
        let dry_run_output = format_dry_run(&plan);
        state.last_apply = Some(LastApply::succeeded(report.profile_id, report.warnings));
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: true,
            skipped: false,
            warnings,
            dry_run_output: Some(dry_run_output),
        });
    }

    if state.mode != Mode::Auto && !force_apply {
        info!(profile_id = %plan.profile_id, "automatic loading paused; selected profile only");
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: false,
            skipped: true,
            warnings,
            dry_run_output: None,
        });
    }

    if plan.actions.is_empty() {
        debug!(profile_id = %plan.profile_id, "outputs already match profile");
        state.last_apply = Some(LastApply::succeeded(
            plan.profile_id.clone(),
            warnings.clone(),
        ));
        return Ok(ReconcileOutcome {
            profile_id: Some(plan.profile_id),
            applied: false,
            dry_run: false,
            skipped: true,
            warnings,
            dry_run_output: None,
        });
    }

    let report = execute_and_record(client, state, &plan)?;
    info!(profile_id = %report.profile_id, actions = report.applied_actions, "applied monitor profile");
    Ok(ReconcileOutcome {
        profile_id: Some(report.profile_id),
        applied: true,
        dry_run: false,
        skipped: false,
        warnings: report.warnings,
        dry_run_output: None,
    })
}

/// Moves enabled outputs to `placements` and switches to manual layout mode, so profiles
/// are not applied over the layout. Invalid or overlapping layouts change nothing.
pub fn apply_manual_layout(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    placements: &[ManualOutputPlacement],
) -> Result<(), DaemonError> {
    let outputs = client.outputs()?;
    let plan = build_manual_layout_plan(&outputs, placements)?;

    state.mode = Mode::ManualLayout;
    state.selected_profile = None;
    state.outputs = outputs;
    execute_and_record(client, state, &plan)?;
    Ok(())
}

/// Executes `plan` and records the result in `last_apply`.
fn execute_and_record(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    plan: &ApplyPlan,
) -> Result<ApplyExecutionReport, DaemonError> {
    match execute_apply_plan(client, plan) {
        Ok(report) => {
            state.last_apply = Some(LastApply::succeeded(
                report.profile_id.clone(),
                report.warnings.clone(),
            ));
            // Keep what niri reports after the apply, so the poll loop does not
            // mistake this apply for an output change.
            state.outputs = client.outputs()?;
            Ok(report)
        }
        Err(error) => {
            state.last_apply = Some(LastApply::failed(
                plan.profile_id.clone(),
                plan.warnings.clone(),
                &error,
            ));
            Err(DaemonError::Apply(error))
        }
    }
}

/// Like `reconcile_once`, but a profile that cannot be planned or applied is only logged
/// and recorded in `last_apply`. Exiting would not fix the profile and would make the
/// service restart in a loop.
///
/// Errors reading niri outputs are still returned: niri's socket path changes when niri
/// restarts, so the service manager has to restart the daemon with the new `NIRI_SOCKET`.
pub fn reconcile_logging_profile_errors(
    client: &mut impl NiriClient,
    state: &mut DaemonState,
    dry_run: bool,
) -> Result<(), DaemonError> {
    match reconcile_once(client, state, dry_run) {
        Ok(_) => Ok(()),
        Err(error @ (DaemonError::Plan(_) | DaemonError::Apply(_))) => {
            error!(%error, "could not apply monitor profile; waiting for the next output change");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Reconciles once, then reconciles again whenever niri reports different outputs.
/// Returns only when niri cannot be reached or the state lock is poisoned.
pub fn run_loop(
    client: &mut impl NiriClient,
    state: &Mutex<DaemonState>,
    dry_run: bool,
) -> Result<(), DaemonError> {
    reconcile_logging_profile_errors(client, &mut *lock_state(state)?, dry_run)?;

    loop {
        poll_once(client, state, dry_run, &mut thread::sleep)?;
    }
}

/// Waits one poll interval, then reconciles if niri reports different outputs than the
/// last reconcile saw. `sleep` is a parameter so tests can run this without waiting.
pub fn poll_once(
    client: &mut impl NiriClient,
    state: &Mutex<DaemonState>,
    dry_run: bool,
    sleep: &mut impl FnMut(Duration),
) -> Result<(), DaemonError> {
    let (previous_outputs, poll_interval, debounce) = {
        let state = lock_state(state)?;
        (
            state.outputs.clone(),
            Duration::from_millis(state.config.daemon.poll_interval_ms),
            Duration::from_millis(state.config.daemon.debounce_ms),
        )
    };
    sleep(poll_interval);

    // Outputs are parsed from niri's connector-keyed map, so their order is stable.
    if client.outputs()? == previous_outputs {
        return Ok(());
    }

    info!("detected output state change; debouncing before reconcile");
    sleep(debounce);
    reconcile_logging_profile_errors(client, &mut *lock_state(state)?, dry_run)
}

fn lock_state(state: &Mutex<DaemonState>) -> Result<MutexGuard<'_, DaemonState>, DaemonError> {
    state.lock().map_err(|_| DaemonError::StateLock)
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
    fn applies_selected_profile_on_startup() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("reconcile should pass");

        assert_eq!(outcome.profile_id.as_deref(), Some("home"));
        assert!(outcome.applied);
        assert_eq!(
            client.actions,
            vec![("DP-1".to_string(), OutputAction::Scale(1.5))]
        );
    }

    #[test]
    fn does_not_reapply_after_own_apply() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        reconcile_once(&mut client, &mut state, false).expect("first reconcile should pass");
        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("second reconcile should pass");

        assert!(!outcome.applied);
        assert!(outcome.skipped);
        assert_eq!(client.actions.len(), 1);
    }

    #[test]
    fn reapplies_after_output_state_changes() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());

        reconcile_once(&mut client, &mut state, false).expect("first reconcile should pass");
        client.outputs = vec![output("DP-1", 0)];
        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("changed reconcile should pass");

        assert!(outcome.applied);
        assert_eq!(client.actions.len(), 2);
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

    #[test]
    fn pause_keeps_manual_overrides() {
        let mut state = DaemonState::new(config());
        state.pause();
        assert_eq!(state.mode, Mode::Paused);

        state.mode = Mode::ManualLayout;
        state.pause();
        assert_eq!(state.mode, Mode::ManualLayout);
    }

    #[test]
    fn manual_layout_mode_does_not_select_or_apply_profiles() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());
        state.mode = Mode::ManualLayout;

        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("reconcile should pass");

        assert!(outcome.skipped);
        assert_eq!(state.selected_profile, None);
        assert!(client.actions.is_empty());
    }

    #[test]
    fn paused_mode_selects_without_applying() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config());
        state.mode = Mode::Paused;

        let outcome =
            reconcile_once(&mut client, &mut state, false).expect("reconcile should pass");

        assert!(outcome.skipped);
        assert_eq!(state.selected_profile.as_deref(), Some("home"));
        assert!(client.actions.is_empty());
    }

    #[test]
    fn poll_does_not_apply_again_after_own_apply() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let state = Mutex::new(DaemonState::new(config()));
        reconcile_once(&mut client, &mut state.lock().unwrap(), false)
            .expect("startup reconcile should pass");

        poll_once(&mut client, &state, false, &mut |_| {}).expect("poll should pass");

        assert_eq!(client.actions.len(), 1);
    }

    #[test]
    fn poll_reconciles_after_output_change() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let state = Mutex::new(DaemonState::new(config()));
        reconcile_once(&mut client, &mut state.lock().unwrap(), false)
            .expect("startup reconcile should pass");
        client.outputs = vec![output("DP-1", 0)];

        poll_once(&mut client, &state, false, &mut |_| {}).expect("poll should pass");

        assert_eq!(client.actions.len(), 2);
    }

    #[test]
    fn poll_keeps_running_when_profile_cannot_be_planned() {
        let mut config = config();
        config.profiles[0].outputs[0].mode = Some("1920x1080@60".to_string());
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let state = Mutex::new(DaemonState::new(config));

        poll_once(&mut client, &state, false, &mut |_| {}).expect("plan errors should not stop");

        let last_apply = state.lock().unwrap().last_apply.clone().unwrap();
        assert_eq!(last_apply.profile_id, "home");
        assert!(!last_apply.success);
        assert!(last_apply.error.unwrap().contains("1920x1080@60"));
        assert!(client.actions.is_empty());
    }

    #[test]
    fn poll_keeps_running_when_niri_rejects_an_action() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        client.reject_actions = true;
        let state = Mutex::new(DaemonState::new(config()));

        poll_once(&mut client, &state, false, &mut |_| {}).expect("apply errors should not stop");

        let last_apply = state.lock().unwrap().last_apply.clone().unwrap();
        assert!(!last_apply.success);
        assert!(last_apply.error.unwrap().contains("rejected"));
    }

    #[test]
    fn poll_returns_error_when_niri_is_unreachable() {
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        client.unreachable = true;
        let state = Mutex::new(DaemonState::new(config()));

        let error = poll_once(&mut client, &state, false, &mut |_| {})
            .expect_err("niri connection errors should stop the daemon");

        assert!(matches!(error, DaemonError::Niri(_)));
    }

    #[test]
    fn startup_reconcile_keeps_running_when_profile_cannot_be_planned() {
        let mut config = config();
        config.profiles[0].outputs[0].mode = Some("1920x1080@60".to_string());
        let mut client = FakeClient::new(vec![output("DP-1", 0)]);
        let mut state = DaemonState::new(config);

        reconcile_logging_profile_errors(&mut client, &mut state, false)
            .expect("plan errors should not stop");

        assert!(!state.last_apply.unwrap().success);
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
                    scale: Some(1.5),
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

    #[derive(Default)]
    struct FakeClient {
        outputs: Vec<NiriOutput>,
        actions: Vec<(String, OutputAction)>,
        reject_actions: bool,
        unreachable: bool,
    }

    impl FakeClient {
        fn new(outputs: Vec<NiriOutput>) -> Self {
            Self {
                outputs,
                ..Self::default()
            }
        }
    }

    impl NiriClient for FakeClient {
        fn outputs(&mut self) -> Result<Vec<NiriOutput>, NiriError> {
            if self.unreachable {
                return Err(NiriError::MissingSocketEnv);
            }
            Ok(self.outputs.clone())
        }

        fn apply_output_action(
            &mut self,
            output: &str,
            action: OutputAction,
        ) -> Result<(), NiriError> {
            if self.reject_actions {
                return Err(NiriError::NiriReply("rejected".to_string()));
            }

            // Mirror niri: the next outputs read reports the applied settings.
            if let Some(logical) = self
                .outputs
                .iter_mut()
                .find(|candidate| candidate.connector == output)
                .and_then(|candidate| candidate.logical.as_mut())
            {
                match &action {
                    OutputAction::Scale(scale) => logical.scale = *scale,
                    OutputAction::Position(position) => {
                        logical.x = position.x;
                        logical.y = position.y;
                    }
                    _ => {}
                }
            }
            self.actions.push((output.to_string(), action));
            Ok(())
        }
    }
}
