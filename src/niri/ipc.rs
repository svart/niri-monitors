use crate::model::{Position, Transform};
use crate::niri::output::{NiriOutput, NiriOutputParseError, parse_outputs_response_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use thiserror::Error;

pub trait NiriClient {
    fn outputs(&mut self) -> Result<Vec<NiriOutput>, NiriError>;

    fn apply_output_action(&mut self, output: &str, action: OutputAction) -> Result<(), NiriError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutputAction {
    Enable,
    Disable,
    Mode {
        width: u16,
        height: u16,
        refresh_millihz: Option<u32>,
    },
    Scale(f64),
    Transform(Transform),
    Position(Position),
    Vrr(bool),
}

#[derive(Debug, Error)]
pub enum NiriError {
    #[error("NIRI_SOCKET is not set")]
    MissingSocketEnv,
    #[error("failed to connect to niri socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to communicate with niri socket: {0}")]
    SocketIo(#[from] std::io::Error),
    #[error("failed to encode niri IPC request: {0}")]
    EncodeRequest(serde_json::Error),
    #[error("failed to parse niri IPC response: {0}")]
    ParseReply(serde_json::Error),
    #[error("niri returned an error response: {0}")]
    NiriReply(String),
    #[error("failed to parse niri outputs: {0}")]
    ParseOutputs(#[from] NiriOutputParseError),
}

#[derive(Debug, Clone)]
pub struct SocketNiriClient {
    socket_path: PathBuf,
}

impl SocketNiriClient {
    pub fn from_env() -> Result<Self, NiriError> {
        let socket_path = env::var_os("NIRI_SOCKET").ok_or(NiriError::MissingSocketEnv)?;
        Ok(Self::new(socket_path))
    }

    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    fn request(&self, request: serde_json::Value) -> Result<String, NiriError> {
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|source| NiriError::Connect {
                path: self.socket_path.clone(),
                source,
            })?;

        serde_json::to_writer(&mut stream, &request).map_err(NiriError::EncodeRequest)?;
        stream.write_all(b"\n")?;
        stream.shutdown(Shutdown::Write)?;

        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response)?;
        Ok(response)
    }
}

impl NiriClient for SocketNiriClient {
    fn outputs(&mut self) -> Result<Vec<NiriOutput>, NiriError> {
        let response = self.request(json!("Outputs"))?;
        parse_outputs_response_json(&response).map_err(NiriError::from)
    }

    fn apply_output_action(&mut self, output: &str, action: OutputAction) -> Result<(), NiriError> {
        let response = self.request(output_action_request(output, &action))?;
        parse_ok_response(&response)
    }
}

fn output_action_request(output: &str, action: &OutputAction) -> Value {
    json!({
        "Output": {
            "output": output,
            "action": output_action_json(action),
        }
    })
}

fn output_action_json(action: &OutputAction) -> Value {
    match action {
        OutputAction::Enable => json!("On"),
        OutputAction::Disable => json!("Off"),
        OutputAction::Mode {
            width,
            height,
            refresh_millihz,
        } => json!({
            "Mode": {
                "mode": {
                    "Specific": {
                        "width": width,
                        "height": height,
                        "refresh": refresh_millihz.map(|refresh| f64::from(refresh) / 1000.0),
                    }
                }
            }
        }),
        OutputAction::Scale(scale) => json!({
            "Scale": {
                "scale": {
                    "Specific": scale,
                }
            }
        }),
        OutputAction::Transform(transform) => json!({
            "Transform": {
                "transform": transform.niri_name(),
            }
        }),
        OutputAction::Position(Position { x, y }) => json!({
            "Position": {
                "position": {
                    "Specific": {
                        "x": x,
                        "y": y,
                    }
                }
            }
        }),
        OutputAction::Vrr(vrr) => json!({
            "Vrr": {
                "vrr": {
                    "vrr": vrr,
                    "on_demand": false,
                }
            }
        }),
    }
}

fn parse_ok_response(response: &str) -> Result<(), NiriError> {
    let value: Value = serde_json::from_str(response).map_err(NiriError::ParseReply)?;
    let Value::Object(mut object) = value else {
        return Err(NiriError::NiriReply(format!(
            "unexpected response: {value}"
        )));
    };

    if let Some(error) = object.remove("Err") {
        return Err(NiriError::NiriReply(response_error_message(error)));
    }

    if object.remove("Ok").is_some() {
        return Ok(());
    }

    Err(NiriError::NiriReply(format!(
        "unexpected response object: {}",
        Value::Object(object)
    )))
}

fn response_error_message(error: Value) -> String {
    error
        .as_str()
        .map_or_else(|| error.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_output_action_request_json() {
        let request = output_action_request(
            "DP-1",
            &OutputAction::Mode {
                width: 3440,
                height: 1440,
                refresh_millihz: Some(59973),
            },
        );

        assert_eq!(
            request,
            json!({
                "Output": {
                    "output": "DP-1",
                    "action": {
                        "Mode": {
                            "mode": {
                                "Specific": {
                                    "width": 3440,
                                    "height": 1440,
                                    "refresh": 59.973,
                                }
                            }
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn encodes_transform_and_position_actions() {
        assert_eq!(
            output_action_request("DP-1", &OutputAction::Transform(Transform::Flipped90)),
            json!({
                "Output": {
                    "output": "DP-1",
                    "action": {
                        "Transform": {
                            "transform": "Flipped90",
                        }
                    }
                }
            })
        );
        assert_eq!(
            output_action_request("DP-1", &OutputAction::Position(Position { x: 10, y: -20 })),
            json!({
                "Output": {
                    "output": "DP-1",
                    "action": {
                        "Position": {
                            "position": {
                                "Specific": {
                                    "x": 10,
                                    "y": -20,
                                }
                            }
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn parses_ok_and_error_responses() {
        parse_ok_response(r#"{"Ok":"Handled"}"#).expect("ok response should parse");

        let error = parse_ok_response(r#"{"Err":"bad output"}"#).expect_err("error should fail");
        assert!(matches!(error, NiriError::NiriReply(message) if message == "bad output"));
    }
}
