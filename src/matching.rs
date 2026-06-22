use crate::model::{Config, MonitorMatcher, Profile, ProfileCondition};
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

pub fn condition_matches_outputs(condition: &ProfileCondition, outputs: &[NiriOutput]) -> bool {
    condition
        .all_connected
        .iter()
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

pub fn profile_matches_outputs(profile: &Profile, outputs: &[NiriOutput]) -> bool {
    condition_matches_outputs(&profile.condition, outputs)
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

        let specificity = specificity_score(&profile.condition);
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

pub fn specificity_score(condition: &ProfileCondition) -> i32 {
    matcher_group_score(&condition.all_connected, 10)
        + matcher_group_score(&condition.any_connected, 3)
        + matcher_group_score(&condition.none_connected, 2)
}

fn matcher_group_score(matchers: &[MonitorMatcher], base_score: i32) -> i32 {
    matchers
        .iter()
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
    use crate::model::{DaemonConfig, ProfileCondition};
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
                transform: "normal".to_string(),
            }),
            vrr_supported: true,
            vrr_enabled: false,
        }
    }
}
