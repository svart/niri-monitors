use crate::model::{Config, DaemonConfig, MonitorMatcher};
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const SUPPORTED_CONFIG_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write config {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to encode config: {0}")]
    Encode(#[from] toml::ser::Error),
    #[error("could not resolve config path: set XDG_CONFIG_HOME or HOME")]
    MissingConfigHome,
    #[error("unsupported config version {0}; supported version is {SUPPORTED_CONFIG_VERSION}")]
    UnsupportedVersion(u32),
    #[error("profile id must be non-empty")]
    EmptyProfileId,
    #[error("duplicate profile id: {0}")]
    DuplicateProfileId(String),
    #[error(
        "profile {profile_id} output {output_index} has invalid scale {scale}; scale must be finite and greater than zero"
    )]
    InvalidScale {
        profile_id: String,
        output_index: usize,
        scale: f64,
    },
    #[error("profile {profile_id} output {output_index} has invalid transform {transform}")]
    InvalidTransform {
        profile_id: String,
        output_index: usize,
        transform: String,
    },
    #[error(
        "profile {profile_id} {context} matcher {matcher_index} must set at least one match field"
    )]
    EmptyMatcher {
        profile_id: String,
        context: String,
        matcher_index: usize,
    },
}

pub fn load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_config(&contents, path)
}

pub fn load_config_or_empty(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(empty_config()),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if contents.trim().is_empty() {
        return Ok(empty_config());
    }

    parse_config(&contents, path)
}

pub fn empty_config() -> Config {
    Config {
        version: SUPPORTED_CONFIG_VERSION,
        daemon: DaemonConfig::default(),
        profiles: Vec::new(),
    }
}

pub fn save_config(path: impl AsRef<Path>, config: &Config) -> Result<(), ConfigError> {
    validate_config(config)?;
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let contents = toml::to_string_pretty(config)?;
    fs::write(path, contents).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

pub fn save_config_with_backup(path: impl AsRef<Path>, config: &Config) -> Result<(), ConfigError> {
    validate_config(config)?;
    let path = path.as_ref();

    if path.exists() {
        let backup_path = backup_config_path(path);
        fs::copy(path, &backup_path).map_err(|source| ConfigError::Write {
            path: backup_path,
            source,
        })?;
    }

    save_config(path, config)
}

pub fn parse_config(contents: &str, path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    let config: Config = toml::from_str(contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    validate_config(&config)?;
    Ok(config)
}

pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    if config.version != SUPPORTED_CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion(config.version));
    }

    let mut profile_ids = HashSet::new();
    for profile in &config.profiles {
        let id = profile.id.trim();
        if id.is_empty() {
            return Err(ConfigError::EmptyProfileId);
        }
        if !profile_ids.insert(id) {
            return Err(ConfigError::DuplicateProfileId(profile.id.clone()));
        }

        for (context, matchers) in [
            ("all_connected", profile.condition.all_connected.as_slice()),
            ("any_connected", profile.condition.any_connected.as_slice()),
            (
                "none_connected",
                profile.condition.none_connected.as_slice(),
            ),
        ] {
            for (matcher_index, matcher) in matchers.iter().enumerate() {
                validate_matcher(&profile.id, context, matcher_index, matcher)?;
            }
        }

        for (output_index, output) in profile.outputs.iter().enumerate() {
            validate_matcher(&profile.id, "outputs", output_index, &output.matcher)?;

            if let Some(scale) = output.scale
                && (!scale.is_finite() || scale <= 0.0)
            {
                return Err(ConfigError::InvalidScale {
                    profile_id: profile.id.clone(),
                    output_index,
                    scale,
                });
            }

            if let Some(transform) = &output.transform
                && !is_valid_transform(transform)
            {
                return Err(ConfigError::InvalidTransform {
                    profile_id: profile.id.clone(),
                    output_index,
                    transform: transform.clone(),
                });
            }
        }
    }

    Ok(())
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    default_config_path_from_env(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
        .ok_or(ConfigError::MissingConfigHome)
}

fn backup_config_path(path: &Path) -> PathBuf {
    let mut backup_name = path
        .file_name()
        .map(|file_name| file_name.to_os_string())
        .unwrap_or_else(|| OsString::from("config.toml"));
    backup_name.push(".bak");
    path.with_file_name(backup_name)
}

fn default_config_path_from_env(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(xdg_config_home) = xdg_config_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(xdg_config_home).join("niri-monitors/config.toml"));
    }

    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config/niri-monitors/config.toml"))
}

pub fn is_valid_transform(transform: &str) -> bool {
    matches!(
        transform,
        "normal" | "90" | "180" | "270" | "flipped" | "flipped-90" | "flipped-180" | "flipped-270"
    )
}

fn validate_matcher(
    profile_id: &str,
    context: &str,
    matcher_index: usize,
    matcher: &MonitorMatcher,
) -> Result<(), ConfigError> {
    if matcher_has_field(matcher) {
        Ok(())
    } else {
        Err(ConfigError::EmptyMatcher {
            profile_id: profile_id.to_owned(),
            context: context.to_owned(),
            matcher_index,
        })
    }
}

