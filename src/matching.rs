use crate::model::{Config, MonitorMatcher, Profile};
use crate::niri::output::NiriOutput;

pub fn matcher_matches_output(matcher: &MonitorMatcher, output: &NiriOutput) -> bool {
    field_matches(matcher.connector.as_deref(), &output.connector)
        && field_matches(matcher.description.as_deref(), &output.description)
        && field_matches(matcher.make.as_deref(), &output.make)
        && field_matches(matcher.model.as_deref(), &output.model)
        && serial_matches(matcher.serial.as_deref(), output.serial.as_deref())
}

pub fn matcher_matches_any_output(matcher: &MonitorMatcher, outputs: &[NiriOutput]) -> bool {
    outputs
        .iter()
        .any(|output| matcher_matches_output(matcher, output))
}

/// Whether the profile applies to the connected outputs. Every `all_connected` matcher
/// and every output entry's `match` must match a connected output, at least one
/// `any_connected` matcher must match when there are any, and no `none_connected`
/// matcher may match.
pub fn profile_matches_outputs(profile: &Profile, outputs: &[NiriOutput]) -> bool {
    let condition = &profile.condition;
    required_matchers(profile)
        .into_iter()
        .all(|matcher| matcher_matches_any_output(matcher, outputs))
        && (condition.any_connected.is_empty()
            || condition
                .any_connected
                .iter()
                .any(|matcher| matcher_matches_any_output(matcher, outputs)))
        && condition
            .none_connected
            .iter()
            .all(|matcher| !matcher_matches_any_output(matcher, outputs))
}

/// The condition's `all_connected` matchers followed by the output entries' matchers,
/// without duplicates.
fn required_matchers(profile: &Profile) -> Vec<&MonitorMatcher> {
    let mut required = Vec::new();
    for matcher in profile
        .condition
        .all_connected
        .iter()
        .chain(profile.outputs.iter().map(|output| &output.matcher))
    {
        if !required.contains(&matcher) {
            required.push(matcher);
        }
    }
    required
}

pub fn select_profile<'a>(
    config: &'a Config,
    outputs: &[NiriOutput],
    manual_profile: Option<&str>,
) -> Option<&'a Profile> {
    if let Some(manual_profile) = manual_profile
        && let Some(profile) = config.profiles.iter().find(|profile| {
            profile.id == manual_profile
                && profile.enabled
                && profile_matches_outputs(profile, outputs)
        })
    {
        return Some(profile);
    }

    let mut best: Option<(&Profile, i32)> = None;
    for profile in &config.profiles {
        if !profile.enabled || !profile_matches_outputs(profile, outputs) {
            continue;
        }

        let specificity = specificity_score(profile);
        if best
            .as_ref()
            .is_none_or(|(best_profile, best_specificity)| {
                profile.priority > best_profile.priority
                    || (profile.priority == best_profile.priority
                        && specificity > *best_specificity)
            })
        {
            best = Some((profile, specificity));
        }
    }

    best.map(|(profile, _)| profile)
}

pub fn specificity_score(profile: &Profile) -> i32 {
    matcher_group_score(required_matchers(profile), 10)
        + matcher_group_score(&profile.condition.any_connected, 3)
        + matcher_group_score(&profile.condition.none_connected, 2)
}

fn matcher_group_score<'a>(
    matchers: impl IntoIterator<Item = &'a MonitorMatcher>,
    base_score: i32,
) -> i32 {
    matchers
        .into_iter()
        .map(|matcher| base_score + matcher_field_count(matcher))
        .sum()
}

fn matcher_field_count(matcher: &MonitorMatcher) -> i32 {
    [
        matcher.connector.as_ref(),
        matcher.description.as_ref(),
        matcher.make.as_ref(),
        matcher.model.as_ref(),
        matcher.serial.as_ref(),
    ]
    .into_iter()
    .flatten()
    .count() as i32
}

fn field_matches(expected: Option<&str>, actual: &str) -> bool {
    expected.is_none_or(|expected| expected == actual)
}

