use crate::model::{Config, DaemonConfig, MonitorMatcher};
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use toml_edit::{ArrayOfTables, InlineTable, Item, Table, Value};

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
    Encode(#[from] toml_edit::ser::Error),
    #[error("could not resolve config path: set XDG_CONFIG_HOME or HOME")]
    MissingConfigHome,
    #[error("unsupported config version {0}; supported version is {SUPPORTED_CONFIG_VERSION}")]
    UnsupportedVersion(u32),
    #[error("profile name must be non-empty")]
    EmptyProfileName,
    #[error("duplicate profile name: {0:?}")]
    DuplicateProfileName(String),
    #[error(
        "profile {profile:?} output {output_index} has invalid scale {scale}; scale must be finite and greater than zero"
    )]
    InvalidScale {
        profile: String,
        output_index: usize,
        scale: f64,
    },
    #[error(
        "profile {profile:?} {context} matcher {matcher_index} must set at least one match field"
    )]
    EmptyMatcher {
        profile: String,
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

    let contents = config_to_toml(config)?;
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

/// Writes the config in the layout `docs/config.md` shows: sections, profiles,
/// conditions, and output entries as tables; matchers and positions inline, with one
/// matcher per line.
fn config_to_toml(config: &Config) -> Result<String, ConfigError> {
    let mut document = toml_edit::ser::to_document(config)?;
    for (_, item) in document.iter_mut() {
        layout_item(item, true);
    }
    Ok(document.to_string())
}

/// The serializer writes every nested value inline. A table becomes a standard table
/// when it is at the top level or holds other tables, and a list of such tables becomes
/// an array of tables. Other lists of values get one value per line.
fn layout_item(item: &mut Item, top_level: bool) {
    let expand = |table: &InlineTable| {
        top_level
            || table.iter().any(|(_, value)| match value {
                Value::InlineTable(_) => true,
                Value::Array(array) => array.iter().any(Value::is_inline_table),
                _ => false,
            })
    };

    *item = match std::mem::take(item) {
        Item::Value(Value::InlineTable(table)) if expand(&table) => {
            Item::Table(expand_table(table))
        }
        Item::Value(Value::Array(array))
            if !array.is_empty()
                && array
                    .iter()
                    .all(|value| value.as_inline_table().is_some_and(expand)) =>
        {
            let mut tables = ArrayOfTables::new();
            for value in array {
                if let Value::InlineTable(table) = value {
                    tables.push(expand_table(table));
                }
            }
            Item::ArrayOfTables(tables)
        }
        Item::Value(Value::Array(mut array)) if !array.is_empty() => {
            for value in array.iter_mut() {
                value.decor_mut().set_prefix("\n  ");
            }
            array.set_trailing("\n");
            array.set_trailing_comma(true);
            Item::Value(Value::Array(array))
        }
        other => other,
    };
}

fn expand_table(table: InlineTable) -> Table {
    let mut table = table.into_table();
    for (_, item) in table.iter_mut() {
        layout_item(item, false);
    }
    table
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

    let mut profile_names = HashSet::new();
    for profile in &config.profiles {
        let name = profile.name.trim();
        if name.is_empty() {
            return Err(ConfigError::EmptyProfileName);
        }
        if !profile_names.insert(name) {
            return Err(ConfigError::DuplicateProfileName(profile.name.clone()));
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
                validate_matcher(&profile.name, context, matcher_index, matcher)?;
            }
        }

        for (output_index, output) in profile.outputs.iter().enumerate() {
            validate_matcher(&profile.name, "outputs", output_index, &output.matcher)?;

            if let Some(scale) = output.scale
                && (!scale.is_finite() || scale <= 0.0)
            {
                return Err(ConfigError::InvalidScale {
                    profile: profile.name.clone(),
                    output_index,
                    scale,
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

fn validate_matcher(
    profile: &str,
    context: &str,
    matcher_index: usize,
    matcher: &MonitorMatcher,
) -> Result<(), ConfigError> {
    if matcher_has_field(matcher) {
        Ok(())
    } else {
        Err(ConfigError::EmptyMatcher {
            profile: profile.to_owned(),
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
        assert_eq!(config.profiles[0].name, "Home");
        assert_eq!(config.profiles[0].outputs.len(), 2);
    }

    #[test]
    fn rejects_empty_profile_names() {
        let config = r#"
version = 1

[[profiles]]
name = "   "
"#;

        let error = parse_config(config, "bad.toml").expect_err("empty name should fail");
        assert!(matches!(error, ConfigError::EmptyProfileName));
    }

    #[test]
    fn rejects_duplicate_profile_names() {
        let config = r#"
version = 1

[[profiles]]
name = "Home"

[[profiles]]
name = "Home "
"#;

        let error = parse_config(config, "bad.toml").expect_err("duplicate name should fail");
        assert!(matches!(error, ConfigError::DuplicateProfileName(name) if name == "Home "));
    }

    #[test]
    fn rejects_invalid_scale() {
        let config = r#"
version = 1

[[profiles]]
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
name = "Home"

[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
transform = "sideways"
"#;

        let error = parse_config(config, "bad.toml").expect_err("invalid transform should fail");
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert!(error.to_string().contains("sideways"));
    }

    #[test]
    fn rejects_malformed_mode() {
        let config = r#"
version = 1

[[profiles]]
name = "Home"

[[profiles.outputs]]
match = { connector = "DP-1" }
mode = "3440x1440@60Hz"
"#;

        let error = parse_config(config, "bad.toml").expect_err("malformed mode should fail");
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert!(error.to_string().contains("3440x1440@60Hz"));
    }

    #[test]
    fn saves_in_documented_layout() {
        let config = r#"version = 1

[daemon]
auto_apply = true
poll_interval_ms = 1500
debounce_ms = 500
prevent_disable_all = true

[[profiles]]
name = "Home"
priority = 100
enabled = true

[profiles.condition]
any_connected = [
  { connector = "DP-1" },
  { connector = "HDMI-A-1" },
]
none_connected = [
  { description = "Projector Vendor PROJECTOR Unknown" },
]

[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
enabled = true
mode = "3440x1440@59.973"
scale = 1.0
transform = "normal"
position = { x = 0, y = 0 }
vrr = false

[[profiles.outputs]]
match = { make = "Lenovo Group Limited", model = "0x40A9" }
enabled = false

[[profiles]]
name = "Fallback"
priority = 0
enabled = true
"#;

        let parsed = parse_config(config, "example.toml").expect("config should parse");

        assert_eq!(config_to_toml(&parsed).unwrap(), config);
    }

    #[test]
    fn saves_mode_and_transform_in_config_spelling() {
        let mut config = parse_config(EXAMPLE_CONFIG, "example.toml").unwrap();
        config.profiles[0].outputs[1].transform = Some(crate::model::Transform::Flipped90);

        let saved = config_to_toml(&config).unwrap();

        assert!(saved.contains(r#"mode = "3440x1440@59.973""#));
        assert!(saved.contains(r#"transform = "flipped-90""#));
    }

    #[test]
    fn rejects_unknown_fields() {
        for (typo, config) in [
            ("versoin", "versoin = 1"),
            ("poll_interval", "version = 1\n[daemon]\npoll_interval = 10"),
            ("profile", "version = 1\n[[profile]]\nname = \"A\""),
            (
                "priorty",
                "version = 1\n[[profiles]]\nname = \"A\"\npriorty = 1",
            ),
            (
                "all",
                "version = 1\n[[profiles]]\nname = \"A\"\n[profiles.condition]\nall = []",
            ),
            (
                "desc",
                "version = 1\n[[profiles]]\nname = \"A\"\n[[profiles.outputs]]\nmatch = { desc = \"Dell\" }",
            ),
            (
                "vr",
                "version = 1\n[[profiles]]\nname = \"A\"\n[[profiles.outputs]]\nmatch = { connector = \"DP-1\" }\nvr = true",
            ),
            (
                "z",
                "version = 1\n[[profiles]]\nname = \"A\"\n[[profiles.outputs]]\nmatch = { connector = \"DP-1\" }\nposition = { x = 0, y = 0, z = 0 }",
            ),
        ] {
            let error = parse_config(config, "bad.toml").expect_err("unknown field should fail");
            assert!(
                matches!(error, ConfigError::Parse { .. })
                    && error
                        .to_string()
                        .contains(&format!("unknown field `{typo}`")),
                "{typo:?} should be reported, got: {error}"
            );
        }
    }

    #[test]
    fn rejects_empty_condition_matchers() {
        let config = r#"
version = 1

[[profiles]]
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