fn matcher_has_field(matcher: &MonitorMatcher) -> bool {
    [
        matcher.connector.as_ref(),
        matcher.description.as_ref(),
        matcher.make.as_ref(),
        matcher.model.as_ref(),
        matcher.serial.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_CONFIG: &str = r#"
version = 1

[daemon]
auto_apply = true
poll_interval_ms = 1500
debounce_ms = 500
prevent_disable_all = true

[[profiles]]
id = "home"
name = "Home"
priority = 100
enabled = true

[profiles.condition]
all_connected = [
  { description = "Dell Inc. DELL U3419W 7VK66T2" },
  { description = "Lenovo Group Limited 0x40A9 Unknown" },
]
none_connected = []

[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
enabled = true
mode = "3440x1440@59.973"
scale = 1.0
transform = "normal"
position = { x = 0, y = 0 }
vrr = false

[[profiles.outputs]]
match = { description = "Lenovo Group Limited 0x40A9 Unknown" }
enabled = true
scale = 1.25
position = { x = 3440, y = 288 }
"#;

    #[test]
    fn parses_example_config() {
        let config =
            parse_config(EXAMPLE_CONFIG, "example.toml").expect("example config should parse");

        assert_eq!(config.version, 1);
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].id, "home");
        assert_eq!(config.profiles[0].outputs.len(), 2);
    }

    #[test]
    fn rejects_empty_profile_ids() {
        let config = r#"
version = 1

[[profiles]]
id = "   "
name = "Broken"
"#;

        let error = parse_config(config, "bad.toml").expect_err("empty id should fail");
        assert!(matches!(error, ConfigError::EmptyProfileId));
    }

    #[test]
    fn rejects_duplicate_profile_ids() {
        let config = r#"
version = 1

[[profiles]]
id = "home"
name = "Home"

[[profiles]]
id = "home"
name = "Duplicate"
"#;

        let error = parse_config(config, "bad.toml").expect_err("duplicate id should fail");
        assert!(matches!(error, ConfigError::DuplicateProfileId(id) if id == "home"));
    }

    #[test]
    fn rejects_invalid_scale() {
        let config = r#"
version = 1

[[profiles]]
id = "home"
name = "Home"

[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
scale = 0.0
"#;

        let error = parse_config(config, "bad.toml").expect_err("zero scale should fail");
        assert!(matches!(error, ConfigError::InvalidScale { .. }));
    }

    #[test]
    fn rejects_invalid_transform() {
        let config = r#"
version = 1

[[profiles]]
id = "home"
name = "Home"

[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
transform = "sideways"
"#;

        let error = parse_config(config, "bad.toml").expect_err("invalid transform should fail");
        assert!(matches!(error, ConfigError::InvalidTransform { .. }));
    }

    #[test]
    fn rejects_empty_condition_matchers() {
        let config = r#"
version = 1

[[profiles]]
id = "home"
name = "Home"

[profiles.condition]
all_connected = [ {} ]
"#;

        let error = parse_config(config, "bad.toml").expect_err("empty matcher should fail");
        assert!(matches!(
            error,
            ConfigError::EmptyMatcher { context, .. } if context == "all_connected"
        ));
    }

    #[test]
    fn rejects_empty_output_matchers() {
        let config = r#"
version = 1

[[profiles]]
id = "home"
name = "Home"

[[profiles.outputs]]
match = {}
enabled = true
"#;

        let error = parse_config(config, "bad.toml").expect_err("empty matcher should fail");
        assert!(matches!(
            error,
            ConfigError::EmptyMatcher { context, .. } if context == "outputs"
        ));
    }

    #[test]
    fn resolves_default_path_from_xdg_config_home() {
        let path =
            default_config_path_from_env(Some("/tmp/config".into()), Some("/tmp/home".into()))
                .expect("path should resolve");

        assert_eq!(path, PathBuf::from("/tmp/config/niri-monitors/config.toml"));
    }

    #[test]
    fn resolves_default_path_from_home() {
        let path = default_config_path_from_env(None, Some("/tmp/home".into()))
            .expect("path should resolve");

        assert_eq!(
            path,
            PathBuf::from("/tmp/home/.config/niri-monitors/config.toml")
        );
    }

    #[test]
    fn save_with_backup_preserves_existing_config() {
        let path = temp_config_path("save-with-backup");
        fs::write(&path, EXAMPLE_CONFIG).expect("existing config should be written");
        let mut config = parse_config(EXAMPLE_CONFIG, &path).expect("example config should parse");
        config.profiles[0].name = "Renamed".to_owned();

        save_config_with_backup(&path, &config).expect("config should save");

        let backup = fs::read_to_string(backup_config_path(&path)).expect("backup should exist");
        assert_eq!(backup, EXAMPLE_CONFIG);
        let saved = load_config(&path).expect("saved config should load");
        assert_eq!(saved.profiles[0].name, "Renamed");

        fs::remove_dir_all(path.parent().expect("temp config should have parent"))
            .expect("temp config directory should be removed");
    }

    #[test]
    fn load_config_or_empty_uses_default_when_file_is_missing() {
        let path = temp_config_path("missing-config");
        assert!(!path.exists());

        let config = load_config_or_empty(&path).expect("missing config should load as empty");

        assert_eq!(config.version, SUPPORTED_CONFIG_VERSION);
        assert_eq!(config.daemon, crate::model::DaemonConfig::default());
        assert!(config.profiles.is_empty());

        fs::remove_dir_all(path.parent().expect("temp config should have parent"))
            .expect("temp config directory should be removed");
    }

    #[test]
    fn load_config_or_empty_uses_default_when_file_is_empty() {
        let path = temp_config_path("empty-config");
        fs::write(&path, "\n  \n").expect("empty config file should be written");

        let config = load_config_or_empty(&path).expect("empty config should load as empty");

        assert_eq!(config.version, SUPPORTED_CONFIG_VERSION);
        assert_eq!(config.daemon, crate::model::DaemonConfig::default());
        assert!(config.profiles.is_empty());

        fs::remove_dir_all(path.parent().expect("temp config should have parent"))
            .expect("temp config directory should be removed");
    }

    fn temp_config_path(test_name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "niri-monitors-{test_name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temp config directory should be created");
        directory.join("config.toml")
    }
}
