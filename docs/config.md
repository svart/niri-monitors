# Configuration

`niri-monitors` uses one TOML config file for both `niri-monitorsd` and `niri-monitors-gui`.

Use the GUI for first-time setup when possible. It can read live niri outputs, create matchers from output descriptions, copy current modes/scales/transforms/positions, and save valid TOML. Edit the file by hand when you need reviewable changes or automation.

## Path

The default config path is:

```text
$XDG_CONFIG_HOME/niri-monitors/config.toml
```

or, when `XDG_CONFIG_HOME` is not set:

```text
$HOME/.config/niri-monitors/config.toml
```

Use `niri-monitorsd --config PATH` to load a different file for the daemon. The GUI currently uses the default path.

If the file is missing or empty, the daemon starts with an empty in-memory config. It still polls niri outputs, listens on the control socket, and can reload later after a config file is saved. The GUI also starts with an empty draft and can show live monitor configuration directly from niri, falling back to daemon-reported outputs when needed. With no profiles, no profile-driven monitor changes are applied.

The smallest config file, if you want one on disk, is:

```toml
version = 1
```

## Example

```toml
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
```

Prefer creating profiles in `niri-monitors-gui` because it can populate connected output descriptions and current settings from niri, with daemon status as a fallback. To inspect raw niri output data yourself, use:

```sh
niri msg --json outputs
```

## Top-Level Fields

`version` is required when the file exists. The only supported value is `1`.

`[daemon]` is optional. Omitted daemon fields use defaults.

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `auto_apply` | boolean | `true` | Apply the selected profile automatically. If `false`, the daemon still observes outputs but applies profiles only through explicit control-socket actions. |
| `poll_interval_ms` | integer | `1500` | Delay between output-state polls. |
| `debounce_ms` | integer | `500` | Delay after an output change before reconciling, to avoid applying during transient hotplug state. |
| `prevent_disable_all` | boolean | `true` | Refuse plans that would disable every connected output. |

## Profiles

Profiles are declared with `[[profiles]]`. The list is optional; an empty list means the daemon observes output changes but does not select or apply a profile.

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | Stable unique profile id. Must not be empty. |
| `name` | string | required | Human-readable profile name shown by the GUI and daemon status. |
| `priority` | integer | `0` | Higher priority wins when multiple profiles match. |
| `enabled` | boolean | `true` | Disabled profiles are ignored. |
| `condition` | table | empty | Match rules for automatic profile selection. |
| `outputs` | array | empty | Output settings to apply when this profile is selected. |

Profile selection behavior:

- A manual layout applied through the control socket pauses automatic profile selection until automatic loading is enabled again.
- A manual profile requested through the control socket wins only if it is enabled and still matches connected outputs.
- Otherwise the daemon picks the enabled matching profile with the highest `priority`.
- Priority ties are broken by condition specificity.
- Exact ties keep config order.

Condition specificity is computed from matcher groups in `src/matching.rs`: `all_connected` contributes the most, then `any_connected`, then `none_connected`; matchers with more filled fields are more specific.

## Conditions

Conditions live under `[profiles.condition]` and contain matcher arrays.

```toml
[profiles.condition]
all_connected = [
  { description = "Dell Inc. DELL U3419W 7VK66T2" },
]
any_connected = [
  { connector = "DP-1" },
  { connector = "HDMI-A-1" },
]
none_connected = [
  { description = "Projector Vendor PROJECTOR Unknown" },
]
```

Condition semantics:

- `all_connected`: every matcher must match at least one connected output.
- `any_connected`: at least one matcher must match; an empty array is ignored.
- `none_connected`: every matcher must match no connected output.
- An empty condition matches any output state, which is useful for a low-priority fallback profile.

## Monitor Matchers

Matchers identify monitors. A matcher can use any of these fields:

| Field | Meaning |
| --- | --- |
| `connector` | niri connector name such as `DP-1` or `HDMI-A-1`. |
| `description` | Full niri output description. |
| `make` | Monitor vendor string. |
| `model` | Monitor model string. |
| `serial` | Monitor serial string. Missing serials are treated as `Unknown`. |

