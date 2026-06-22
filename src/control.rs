use crate::config::{ConfigError, load_config};
use crate::daemon::{
    DaemonError, DaemonState, LastApply, reconcile_once, reconcile_outputs_for_manual_apply,
};
use crate::matching::profile_matches_outputs;
use crate::model::{Position, Profile};
use crate::niri::apply::{
    ApplyPlan, ApplyPlanError, PlannedOutputAction, build_apply_plan, execute_apply_plan,
    format_dry_run,
};
use crate::niri::ipc::{NiriClient, NiriError, OutputAction, SocketNiriClient};
use crate::niri::output::NiriOutput;
use crate::placement::{
    derive_connected_output_rects, first_overlap_message, normalize_output_rect_positions,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    ReloadConfig,
    SetAutoMode {
        enabled: bool,
    },
    ActivateProfile {
        profile_id: String,
    },
    ApplyManualLayout {
        placements: Vec<ManualOutputPlacement>,
    },
    ClearManualProfile,
    PreviewProfile {
        profile_id: String,
    },
    DryRunProfile {
        profile_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualOutputPlacement {
    pub output: String,
    pub position: Position,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl ControlResponse {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            data: Some(serde_json::to_value(data).unwrap_or_else(|error| {
                json!({
                    "serialization_error": error.to_string(),
                })
            })),
            error: None,
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(ControlError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    BadRequest,
    ProfileNotFound,
    ProfileNotMatching,
    ConfigReloadFailed,
    PlanFailed,
    LayoutOverlaps,
    ApplyFailed,
    NiriError,
    StateLock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusData {
    pub auto_apply: bool,
    pub selected_profile: Option<String>,
    pub manual_profile: Option<String>,
    pub manual_layout: bool,
    pub outputs: Vec<NiriOutput>,
    pub last_apply: Option<LastApply>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewData {
    pub profile_id: String,
    pub actions: Vec<PlannedOutputAction>,
    pub warnings: Vec<String>,
    pub dry_run_output: String,
}

#[derive(Debug, Error)]
pub enum ControlServerError {
    #[error("XDG_RUNTIME_DIR is not set")]
    MissingRuntimeDir,
    #[error("failed to create control socket {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to remove stale control socket {path}: {source}")]
    RemoveStaleSocket {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn default_control_socket_path() -> Result<PathBuf, ControlServerError> {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|runtime_dir| runtime_dir.join("niri-monitors.sock"))
        .ok_or(ControlServerError::MissingRuntimeDir)
}

pub fn start_control_socket(
    socket_path: PathBuf,
    state: Arc<Mutex<DaemonState>>,
    config_path: PathBuf,
) -> Result<JoinHandle<()>, ControlServerError> {
    if socket_path.exists() {
        fs::remove_file(&socket_path).map_err(|source| ControlServerError::RemoveStaleSocket {
            path: socket_path.clone(),
            source,
        })?;
    }

    let listener = UnixListener::bind(&socket_path).map_err(|source| ControlServerError::Bind {
        path: socket_path.clone(),
        source,
    })?;
    info!(path = %socket_path.display(), "started control socket");

    Ok(thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_control_stream(stream, &state, &config_path),
                Err(error) => error!(%error, "failed to accept control socket connection"),
            }
        }
    }))
}

pub fn handle_control_request(
    request: ControlRequest,
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    config_path: &Path,
) -> ControlResponse {
    match request {
        ControlRequest::Status => ControlResponse::ok(status_data(state)),
        ControlRequest::ReloadConfig => reload_config(state, client, config_path),
        ControlRequest::SetAutoMode { enabled } => set_auto_mode(state, client, enabled),
        ControlRequest::ActivateProfile { profile_id } => {
            activate_profile(state, client, profile_id)
        }
        ControlRequest::ApplyManualLayout { placements } => {
            apply_manual_layout(state, client, placements)
        }
        ControlRequest::ClearManualProfile => clear_manual_profile(state, client),
        ControlRequest::PreviewProfile { profile_id }
        | ControlRequest::DryRunProfile { profile_id } => {
            preview_profile(state, client, &profile_id)
        }
    }
}

fn handle_control_stream(
    mut stream: UnixStream,
    state: &Arc<Mutex<DaemonState>>,
    config_path: &Path,
) {
    let response = read_control_request(&mut stream).map_or_else(
        |error| ControlResponse::error(ErrorCode::BadRequest, error),
        |request| {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(_) => {
                    return ControlResponse::error(
                        ErrorCode::StateLock,
                        "daemon state lock poisoned",
                    );
                }
            };
            let mut client = match SocketNiriClient::from_env() {
                Ok(client) => client,
                Err(error) => return error_from_niri(error),
            };
            handle_control_request(request, &mut state, &mut client, config_path)
        },
    );

    if let Err(error) = write_control_response(&mut stream, &response) {
        error!(%error, "failed to write control socket response");
    }
}

fn read_control_request(stream: &mut UnixStream) -> Result<ControlRequest, String> {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

fn write_control_response(
    stream: &mut UnixStream,
    response: &ControlResponse,
) -> std::io::Result<()> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")
}

fn reload_config(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    config_path: &Path,
) -> ControlResponse {
    match load_config(config_path) {
        Ok(config) => {
            let auto_apply = config.daemon.auto_apply;
            state.config = config;
            state.auto_apply = auto_apply;
            reconcile_to_status(state, client)
        }
        Err(error) => error_from_config(error),
    }
}

fn set_auto_mode(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    enabled: bool,
) -> ControlResponse {
    state.auto_apply = enabled;
    if enabled {
        state.manual_profile = None;
        state.manual_layout = false;
        state.last_applied_fingerprint = None;
        reconcile_to_status(state, client)
    } else {
        ControlResponse::ok(status_data(state))
    }
}

fn activate_profile(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    profile_id: String,
) -> ControlResponse {
    let Some(profile) = find_profile(state, &profile_id).cloned() else {
        return ControlResponse::error(
            ErrorCode::ProfileNotFound,
            format!("profile not found: {profile_id}"),
        );
    };

    let outputs = match client.outputs() {
        Ok(outputs) => outputs,
        Err(error) => return error_from_niri(error),
    };
    if !profile_matches_outputs(&profile, &outputs) {
        return ControlResponse::error(
            ErrorCode::ProfileNotMatching,
            format!("profile does not match connected outputs: {profile_id}"),
        );
    }

    state.auto_apply = false;
    state.manual_layout = false;
    state.manual_profile = Some(profile_id);
    state.last_applied_fingerprint = None;
    match reconcile_outputs_for_manual_apply(client, state, outputs) {
        Ok(_) => ControlResponse::ok(status_data(state)),
        Err(error) => error_from_daemon(error),
    }
}

fn clear_manual_profile(state: &mut DaemonState, client: &mut impl NiriClient) -> ControlResponse {
    state.manual_profile = None;
    state.manual_layout = false;
    state.auto_apply = true;
    state.last_applied_fingerprint = None;
    reconcile_to_status(state, client)
}

fn apply_manual_layout(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    placements: Vec<ManualOutputPlacement>,
) -> ControlResponse {
    if placements.is_empty() {
        return ControlResponse::error(ErrorCode::BadRequest, "manual layout has no placements");
    }

    let outputs = match client.outputs() {
        Ok(outputs) => outputs,
        Err(error) => return error_from_niri(error),
    };
    let mut planned_outputs = outputs.clone();
    let mut seen_outputs = HashSet::new();

    for placement in &placements {
        if !seen_outputs.insert(placement.output.clone()) {
            return ControlResponse::error(
                ErrorCode::BadRequest,
                format!("duplicate manual layout placement for {}", placement.output),
            );
        }

        let Some(output) = outputs
            .iter()
            .find(|output| output.connector == placement.output)
        else {
            return ControlResponse::error(
                ErrorCode::BadRequest,
                format!(
                    "manual layout output is not connected: {}",
                    placement.output
                ),
            );
        };

        if output.logical.is_none() {
            return ControlResponse::error(
                ErrorCode::BadRequest,
                format!("manual layout output is disabled: {}", placement.output),
            );
        }

        if let Some(output) = planned_outputs
            .iter_mut()
            .find(|output| output.connector == placement.output)
            && let Some(logical) = &mut output.logical
        {
            logical.x = placement.position.x;
            logical.y = placement.position.y;
        }
    }

    let mut rects = derive_connected_output_rects(&planned_outputs);
    normalize_output_rect_positions(&mut rects);

    if let Some(message) = first_overlap_message(&rects) {
        return ControlResponse::error(
            ErrorCode::LayoutOverlaps,
            format!("manual layout has overlapping outputs: {message}"),
        );
    }

    for rect in &rects {
        if let Some(output) = planned_outputs.get_mut(rect.output_index)
            && let Some(logical) = &mut output.logical
        {
            logical.x = rect.x;
            logical.y = rect.y;
        }
    }

    let actions = rects
        .iter()
        .filter_map(|rect| {
            let output = planned_outputs.get(rect.output_index)?;
            Some(PlannedOutputAction {
                output: output.connector.clone(),
                action: OutputAction::Position(Position {
                    x: rect.x,
                    y: rect.y,
                }),
            })
        })
        .collect();

    let plan = ApplyPlan {
        profile_id: "manual-layout".to_owned(),
        actions,
        warnings: Vec::new(),
    };

    state.auto_apply = false;
    state.manual_profile = None;
    state.manual_layout = true;
    state.selected_profile = None;
    state.last_applied_fingerprint = None;

    match execute_apply_plan(client, &plan) {
        Ok(report) => {
            state.outputs = planned_outputs;
            state.last_apply = Some(LastApply {
                profile_id: report.profile_id,
                success: true,
                warnings: report.warnings,
            });
            ControlResponse::ok(status_data(state))
        }
        Err(error) => {
            state.last_apply = Some(LastApply {
                profile_id: plan.profile_id,
                success: false,
                warnings: Vec::new(),
            });
            error_from_daemon(DaemonError::Apply(error))
        }
    }
}

fn preview_profile(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    profile_id: &str,
) -> ControlResponse {
    let Some(profile) = find_profile(state, profile_id) else {
        return ControlResponse::error(
            ErrorCode::ProfileNotFound,
            format!("profile not found: {profile_id}"),
        );
    };

    let outputs = match client.outputs() {
        Ok(outputs) => outputs,
        Err(error) => return error_from_niri(error),
    };
    match build_apply_plan(&state.config, profile, &outputs) {
        Ok(plan) => ControlResponse::ok(PreviewData {
            profile_id: plan.profile_id.clone(),
            actions: plan.actions.clone(),
            warnings: plan.warnings.clone(),
            dry_run_output: format_dry_run(&plan),
        }),
        Err(error) => error_from_plan(error),
    }
}

fn reconcile_to_status(state: &mut DaemonState, client: &mut impl NiriClient) -> ControlResponse {
    match reconcile_once(client, state, false) {
        Ok(_) => ControlResponse::ok(status_data(state)),
        Err(error) => error_from_daemon(error),
    }
}

fn status_data(state: &DaemonState) -> StatusData {
    StatusData {
        auto_apply: state.auto_apply,
        selected_profile: state.selected_profile.clone(),
        manual_profile: state.manual_profile.clone(),
        manual_layout: state.manual_layout,
        outputs: state.outputs.clone(),
        last_apply: state.last_apply.clone(),
    }
}

fn find_profile<'a>(state: &'a DaemonState, profile_id: &str) -> Option<&'a Profile> {
    state
        .config
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
}

