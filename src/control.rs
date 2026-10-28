use crate::config::{ConfigError, load_config};
use crate::daemon::{
    DaemonError, DaemonState, LastApply, Mode, apply_manual_layout, apply_profile_draft,
    reconcile_once, reconcile_outputs_for_manual_apply,
};
use crate::matching::profile_matches_outputs;
use crate::model::{ManualOutputPlacement, Profile};
use crate::niri::apply::{
    ApplyPlanError, ManualLayoutError, PlannedOutputAction, build_apply_plan, format_dry_run,
};
use crate::niri::ipc::{NiriClient, NiriError, SocketNiriClient};
use crate::niri::output::NiriOutput;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    ReloadConfig,
    SetAutoMode {
        enabled: bool,
    },
    ActivateProfile {
        profile: String,
    },
    ApplyManualLayout {
        placements: Vec<ManualOutputPlacement>,
    },
    /// Applies a profile sent in the request, such as an unsaved GUI draft.
    ApplyProfileDraft {
        profile: Profile,
    },
    ClearManualProfile,
    PreviewProfile {
        profile: String,
    },
    DryRunProfile {
        profile: String,
    },
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
    pub profile: String,
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
        ControlRequest::ActivateProfile { profile } => activate_profile(state, client, profile),
        ControlRequest::ApplyManualLayout { placements } => {
            match apply_manual_layout(client, state, &placements) {
                Ok(()) => ControlResponse::ok(status_data(state)),
                Err(error) => error_from_daemon(error),
            }
        }
        ControlRequest::ApplyProfileDraft { profile } => {
            match apply_profile_draft(client, state, &profile) {
                Ok(()) => ControlResponse::ok(status_data(state)),
                Err(error) => error_from_daemon(error),
            }
        }
        ControlRequest::ClearManualProfile => clear_manual_profile(state, client),
        ControlRequest::PreviewProfile { profile } | ControlRequest::DryRunProfile { profile } => {
            preview_profile(state, client, &profile)
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
            state.reload(config);
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
    if enabled {
        state.mode = Mode::Auto;
        reconcile_to_status(state, client)
    } else {
        state.pause();
        ControlResponse::ok(status_data(state))
    }
}

fn activate_profile(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    name: String,
) -> ControlResponse {
    let Some(profile) = find_profile(state, &name).cloned() else {
        return ControlResponse::error(
            ErrorCode::ProfileNotFound,
            format!("profile not found: {name:?}"),
        );
    };

    let outputs = match client.outputs() {
        Ok(outputs) => outputs,
        Err(error) => return error_from_niri(error),
    };
    if !profile_matches_outputs(&profile, &outputs) {
        return ControlResponse::error(
            ErrorCode::ProfileNotMatching,
            format!("profile does not match connected outputs: {name:?}"),
        );
    }

    state.mode = Mode::ManualProfile(name);
    match reconcile_outputs_for_manual_apply(client, state, outputs) {
        Ok(_) => ControlResponse::ok(status_data(state)),
        Err(error) => error_from_daemon(error),
    }
}

fn clear_manual_profile(state: &mut DaemonState, client: &mut impl NiriClient) -> ControlResponse {
    state.mode = Mode::Auto;
    reconcile_to_status(state, client)
}

