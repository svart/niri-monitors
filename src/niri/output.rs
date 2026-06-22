use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NiriOutput {
    pub connector: String,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    pub description: String,
    pub modes: Vec<NiriMode>,
    pub current_mode: Option<usize>,
    pub logical: Option<LogicalOutput>,
    pub vrr_supported: bool,
    pub vrr_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NiriMode {
    pub width: u16,
    pub height: u16,
    pub refresh_millihz: u32,
    pub is_preferred: bool,
}

impl NiriMode {
    pub fn refresh_hz(self) -> f64 {
        f64::from(self.refresh_millihz) / 1000.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogicalOutput {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub transform: String,
}

#[derive(Debug, Error)]
pub enum NiriOutputParseError {
    #[error("failed to parse niri output JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("niri returned an error response: {0}")]
    NiriError(String),
    #[error("niri response did not contain outputs")]
    MissingOutputs,
}

pub fn parse_outputs_json(contents: &str) -> Result<Vec<NiriOutput>, NiriOutputParseError> {
    let value: Value = serde_json::from_str(contents)?;
    parse_outputs_value(value)
}

pub fn parse_outputs_response_json(
    contents: &str,
) -> Result<Vec<NiriOutput>, NiriOutputParseError> {
    let value: Value = serde_json::from_str(contents)?;
    parse_outputs_value(extract_outputs_value(value)?)
}

fn parse_outputs_value(value: Value) -> Result<Vec<NiriOutput>, NiriOutputParseError> {
    let raw_outputs: BTreeMap<String, RawOutput> = serde_json::from_value(value)?;
    Ok(raw_outputs
        .into_iter()
        .map(|(connector, raw)| normalize_output(connector, raw))
        .collect())
}

fn extract_outputs_value(value: Value) -> Result<Value, NiriOutputParseError> {
    match value {
        Value::Object(mut object) => {
            if let Some(error) = object.remove("Err") {
                return Err(NiriOutputParseError::NiriError(error.to_string()));
            }

            if let Some(ok) = object.remove("Ok") {
                return extract_outputs_value(ok);
            }

            if let Some(outputs) = object.remove("Outputs") {
                return Ok(outputs);
            }

            if let Some(outputs) = object.remove("outputs") {
                return Ok(outputs);
            }

            Ok(Value::Object(object))
        }
        _ => Err(NiriOutputParseError::MissingOutputs),
    }
}

fn normalize_output(connector: String, raw: RawOutput) -> NiriOutput {
    let connector = non_empty(raw.name).unwrap_or(connector);
    let make = non_empty(raw.make).unwrap_or_else(|| "Unknown".to_string());
    let model = non_empty(raw.model).unwrap_or_else(|| "Unknown".to_string());
    let serial = raw.serial.and_then(non_empty);
    let description_serial = serial.as_deref().unwrap_or("Unknown");
    let description = format!("{make} {model} {description_serial}");

    NiriOutput {
        connector,
        make,
        model,
        serial,
        description,
        modes: raw.modes.into_iter().map(normalize_mode).collect(),
        current_mode: raw.current_mode,
        logical: raw.logical.map(normalize_logical_output),
        vrr_supported: raw.vrr_supported,
        vrr_enabled: raw.vrr_enabled,
    }
}

fn normalize_mode(raw: RawMode) -> NiriMode {
    NiriMode {
        width: raw.width,
        height: raw.height,
        refresh_millihz: raw.refresh_rate,
        is_preferred: raw.is_preferred,
    }
}

fn normalize_logical_output(raw: RawLogicalOutput) -> LogicalOutput {
    LogicalOutput {
        x: raw.x,
        y: raw.y,
        width: raw.width,
        height: raw.height,
        scale: raw.scale,
        transform: normalize_transform(&raw.transform),
    }
}

fn normalize_transform(transform: &str) -> String {
    match transform {
        "Normal" | "normal" => "normal",
        "_90" | "90" => "90",
        "_180" | "180" => "180",
        "_270" | "270" => "270",
        "Flipped" | "flipped" => "flipped",
        "Flipped90" | "flipped-90" => "flipped-90",
        "Flipped180" | "flipped-180" => "flipped-180",
        "Flipped270" | "flipped-270" => "flipped-270",
        other => other,
    }
    .to_string()
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

#[derive(Debug, Deserialize)]
struct RawOutput {
    #[serde(default)]
    name: String,
    #[serde(default)]
    make: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    serial: Option<String>,
    #[serde(default)]
    modes: Vec<RawMode>,
    #[serde(default)]
    current_mode: Option<usize>,
    #[serde(default)]
    logical: Option<RawLogicalOutput>,
    #[serde(default)]
    vrr_supported: bool,
    #[serde(default)]
    vrr_enabled: bool,
}

#[derive(Debug, Deserialize)]
struct RawMode {
    width: u16,
    height: u16,
    refresh_rate: u32,
    #[serde(default)]
    is_preferred: bool,
}

#[derive(Debug, Deserialize)]
struct RawLogicalOutput {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    transform: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUTS_JSON: &str = r#"
{
  "DP-1": {
    "name": "DP-1",
    "make": "Dell Inc.",
    "model": "DELL U3419W",
    "serial": "7VK66T2",
    "physical_size": [800, 340],
    "modes": [
      { "width": 3440, "height": 1440, "refresh_rate": 59973, "is_preferred": true }
    ],
    "current_mode": 0,
    "is_custom_mode": false,
    "logical": { "x": 0, "y": 0, "width": 3440, "height": 1440, "scale": 1.0, "transform": "Normal" },
    "vrr_supported": true,
    "vrr_enabled": false,
    "future_field": "ignored"
  },
  "HDMI-A-1": {
    "name": "HDMI-A-1",
    "make": "Lenovo Group Limited",
    "model": "0x40A9",
    "serial": null,
    "modes": [
      { "width": 1920, "height": 1080, "refresh_rate": 60000, "is_preferred": true }
    ],
    "current_mode": null,
    "logical": null,
    "vrr_supported": false,
    "vrr_enabled": false
  }
}
"#;

    #[test]
    fn parses_reference_output_fixture() {
        let outputs = parse_outputs_json(OUTPUTS_JSON).expect("outputs should parse");

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].connector, "DP-1");
        assert_eq!(outputs[0].description, "Dell Inc. DELL U3419W 7VK66T2");
        assert_eq!(outputs[0].current_mode, Some(0));
        assert_eq!(outputs[0].logical.as_ref().unwrap().transform, "normal");
        assert!((outputs[0].modes[0].refresh_hz() - 59.973).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_serial_becomes_unknown_in_description() {
        let outputs = parse_outputs_json(OUTPUTS_JSON).expect("outputs should parse");

        assert_eq!(outputs[1].serial, None);
        assert_eq!(
            outputs[1].description,
            "Lenovo Group Limited 0x40A9 Unknown"
        );
    }

    #[test]
    fn disabled_outputs_have_no_logical_data() {
        let outputs = parse_outputs_json(OUTPUTS_JSON).expect("outputs should parse");

        assert_eq!(outputs[1].current_mode, None);
        assert_eq!(outputs[1].logical, None);
    }

    #[test]
    fn parses_socket_ok_outputs_wrapper() {
        let response = format!(r#"{{"Ok":{{"Outputs":{OUTPUTS_JSON}}}}}"#);
        let outputs = parse_outputs_response_json(&response).expect("wrapped outputs should parse");

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].connector, "DP-1");
    }
}
