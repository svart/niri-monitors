use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    #[serde(default = "default_auto_apply")]
    pub auto_apply: bool,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_prevent_disable_all")]
    pub prevent_disable_all: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            auto_apply: default_auto_apply(),
            poll_interval_ms: default_poll_interval_ms(),
            debounce_ms: default_debounce_ms(),
            prevent_disable_all: default_prevent_disable_all(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_profile_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "ProfileCondition::is_empty")]
    pub condition: ProfileCondition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<ProfileOutput>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCondition {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all_connected: Vec<MonitorMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any_connected: Vec<MonitorMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub none_connected: Vec<MonitorMatcher>,
}

impl ProfileCondition {
    pub fn is_empty(&self) -> bool {
        self.all_connected.is_empty()
            && self.any_connected.is_empty()
            && self.none_connected.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorMatcher {
    #[serde(default)]
    pub connector: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub serial: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileOutput {
    #[serde(rename = "match")]
    pub matcher: MonitorMatcher,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub mode: Option<ModeSpec>,
    #[serde(default)]
    pub scale: Option<f64>,
    #[serde(default)]
    pub transform: Option<Transform>,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub vrr: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// An output mode written as `WIDTHxHEIGHT` or `WIDTHxHEIGHT@HZ`. The refresh rate is
/// kept in millihertz, the unit niri reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModeSpec {
    pub width: u16,
    pub height: u16,
    pub refresh_millihz: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid mode {mode:?}: {reason}")]
pub struct ParseModeError {
    mode: String,
    reason: &'static str,
}

impl FromStr for ModeSpec {
    type Err = ParseModeError;

    fn from_str(mode: &str) -> Result<Self, Self::Err> {
        let error = |reason| ParseModeError {
            mode: mode.to_owned(),
            reason,
        };
        let (size, refresh) = match mode.split_once('@') {
            Some((size, refresh)) => (size, Some(refresh)),
            None => (mode, None),
        };
        let (width, height) = size
            .split_once('x')
            .ok_or_else(|| error("expected WIDTHxHEIGHT or WIDTHxHEIGHT@HZ"))?;
        let width = width
            .parse()
            .map_err(|_| error("width must be a whole number up to 65535"))?;
        let height = height
            .parse()
            .map_err(|_| error("height must be a whole number up to 65535"))?;
        let refresh_millihz = refresh
            .map(|refresh| {
                let hz: f64 = refresh
                    .parse()
                    .map_err(|_| error("refresh rate must be a number in Hz"))?;
                let millihz = (hz * 1000.0).round();
                if (1.0..=f64::from(u32::MAX)).contains(&millihz) {
                    Ok(millihz as u32)
                } else {
                    Err(error("refresh rate must be positive"))
                }
            })
            .transpose()?;

        Ok(Self {
            width,
            height,
            refresh_millihz,
        })
    }
}

impl fmt::Display for ModeSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)?;
        if let Some(millihz) = self.refresh_millihz {
            let refresh = format!("{}.{:03}", millihz / 1000, millihz % 1000);
            write!(
                f,
                "@{}",
                refresh.trim_end_matches('0').trim_end_matches('.')
            )?;
        }
        Ok(())
    }
}

impl TryFrom<String> for ModeSpec {
    type Error = ParseModeError;

    fn try_from(mode: String) -> Result<Self, Self::Error> {
        mode.parse()
    }
}

impl From<ModeSpec> for String {
    fn from(mode: ModeSpec) -> Self {
        mode.to_string()
    }
}

/// Output rotation and flip. Serialized with the config names; the aliases accept the
/// names niri's JSON IPC uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transform {
    #[serde(rename = "normal", alias = "Normal")]
    Normal,
    #[serde(rename = "90", alias = "_90")]
    Rotate90,
    #[serde(rename = "180", alias = "_180")]
    Rotate180,
    #[serde(rename = "270", alias = "_270")]
    Rotate270,
    #[serde(rename = "flipped", alias = "Flipped")]
    Flipped,
    #[serde(rename = "flipped-90", alias = "Flipped90")]
    Flipped90,
    #[serde(rename = "flipped-180", alias = "Flipped180")]
    Flipped180,
    #[serde(rename = "flipped-270", alias = "Flipped270")]
    Flipped270,
}

impl Transform {
    pub const ALL: [Self; 8] = [
        Self::Normal,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::Flipped,
        Self::Flipped90,
        Self::Flipped180,
        Self::Flipped270,
    ];

    /// The name used in the config file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Rotate90 => "90",
            Self::Rotate180 => "180",
            Self::Rotate270 => "270",
            Self::Flipped => "flipped",
            Self::Flipped90 => "flipped-90",
            Self::Flipped180 => "flipped-180",
            Self::Flipped270 => "flipped-270",
        }
    }

    /// The name niri's JSON IPC uses.
    pub fn niri_name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Rotate90 => "_90",
            Self::Rotate180 => "_180",
            Self::Rotate270 => "_270",
            Self::Flipped => "Flipped",
            Self::Flipped90 => "Flipped90",
            Self::Flipped180 => "Flipped180",
            Self::Flipped270 => "Flipped270",
        }
    }

    /// Whether the output's width and height trade places.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

impl fmt::Display for Transform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Requested runtime position for one connected output, by connector name, and the mode,
/// scale, and rotation to change. A `None` setting keeps its current value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManualOutputPlacement {
    pub output: String,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
}

fn default_auto_apply() -> bool {
    true
}

fn default_poll_interval_ms() -> u64 {
    1500
}

fn default_debounce_ms() -> u64 {
    500
}

fn default_prevent_disable_all() -> bool {
    true
}

fn default_profile_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_spec_round_trips_through_text() {
        for text in [
            "3440x1440@59.973",
            "2560x1440@60",
            "3840x2160@59.94",
            "1920x1080",
        ] {
            let mode: ModeSpec = text.parse().expect("mode should parse");
            assert_eq!(mode.to_string(), text);
        }

        assert_eq!(
            "3440x1440@59.973".parse::<ModeSpec>().unwrap(),
            ModeSpec {
                width: 3440,
                height: 1440,
                refresh_millihz: Some(59973),
            }
        );
    }

    #[test]
    fn mode_spec_rejects_malformed_text() {
        for text in [
            "",
            "3440x",
            "3440",
            "wide",
            "1920x1080@",
            "1920x1080@0",
            "1920x1080@fast",
            "1920x1080@60Hz",
            "70000x1080",
        ] {
            assert!(
                text.parse::<ModeSpec>().is_err(),
                "{text:?} should not parse"
            );
        }
    }

    #[test]
    fn transform_uses_config_names_and_accepts_niri_names() {
        let from_config: Transform = serde_json::from_str(r#""flipped-90""#).unwrap();
        let from_niri: Transform = serde_json::from_str(r#""Flipped90""#).unwrap();

        assert_eq!(from_config, Transform::Flipped90);
        assert_eq!(from_niri, Transform::Flipped90);
        assert_eq!(
            serde_json::to_string(&Transform::Rotate90).unwrap(),
            r#""90""#
        );
        assert!(serde_json::from_str::<Transform>(r#""sideways""#).is_err());
    }
}
