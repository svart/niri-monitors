use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_profile_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub condition: ProfileCondition,
    #[serde(default)]
    pub outputs: Vec<ProfileOutput>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileCondition {
    #[serde(default)]
    pub all_connected: Vec<MonitorMatcher>,
    #[serde(default)]
    pub any_connected: Vec<MonitorMatcher>,
    #[serde(default)]
    pub none_connected: Vec<MonitorMatcher>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct ProfileOutput {
    #[serde(rename = "match")]
    pub matcher: MonitorMatcher,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub scale: Option<f64>,
    #[serde(default)]
    pub transform: Option<String>,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub vrr: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
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