fn serial_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    expected.is_none_or(|expected| expected == actual.unwrap_or("Unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transform;
    use crate::model::{DaemonConfig, ProfileCondition, ProfileOutput};
    use crate::niri::output::{LogicalOutput, NiriMode};

    #[test]
    fn matches_monitors_by_connector_description_and_identity_fields() {
        let output = output("DP-1", "Dell Inc.", "DELL U3419W", Some("7VK66T2"));

        assert!(matcher_matches_output(
            &MonitorMatcher {
                connector: Some("DP-1".to_string()),
                ..MonitorMatcher::default()
            },
            &output
        ));
        assert!(matcher_matches_output(
            &MonitorMatcher {
                description: Some("Dell Inc. DELL U3419W 7VK66T2".to_string()),
                ..MonitorMatcher::default()
            },
            &output
        ));
        assert!(matcher_matches_output(
            &MonitorMatcher {
                make: Some("Dell Inc.".to_string()),
                model: Some("DELL U3419W".to_string()),
                serial: Some("7VK66T2".to_string()),
                ..MonitorMatcher::default()
            },
            &output
        ));
    }

    #[test]
    fn selects_highest_priority_matching_profile() {
        let outputs = vec![output("DP-1", "Dell Inc.", "DELL U3419W", Some("7VK66T2"))];
        let config = Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![
                profile("fallback", 0, ProfileCondition::default()),
                profile(
                    "home",
                    100,
                    ProfileCondition {
                        all_connected: vec![matcher_description("Dell Inc. DELL U3419W 7VK66T2")],
                        ..ProfileCondition::default()
                    },
                ),
            ],
        };

        let selected = select_profile(&config, &outputs, None).expect("profile should match");

        assert_eq!(selected.id, "home");
    }

    #[test]
    fn breaks_priority_ties_by_specificity_then_config_order() {
        let outputs = vec![output("DP-1", "Dell Inc.", "DELL U3419W", Some("7VK66T2"))];
        let config = Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![
                profile("first", 10, ProfileCondition::default()),
                profile("second", 10, ProfileCondition::default()),
                profile(
                    "specific",
                    10,
                    ProfileCondition {
                        any_connected: vec![matcher_description("Dell Inc. DELL U3419W 7VK66T2")],
                        ..ProfileCondition::default()
                    },
                ),
            ],
        };

        let selected = select_profile(&config, &outputs, None).expect("profile should match");

        assert_eq!(selected.id, "specific");

        let tied_config = Config {
            profiles: config.profiles[..2].to_vec(),
            ..config
        };
        let selected = select_profile(&tied_config, &outputs, None).expect("profile should match");

        assert_eq!(selected.id, "first");
    }

    #[test]
    fn profile_requires_its_output_entries_to_be_connected() {
        let dell = output("DP-1", "Dell Inc.", "DELL U3419W", Some("7VK66T2"));
        let lenovo = output("HDMI-A-1", "Lenovo", "T27", None);
        let mut home = profile("home", 0, ProfileCondition::default());
        home.outputs = vec![output_entry("Dell Inc. DELL U3419W 7VK66T2")];

        assert!(profile_matches_outputs(
            &home,
            &[dell.clone(), lenovo.clone()]
        ));
        assert!(!profile_matches_outputs(&home, &[lenovo]));

        home.condition.none_connected = vec![matcher_description("Dell Inc. DELL U3419W 7VK66T2")];
        assert!(!profile_matches_outputs(&home, &[dell]));
    }

    #[test]
    fn output_entries_count_toward_specificity_once() {
        let outputs = vec![output("DP-1", "Dell Inc.", "DELL U3419W", Some("7VK66T2"))];
        let mut duplicated = profile(
            "duplicated",
            0,
            ProfileCondition {
                all_connected: vec![matcher_description("Dell Inc. DELL U3419W 7VK66T2")],
                ..ProfileCondition::default()
            },
        );
        duplicated.outputs = vec![output_entry("Dell Inc. DELL U3419W 7VK66T2")];
        let mut implicit = profile("implicit", 0, ProfileCondition::default());
        implicit.outputs = vec![output_entry("Dell Inc. DELL U3419W 7VK66T2")];

        assert_eq!(specificity_score(&duplicated), specificity_score(&implicit));
        assert!(specificity_score(&implicit) > 0);

        let config = Config {
            version: 1,
            daemon: DaemonConfig::default(),
            profiles: vec![
                profile("fallback", 0, ProfileCondition::default()),
                implicit,
            ],
        };
        let selected = select_profile(&config, &outputs, None).expect("profile should match");

        assert_eq!(selected.id, "implicit");
    }

    fn output_entry(description: &str) -> ProfileOutput {
        ProfileOutput {
            matcher: matcher_description(description),
            enabled: Some(true),
            mode: None,
            scale: None,
            transform: None,
            position: None,
            vrr: None,
        }
    }

    fn profile(id: &str, priority: i32, condition: ProfileCondition) -> Profile {
        Profile {
            id: id.to_string(),
            name: id.to_string(),
            priority,
            enabled: true,
            condition,
            outputs: Vec::new(),
        }
    }

    fn matcher_description(description: &str) -> MonitorMatcher {
        MonitorMatcher {
            description: Some(description.to_string()),
            ..MonitorMatcher::default()
        }
    }

    fn output(connector: &str, make: &str, model: &str, serial: Option<&str>) -> NiriOutput {
        let serial_string = serial.map(str::to_string);
        let description = format!("{} {} {}", make, model, serial.unwrap_or("Unknown"));

        NiriOutput {
            connector: connector.to_string(),
            make: make.to_string(),
            model: model.to_string(),
            serial: serial_string,
            description,
            modes: vec![NiriMode {
                width: 3440,
                height: 1440,
                refresh_millihz: 59973,
                is_preferred: true,
            }],
            current_mode: Some(0),
            logical: Some(LogicalOutput {
                x: 0,
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
}