fn error_from_config(error: ConfigError) -> ControlResponse {
    ControlResponse::error(ErrorCode::ConfigReloadFailed, error.to_string())
}

fn error_from_plan(error: ApplyPlanError) -> ControlResponse {
    let code = match &error {
        ApplyPlanError::OverlappingLayout { .. } => ErrorCode::LayoutOverlaps,
        _ => ErrorCode::PlanFailed,
    };
    ControlResponse::error(code, error.to_string())
}

fn error_from_niri(error: NiriError) -> ControlResponse {
    ControlResponse::error(ErrorCode::NiriError, error.to_string())
}

fn error_from_daemon(error: DaemonError) -> ControlResponse {
    match error {
        DaemonError::Niri(error) => error_from_niri(error),
        DaemonError::Plan(error) => error_from_plan(error),
        DaemonError::Apply(error) => {
            ControlResponse::error(ErrorCode::ApplyFailed, error.to_string())
        }
        DaemonError::StateLock => ControlResponse::error(ErrorCode::StateLock, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Config, DaemonConfig, MonitorMatcher, Position, ProfileCondition, ProfileOutput,
    };
    use crate::niri::ipc::OutputAction;
    use crate::niri::output::{LogicalOutput, NiriMode};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn deserializes_status_request() {
        let request: ControlRequest = serde_json::from_str(r#"{"type":"status"}"#).unwrap();

        assert_eq!(request, ControlRequest::Status);
    }

    #[test]
    fn returns_status_response() {
        let mut state = DaemonState::new(config("home"));
        state.selected_profile = Some("home".to_string());
        state.outputs = vec![output("DP-1")];
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::Status,
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(response.data.unwrap()["selected_profile"], "home");
    }

    #[test]
    fn manual_activation_applies_profile() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ActivateProfile {
                profile_id: "home".to_string(),
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.manual_profile.as_deref(), Some("home"));
        assert!(!state.auto_apply);
        assert!(!state.manual_layout);
        assert_eq!(client.actions.len(), 3);
    }

    #[test]
    fn manual_activation_applies_when_auto_apply_is_disabled() {
        let mut state = DaemonState::new(config("home"));
        state.auto_apply = false;
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ActivateProfile {
                profile_id: "home".to_string(),
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.manual_profile.as_deref(), Some("home"));
        assert_eq!(client.actions.len(), 3);
    }

    #[test]
    fn manual_layout_applies_positions_and_disables_auto_apply() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ApplyManualLayout {
                placements: vec![ManualOutputPlacement {
                    output: "DP-1".to_string(),
                    position: Position { x: 120, y: 80 },
                }],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert!(!state.auto_apply);
        assert!(state.manual_layout);
        assert_eq!(state.manual_profile, None);
        assert_eq!(state.selected_profile, None);
        assert_eq!(client.actions.len(), 1);
        assert_eq!(
            client.actions[0],
            (
                "DP-1".to_string(),
                OutputAction::Position(Position { x: 0, y: 0 })
            )
        );
        assert_eq!(state.outputs[0].logical.as_ref().unwrap().x, 0);
        assert_eq!(state.outputs[0].logical.as_ref().unwrap().y, 0);
    }

    #[test]
    fn overlapping_manual_layout_is_rejected_before_apply() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1"), output("DP-2")]);

        let response = handle_control_request(
            ControlRequest::ApplyManualLayout {
                placements: vec![ManualOutputPlacement {
                    output: "DP-2".to_string(),
                    position: Position { x: 10, y: 10 },
                }],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, ErrorCode::LayoutOverlaps);
        assert!(client.actions.is_empty());
        assert!(state.auto_apply);
        assert!(!state.manual_layout);
    }

    #[test]
    fn manual_layout_normalizes_positions_before_apply() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1"), output("DP-2")]);

        let response = handle_control_request(
            ControlRequest::ApplyManualLayout {
                placements: vec![
                    ManualOutputPlacement {
                        output: "DP-1".to_string(),
                        position: Position { x: -100, y: -50 },
                    },
                    ManualOutputPlacement {
                        output: "DP-2".to_string(),
                        position: Position { x: 3_340, y: -50 },
                    },
                ],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(client.actions.len(), 2);
        assert_eq!(
            client.actions[0],
            (
                "DP-1".to_string(),
                OutputAction::Position(Position { x: 0, y: 0 })
            )
        );
        assert_eq!(
            client.actions[1],
            (
                "DP-2".to_string(),
                OutputAction::Position(Position { x: 3_440, y: 0 })
            )
        );
        assert_eq!(state.outputs[0].logical.as_ref().unwrap().x, 0);
        assert_eq!(state.outputs[1].logical.as_ref().unwrap().x, 3_440);
    }

    #[test]
    fn enabling_auto_apply_clears_manual_layout() {
        let mut state = DaemonState::new(config("home"));
        state.auto_apply = false;
        state.manual_layout = true;
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::SetAutoMode { enabled: true },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert!(state.auto_apply);
        assert!(!state.manual_layout);
        assert_eq!(state.manual_profile, None);
        assert_eq!(state.selected_profile.as_deref(), Some("home"));
    }

    #[test]
    fn missing_profile_uses_stable_error_code() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ActivateProfile {
                profile_id: "missing".to_string(),
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, ErrorCode::ProfileNotFound);
    }

    #[test]
    fn reload_config_replaces_state_config() {
        let path = temp_config_path();
        fs::write(&path, config_toml("office")).unwrap();
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response =
            handle_control_request(ControlRequest::ReloadConfig, &mut state, &mut client, &path);

        let _ = fs::remove_file(path);
        assert!(response.ok);
        assert_eq!(state.config.profiles[0].id, "office");
    }

    fn config(id: &str) -> Config {
        Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![Profile {
                id: id.to_string(),
                name: id.to_string(),
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

    fn config_toml(id: &str) -> String {
        format!(
            r#"
version = 1

[[profiles]]
id = "{id}"
name = "{id}"
priority = 100

[profiles.condition]
all_connected = [{{ connector = "DP-1" }}]

[[profiles.outputs]]
match = {{ connector = "DP-1" }}
enabled = true
mode = "3440x1440@59.973"
position = {{ x = 0, y = 0 }}
"#
        )
    }

    fn temp_config_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("niri-monitors-test-{nanos}.toml"))
    }

    fn output(connector: &str) -> NiriOutput {
        let x = match connector {
            "DP-1" => 0,
            "DP-2" => 4_000,
            _ => 8_000,
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
