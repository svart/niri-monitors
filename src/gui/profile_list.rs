use crate::model::{MonitorMatcher, Position, Profile, ProfileCondition, ProfileOutput};
use crate::niri::output::{NiriMode, NiriOutput};
use std::collections::HashSet;

pub fn build_new_profile(existing: &[Profile], connected_outputs: &[NiriOutput]) -> Profile {
    let name = next_profile_name(existing);
    Profile {
        id: unique_profile_id(existing, &name),
        name,
        priority: next_profile_priority(existing),
        enabled: true,
        condition: ProfileCondition {
            all_connected: connected_outputs.iter().map(matcher_from_output).collect(),
            any_connected: Vec::new(),
            none_connected: Vec::new(),
        },
        outputs: connected_outputs
            .iter()
            .map(profile_output_from_output)
            .collect(),
    }
}

pub fn duplicate_profile(existing: &[Profile], source: &Profile) -> Profile {
    let mut duplicated = source.clone();
    duplicated.id = unique_profile_id(existing, &format!("{} Copy", source.id));
    duplicated.name = unique_profile_name(existing, &format!("{} Copy", source.name));
    duplicated
}

pub fn remove_profile(profiles: &mut Vec<Profile>, profile_id: &str) -> bool {
    let Some(index) = profiles.iter().position(|profile| profile.id == profile_id) else {
        return false;
    };
    profiles.remove(index);
    true
}

fn next_profile_name(existing: &[Profile]) -> String {
    let names = existing
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<HashSet<_>>();

    for index in 1.. {
        let candidate = format!("Profile {index}");
        if !names.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("unbounded profile name search should always return")
}

fn next_profile_priority(existing: &[Profile]) -> i32 {
    existing
        .iter()
        .map(|profile| profile.priority)
        .max()
        .map(|priority| priority.saturating_add(1))
        .unwrap_or(0)
}

fn unique_profile_name(existing: &[Profile], base: &str) -> String {
    let names = existing
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<HashSet<_>>();
    if !names.contains(base) {
        return base.to_owned();
    }

    for index in 2.. {
        let candidate = format!("{base} {index}");
        if !names.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("unbounded profile name search should always return")
}

fn unique_profile_id(existing: &[Profile], base: &str) -> String {
    let base = profile_id_from_name(base);
    let ids = existing
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<HashSet<_>>();
    if !ids.contains(base.as_str()) {
        return base;
    }

    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !ids.contains(candidate.as_str()) {
            return candidate;
        }
    }

    unreachable!("unbounded profile id search should always return")
}

fn profile_id_from_name(name: &str) -> String {
    let mut id = String::new();
    let mut last_was_separator = false;

    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            id.push(character);
            last_was_separator = false;
        } else if !last_was_separator && !id.is_empty() {
            id.push('-');
            last_was_separator = true;
        }
    }

    while id.ends_with('-') {
        id.pop();
    }

    if id.is_empty() {
        "profile".to_owned()
    } else {
        id
    }
}

fn matcher_from_output(output: &NiriOutput) -> MonitorMatcher {
    MonitorMatcher {
        connector: None,
        description: Some(output.description.clone()),
        make: None,
        model: None,
        serial: None,
    }
}

fn profile_output_from_output(output: &NiriOutput) -> ProfileOutput {
    let logical = output.logical.as_ref();
    ProfileOutput {
        matcher: matcher_from_output(output),
        enabled: Some(logical.is_some()),
        mode: output
            .current_mode
            .and_then(|index| output.modes.get(index).copied())
            .map(format_mode),
        scale: logical.map(|logical| logical.scale),
        transform: logical.map(|logical| logical.transform.clone()),
        position: logical.map(|logical| Position {
            x: logical.x,
            y: logical.y,
        }),
        vrr: output.vrr_supported.then_some(output.vrr_enabled),
    }
}

fn format_mode(mode: NiriMode) -> String {
    let mut refresh = format!("{:.3}", mode.refresh_hz());
    while refresh.contains('.') && refresh.ends_with('0') {
        refresh.pop();
    }
    if refresh.ends_with('.') {
        refresh.pop();
    }
    format!("{}x{}@{}", mode.width, mode.height, refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProfileCondition;
    use crate::niri::output::LogicalOutput;

    #[test]
    fn new_profile_uses_next_name_priority_and_connected_outputs() {
        let existing = vec![profile("profile-1", "Profile 1", 4)];
        let output = output("DP-1", "Dell Inc. DELL 123", 0, 0);

        let profile = build_new_profile(&existing, &[output]);

        assert_eq!(profile.id, "profile-2");
        assert_eq!(profile.name, "Profile 2");
        assert_eq!(profile.priority, 5);
        assert_eq!(profile.condition.all_connected.len(), 1);
        assert_eq!(profile.outputs.len(), 1);
        assert_eq!(profile.outputs[0].mode.as_deref(), Some("3840x2160@59.94"));
        assert_eq!(profile.outputs[0].scale, Some(1.5));
        assert_eq!(profile.outputs[0].position, Some(Position { x: 0, y: 0 }));
    }

    #[test]
    fn duplicate_profile_uses_unique_id_and_name() {
        let source = profile("home", "Home", 10);
        let existing = vec![source.clone(), profile("home-copy", "Home Copy", 10)];

        let duplicated = duplicate_profile(&existing, &source);

        assert_eq!(duplicated.id, "home-copy-2");
        assert_eq!(duplicated.name, "Home Copy 2");
        assert_eq!(duplicated.priority, source.priority);
    }

    #[test]
    fn remove_profile_deletes_matching_profile() {
        let mut profiles = vec![profile("home", "Home", 0), profile("office", "Office", 1)];

        assert!(remove_profile(&mut profiles, "home"));

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, "office");
    }

    fn profile(id: &str, name: &str, priority: i32) -> Profile {
        Profile {
            id: id.to_owned(),
            name: name.to_owned(),
            priority,
            enabled: true,
            condition: ProfileCondition::default(),
            outputs: Vec::new(),
        }
    }

    fn output(connector: &str, description: &str, x: i32, y: i32) -> NiriOutput {
        NiriOutput {
            connector: connector.to_owned(),
            make: "Dell Inc.".to_owned(),
            model: "DELL".to_owned(),
            serial: Some("123".to_owned()),
            description: description.to_owned(),
            modes: vec![NiriMode {
                width: 3840,
                height: 2160,
                refresh_millihz: 59940,
                is_preferred: true,
            }],
            current_mode: Some(0),
            logical: Some(LogicalOutput {
                x,
                y,
                width: 2560,
                height: 1440,
                scale: 1.5,
                transform: "normal".to_owned(),
            }),
            vrr_supported: true,
            vrr_enabled: false,
        }
    }
}