fn preview_profile(
    state: &mut DaemonState,
    client: &mut impl NiriClient,
    name: &str,
) -> ControlResponse {
    let Some(profile) = find_profile(state, name) else {
        return ControlResponse::error(
            ErrorCode::ProfileNotFound,
            format!("profile not found: {name:?}"),
        );
    };

    let outputs = match client.outputs() {
        Ok(outputs) => outputs,
        Err(error) => return error_from_niri(error),
    };
    match build_apply_plan(&state.config, profile, &outputs) {
        Ok(plan) => ControlResponse::ok(PreviewData {
            profile: plan.profile.clone(),
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
        auto_apply: state.mode == Mode::Auto,
        selected_profile: state.selected_profile.clone(),
        manual_profile: state.mode.manual_profile().map(str::to_owned),
        manual_layout: state.mode == Mode::ManualLayout,
        outputs: state.outputs.clone(),
        last_apply: state.last_apply.clone(),
    }
}

fn find_profile<'a>(state: &'a DaemonState, name: &str) -> Option<&'a Profile> {
    state
        .config
        .profiles
        .iter()
        .find(|profile| profile.name == name)
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
        DaemonError::ManualLayout(error) => {
            let code = match error {
                ManualLayoutError::Overlap(_) => ErrorCode::LayoutOverlaps,
                ManualLayoutError::Empty
                | ManualLayoutError::DuplicateOutput(_)
                | ManualLayoutError::NotConnected(_)
                | ManualLayoutError::Disabled(_)
                | ManualLayoutError::UnavailableMode { .. }
                | ManualLayoutError::InvalidScale { .. } => ErrorCode::BadRequest,
            };
            ControlResponse::error(code, error.to_string())
        }
        DaemonError::StateLock => ControlResponse::error(ErrorCode::StateLock, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transform;
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
    fn profile_requests_name_the_profile() {
        let request: ControlRequest =
            serde_json::from_str(r#"{"type":"activate_profile","profile":"Work Desk"}"#).unwrap();

        assert_eq!(
            request,
            ControlRequest::ActivateProfile {
                profile: "Work Desk".to_string()
            }
        );
        assert!(
            serde_json::from_str::<ControlRequest>(
                r#"{"type":"preview_profile","profile_id":"x"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn manual_layout_placement_settings_are_optional() {
        let request: ControlRequest = serde_json::from_str(
            r#"{"type":"apply_manual_layout","placements":[
                {"output":"DP-1","position":{"x":0,"y":0}},
                {"output":"DP-2","position":{"x":3440,"y":0},"mode":"2560x1440@59.951",
                 "scale":1.25,"transform":"90"}
            ]}"#,
        )
        .unwrap();

        let ControlRequest::ApplyManualLayout { placements } = request else {
            panic!("expected apply_manual_layout, got {request:?}");
        };
        assert_eq!(
            (
                placements[0].mode,
                placements[0].scale,
                placements[0].transform
            ),
            (None, None, None)
        );
        assert_eq!(
            (
                placements[1].mode,
                placements[1].scale,
                placements[1].transform
            ),
            (
                Some("2560x1440@59.951".parse().unwrap()),
                Some(1.25),
                Some(Transform::Rotate90)
            )
        );
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
                profile: "home".to_string(),
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.mode, Mode::ManualProfile("home".to_string()));
        assert_eq!(
            client.actions,
            vec![("DP-1".to_string(), OutputAction::Scale(1.5))]
        );
    }

    #[test]
    fn manual_activation_applies_when_auto_apply_is_disabled() {
        let mut state = DaemonState::new(config("home"));
        state.mode = Mode::Paused;
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ActivateProfile {
                profile: "home".to_string(),
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.mode, Mode::ManualProfile("home".to_string()));
        assert_eq!(
            client.actions,
            vec![("DP-1".to_string(), OutputAction::Scale(1.5))]
        );
    }

    #[test]
    fn manual_layout_moves_only_changed_outputs_and_enters_manual_layout_mode() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1"), output("DP-2")]);

        let response = handle_control_request(
            ControlRequest::ApplyManualLayout {
                placements: vec![ManualOutputPlacement {
                    output: "DP-2".to_string(),
                    position: Position { x: 3_440, y: 0 },
                    mode: None,
                    scale: None,
                    transform: None,
                }],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.mode, Mode::ManualLayout);
        assert_eq!(state.selected_profile, None);
        assert_eq!(
            client.actions,
            vec![(
                "DP-2".to_string(),
                OutputAction::Position(Position { x: 3_440, y: 0 })
            )]
        );
        assert_eq!(state.outputs[1].logical.as_ref().unwrap().x, 3_440);
    }

    #[test]
    fn invalid_manual_layout_placements_are_bad_requests() {
        let placement = |output: &str| ManualOutputPlacement {
            output: output.to_string(),
            position: Position { x: 0, y: 0 },
            mode: None,
            scale: None,
            transform: None,
        };
        let mut disabled = output("DP-2");
        disabled.logical = None;
        let unavailable_mode = ManualOutputPlacement {
            mode: Some("640x480@60".parse().unwrap()),
            ..placement("DP-1")
        };
        let zero_scale = ManualOutputPlacement {
            scale: Some(0.0),
            ..placement("DP-1")
        };

        for placements in [
            Vec::new(),
            vec![placement("DP-1"), placement("DP-1")],
            vec![placement("HDMI-A-1")],
            vec![placement("DP-2")],
            vec![unavailable_mode],
            vec![zero_scale],
        ] {
            let mut state = DaemonState::new(config("home"));
            let mut client = FakeClient::new(vec![output("DP-1"), disabled.clone()]);

            let response = handle_control_request(
                ControlRequest::ApplyManualLayout { placements },
                &mut state,
                &mut client,
                Path::new("unused.toml"),
            );

            assert_eq!(response.error.unwrap().code, ErrorCode::BadRequest);
            assert!(client.actions.is_empty());
            assert_eq!(state.mode, Mode::Auto);
        }
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
                    mode: None,
                    scale: None,
                    transform: None,
                }],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, ErrorCode::LayoutOverlaps);
        assert!(client.actions.is_empty());
        assert_eq!(state.mode, Mode::Auto);
    }

    #[test]
    fn profile_draft_request_carries_the_profile() {
        let request: ControlRequest = serde_json::from_str(
            r#"{"type":"apply_profile_draft","profile":{"name":"home","outputs":[{"match":{"connector":"DP-1"},"scale":1.25}]}}"#,
        )
        .unwrap();

        let ControlRequest::ApplyProfileDraft { profile } = request else {
            panic!("expected apply_profile_draft");
        };
        assert_eq!(profile.name, "home");
        assert_eq!(profile.outputs[0].scale, Some(1.25));
    }

    #[test]
    fn profile_draft_is_applied_without_changing_the_config() {
        let config = config("home");
        let mut draft = config.profiles[0].clone();
        draft.outputs[0].scale = Some(1.25);
        let mut state = DaemonState::new(config.clone());
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ApplyProfileDraft { profile: draft },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.mode, Mode::ManualLayout);
        assert_eq!(state.config, config);
        assert_eq!(
            client.actions,
            vec![("DP-1".to_string(), OutputAction::Scale(1.25))]
        );
        assert_eq!(state.last_apply.unwrap().profile, "home");
    }

    #[test]
    fn profile_draft_that_cannot_be_planned_changes_nothing() {
        let mut draft = config("home").profiles[0].clone();
        draft.outputs[0].mode = Some("640x480@60".parse().unwrap());
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ApplyProfileDraft { profile: draft },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert_eq!(response.error.unwrap().code, ErrorCode::PlanFailed);
        assert!(client.actions.is_empty());
        assert_eq!(state.mode, Mode::Auto);
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
                        mode: None,
                        scale: None,
                        transform: None,
                    },
                    ManualOutputPlacement {
                        output: "DP-2".to_string(),
                        position: Position { x: 3_340, y: -50 },
                        mode: None,
                        scale: None,
                        transform: None,
                    },
                ],
            },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(
            client.actions,
            vec![(
                "DP-2".to_string(),
                OutputAction::Position(Position { x: 3_440, y: 0 })
            )]
        );
        assert_eq!(state.outputs[0].logical.as_ref().unwrap().x, 0);
        assert_eq!(state.outputs[1].logical.as_ref().unwrap().x, 3_440);
    }

    #[test]
    fn enabling_auto_apply_clears_manual_layout() {
        let mut state = DaemonState::new(config("home"));
        state.mode = Mode::ManualLayout;
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::SetAutoMode { enabled: true },
            &mut state,
            &mut client,
            Path::new("unused.toml"),
        );

        assert!(response.ok);
        assert_eq!(state.mode, Mode::Auto);
        assert_eq!(state.selected_profile.as_deref(), Some("home"));
    }

    #[test]
    fn missing_profile_uses_stable_error_code() {
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);

        let response = handle_control_request(
            ControlRequest::ActivateProfile {
                profile: "missing".to_string(),
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
        assert_eq!(state.config.profiles[0].name, "office");
    }

    #[test]
    fn reload_config_ends_manual_layout_mode() {
        let path = temp_config_path();
        fs::write(&path, config_toml("home")).unwrap();
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1"), output("DP-2")]);
        let manual_layout = handle_control_request(
            ControlRequest::ApplyManualLayout {
                placements: vec![ManualOutputPlacement {
                    output: "DP-2".to_string(),
                    position: Position { x: 3_440, y: 0 },
                    mode: None,
                    scale: None,
                    transform: None,
                }],
            },
            &mut state,
            &mut client,
            &path,
        );

        let response =
            handle_control_request(ControlRequest::ReloadConfig, &mut state, &mut client, &path);

        let _ = fs::remove_file(path);
        assert!(manual_layout.ok);
        let status = response.data.unwrap();
        assert_eq!(status["auto_apply"], true);
        assert_eq!(status["manual_layout"], false);
        assert_eq!(status["selected_profile"], "home");
    }

    #[test]
    fn reload_config_drops_manual_profile() {
        let path = temp_config_path();
        fs::write(&path, config_toml("home")).unwrap();
        let mut state = DaemonState::new(config("home"));
        let mut client = FakeClient::new(vec![output("DP-1")]);
        let activation = handle_control_request(
            ControlRequest::ActivateProfile {
                profile: "home".to_string(),
            },
            &mut state,
            &mut client,
            &path,
        );

        let response =
            handle_control_request(ControlRequest::ReloadConfig, &mut state, &mut client, &path);

        let _ = fs::remove_file(path);
        assert!(activation.ok);
        let status = response.data.unwrap();
        assert_eq!(status["auto_apply"], true);
        assert_eq!(status["manual_profile"], serde_json::Value::Null);
    }

    fn config(name: &str) -> Config {
        Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![Profile {
                name: name.to_string(),
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
                    mode: Some("3440x1440@59.973".parse().unwrap()),
                    scale: Some(1.5),
                    transform: None,
                    position: Some(Position { x: 0, y: 0 }),
                    vrr: None,
                }],
            }],
        }
    }

    fn config_toml(name: &str) -> String {
        format!(
            r#"
version = 1

[[profiles]]
name = "{name}"
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
                transform: Transform::Normal,
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
            // Mirror niri: the next outputs read reports the new position.
            if let OutputAction::Position(position) = &action
                && let Some(logical) = self
                    .outputs
                    .iter_mut()
                    .find(|candidate| candidate.connector == output)
                    .and_then(|candidate| candidate.logical.as_mut())
            {
                logical.x = position.x;
                logical.y = position.y;
            }
            self.actions.push((output.to_string(), action));
            Ok(())
        }
    }
}
