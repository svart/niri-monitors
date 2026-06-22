use crate::control::{
    ControlRequest, ControlResponse, ErrorCode, ManualOutputPlacement, PreviewData, StatusData,
    default_control_socket_path,
};
use serde::de::DeserializeOwned;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

const SOCKET_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Default, PartialEq)]
pub enum DaemonConnectionState {
    #[default]
    Checking,
    Offline {
        message: String,
    },
    Running(StatusData),
}

#[derive(Debug, Error)]
pub enum DaemonClientError {
    #[error("{0}")]
    SocketPath(String),
    #[error("failed to connect to daemon socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write daemon request: {0}")]
    Write(#[source] std::io::Error),
    #[error("failed to read daemon response: {0}")]
    Read(#[source] std::io::Error),
    #[error("failed to encode daemon request: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("failed to decode daemon response: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("daemon response did not include data")]
    MissingData,
    #[error("daemon returned {code:?}: {message}")]
    Server { code: ErrorCode, message: String },
}

#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl DaemonClient {
    pub fn from_default_socket() -> Result<Self, DaemonClientError> {
        let socket_path = default_control_socket_path()
            .map_err(|error| DaemonClientError::SocketPath(error.to_string()))?;
        Ok(Self::new(socket_path))
    }

    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout: SOCKET_TIMEOUT,
        }
    }

    pub fn status(&self) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::Status)
    }

    pub fn reload_config(&self) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::ReloadConfig)
    }

    pub fn dry_run_profile(&self, profile_id: String) -> Result<PreviewData, DaemonClientError> {
        self.request(ControlRequest::DryRunProfile { profile_id })
    }

    pub fn preview_profile(&self, profile_id: String) -> Result<PreviewData, DaemonClientError> {
        self.request(ControlRequest::PreviewProfile { profile_id })
    }

    pub fn activate_profile(&self, profile_id: String) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::ActivateProfile { profile_id })
    }

    pub fn apply_manual_layout(
        &self,
        placements: Vec<ManualOutputPlacement>,
    ) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::ApplyManualLayout { placements })
    }

    pub fn set_auto_mode(&self, enabled: bool) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::SetAutoMode { enabled })
    }

    pub fn clear_manual_profile(&self) -> Result<StatusData, DaemonClientError> {
        self.request(ControlRequest::ClearManualProfile)
    }

    fn request<T>(&self, request: ControlRequest) -> Result<T, DaemonClientError>
    where
        T: DeserializeOwned,
    {
        let mut stream = connect(&self.socket_path, self.timeout)?;
        serde_json::to_writer(&mut stream, &request).map_err(DaemonClientError::Encode)?;
        stream.write_all(b"\n").map_err(DaemonClientError::Write)?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(DaemonClientError::Read)?;
        let response: ControlResponse =
            serde_json::from_str(&line).map_err(DaemonClientError::Decode)?;

        decode_response(response)
    }
}

fn connect(path: &Path, timeout: Duration) -> Result<UnixStream, DaemonClientError> {
    let stream = UnixStream::connect(path).map_err(|source| DaemonClientError::Connect {
        path: path.to_path_buf(),
        source,
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(DaemonClientError::Read)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(DaemonClientError::Write)?;
    Ok(stream)
}

fn decode_response<T>(response: ControlResponse) -> Result<T, DaemonClientError>
where
    T: DeserializeOwned,
{
    if response.ok {
        let data = response.data.ok_or(DaemonClientError::MissingData)?;
        return serde_json::from_value(data).map_err(DaemonClientError::Decode);
    }

    let Some(error) = response.error else {
        return Err(DaemonClientError::MissingData);
    };

    Err(DaemonClientError::Server {
        code: error.code,
        message: error.message,
    })
}

pub fn fetch_daemon_status() -> DaemonConnectionState {
    match DaemonClient::from_default_socket().and_then(|client| client.status()) {
        Ok(status) => DaemonConnectionState::Running(status),
        Err(error) => DaemonConnectionState::Offline {
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlResponse;

    #[test]
    fn decodes_status_response() {
        let status = StatusData {
            auto_apply: true,
            selected_profile: Some("home".to_owned()),
            manual_profile: None,
            manual_layout: false,
            outputs: Vec::new(),
            last_apply: None,
        };
        let decoded: StatusData = decode_response(ControlResponse::ok(&status)).unwrap();

        assert_eq!(decoded, status);
    }

    #[test]
    fn server_error_becomes_client_error() {
        let error = decode_response::<StatusData>(ControlResponse::error(
            ErrorCode::StateLock,
            "state lock failed",
        ))
        .unwrap_err();

        assert!(matches!(
            error,
            DaemonClientError::Server {
                code: ErrorCode::StateLock,
                ..
            }
        ));
    }

    #[test]
    fn decodes_preview_response() {
        let preview = PreviewData {
            profile_id: "home".to_owned(),
            actions: Vec::new(),
            warnings: vec!["check me".to_owned()],
            dry_run_output: "dry run".to_owned(),
        };
        let decoded: PreviewData = decode_response(ControlResponse::ok(&preview)).unwrap();

        assert_eq!(decoded, preview);
    }
}