All fields in one matcher are exact-match AND conditions. For example, `{ make = "Dell Inc.", model = "DELL U3419W" }` matches only outputs with both fields.

Every matcher must set at least one field. Prefer `description` or `make` plus `model` plus `serial` when possible. Connector-only matchers are easy to write but can be unstable when cabling changes.

## Output Settings

Output settings are declared with `[[profiles.outputs]]` inside a profile.

```toml
[[profiles.outputs]]
match = { description = "Dell Inc. DELL U3419W 7VK66T2" }
enabled = true
mode = "3440x1440@59.973"
scale = 1.0
transform = "normal"
position = { x = 0, y = 0 }
vrr = false
```

`match` is required and must contain a non-empty monitor matcher. At apply time, it should resolve to exactly one connected output. If it matches no output, the daemon skips that output entry and reports a warning. If it matches multiple outputs, planning fails.

All other output fields are optional:

| Field | Type | Meaning |
| --- | --- | --- |
| `enabled` | boolean | Enable or disable the output. If `false`, no other settings from that output entry are applied. |
| `mode` | string | `WIDTHxHEIGHT` or `WIDTHxHEIGHT@HZ`. The mode must exist on the connected output. If refresh is included, it is rounded to millihertz and matched exactly against niri's reported mode. Do not include a trailing `Hz`. |
| `scale` | number | Positive finite scale value. |
| `transform` | string | Output transform. See valid values below. |
| `position` | table | Logical position, for example `{ x = 3440, y = 0 }`. |
| `vrr` | boolean | Enable or disable variable refresh rate. |

Valid `transform` values are:

```text
normal
90
180
270
flipped
flipped-90
flipped-180
flipped-270
```

Unmentioned connected outputs are left in their current enabled state and reported as warnings in dry-run output.

Apply action order is fixed for safety: enable outputs first, then mode, scale, transform, VRR, position, and finally disable outputs.

## Validation and Safety

The config loader rejects:

- Unsupported `version` values.
- Empty or duplicate profile ids.
- Empty monitor matchers.
- Non-positive or non-finite scale values.
- Unknown transform strings.

Apply planning rejects:

- Output matchers that match multiple connected outputs.
- Multiple output entries targeting the same connected output.
- Modes that are malformed or unavailable on the connected output.
- Profile layouts whose enabled outputs have known live rectangles that overlap with positive area. Monitors may touch edges.
- Plans that would disable all connected outputs while `prevent_disable_all = true`.

Before profile or manual-layout positions are applied, enabled outputs with live logical rectangles are shifted as a group so the enclosing layout rectangle starts at logical coordinate `0,0`. This preserves relative placement while avoiding negative final coordinates. If only some positions are configured, the planner may add position actions for other currently enabled outputs so the whole known layout stays normalized.

Use dry-run before applying a new profile:

```sh
cargo run --bin niri-monitorsd -- --dry-run --once
```

## Save Behavior

`niri-monitors-gui` saves with validation. If the target file already exists, it first copies the old file to the same directory with `.bak` appended to the filename, for example `config.toml.bak`.

When the daemon is running, the GUI asks it to reload the saved config after a successful save. `Apply Preview` is disabled while the GUI has unsaved changes because the daemon only reads the file on disk.

## Agent Reference

Source-of-truth implementation points:

- Data model and TOML field names: `src/model.rs`.
- Config path resolution, missing/empty config handling, backups, and validation: `src/config.rs`.
- Matcher semantics and profile ordering: `src/matching.rs`.
- Apply-plan validation, action ordering, warnings, and dry-run text: `src/niri/apply.rs`.
- Runtime manual/automatic selection behavior: `src/daemon.rs` and `src/control.rs`.

Tests in those files are part of the expected behavior. Update this document in the same change as any config schema, matcher, validation, or apply-plan change.
