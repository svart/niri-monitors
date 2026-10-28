# niri-monitorsd

`niri-monitorsd` is the background daemon that reads `niri-monitors` TOML profiles, selects the best matching profile for the currently connected niri outputs, and applies output settings through niri JSON IPC.

The daemon requires `NIRI_SOCKET`. The control socket also requires `XDG_RUNTIME_DIR` unless disabled or running one-shot mode.

## Config Path

By default, the daemon reads `$XDG_CONFIG_HOME/niri-monitors/config.toml`, or `~/.config/niri-monitors/config.toml` when `XDG_CONFIG_HOME` is not set. Use `--config PATH` to point at another file. See [config.md](config.md) for the config format.

If the config file is missing or empty, the daemon starts with an empty in-memory config. It still polls niri outputs and listens on the control socket, but it will not apply monitor changes until a saved config with matching profiles is loaded.

## CLI

| Option | Meaning |
| --- | --- |
| `--config PATH` | Load a specific TOML config instead of the default path. |
| `--dry-run` | Build and record apply plans without sending output actions to niri. With `--once`, the plan is printed to stdout. The plan lists only the changes needed to reach the profile. |
| `--once` | Reconcile once and exit. The control socket is not started in this mode. Exits with an error if the profile cannot be planned or applied. |
| `--log-level LEVEL` | Set the most detailed log level: `error`, `warn`, `info`, `debug`, or `trace`. Defaults to `info`; `debug` is useful for troubleshooting. Per-module filters are not supported. |
| `--no-control-socket` | Run continuously without the local control socket. |

## Manual Run

Normally the systemd user service runs the daemon; see [installation.md](installation.md#enable-the-daemon). `--dry-run --once` is safe to run at any time. Stop the service before you run a second continuous daemon, because a new daemon replaces the control socket of the running one.

```sh
niri-monitorsd --dry-run --once   # print the selected profile's plan, apply nothing
niri-monitorsd --once             # apply once and exit
niri-monitorsd                    # run continuously with the control socket
niri-monitorsd --dry-run          # run continuously without applying changes
```

From a source checkout, replace `niri-monitorsd` with `cargo run --bin niri-monitorsd --`.

Continuous dry-run mode suppresses niri output actions, but it currently does not print every plan. Use `--dry-run --once` when you need readable planned actions.

## Reconcile Behavior

On startup and after detected output changes, the daemon:

- Reads connected outputs from niri JSON IPC.
- Skips automatic profile selection if a manual layout is active and automatic loading is disabled.
- Selects the manual profile if one is set, enabled, and still matches connected outputs.
- Otherwise selects the enabled matching profile with the highest priority, then highest specificity (see [config.md](config.md#profiles)), then config order.
- Builds an apply plan and refuses unsafe plans such as enabled outputs that overlap after the plan or disabling every connected output when `prevent_disable_all = true`. See [config.md](config.md#validation-and-safety) for how the resulting layout is checked.
- Plans only the settings that differ from the current output state. If the outputs already match the profile, the plan is empty and nothing is sent to niri.
- Applies the plan unless `--dry-run` or automatic loading is disabled, then reads the outputs from niri again.

The continuous loop polls every `daemon.poll_interval_ms`. When niri reports different outputs than the last reconcile saw, it waits `daemon.debounce_ms` before reconciling so transient hotplug states are less likely to be applied.

### Errors

If the selected profile cannot be planned (for example, its mode is not available, a matcher is ambiguous, or the layout overlaps) or niri rejects an action, the continuous daemon logs the error, records it in `last_apply`, and keeps running. It tries again on the next output change, config reload, or control request that reconciles (`set_auto_mode`, `activate_profile`, `clear_manual_profile`).

If the daemon cannot read outputs from niri, it exits with an error. niri creates a new socket path each time it starts, so a restarted daemon is needed to pick up the new `NIRI_SOCKET`. The example systemd service restarts the daemon on failure.

## Control Socket

When enabled, the daemon listens on:

```text
$XDG_RUNTIME_DIR/niri-monitors.sock
```

The protocol is one line-delimited JSON request per connection and one line-delimited JSON response. Example status request:

```sh
printf '%s\n' '{"type":"status"}' | socat - "$XDG_RUNTIME_DIR/niri-monitors.sock"
```

Requests use a `type` field with snake_case names:

| Request | Required fields | Effect |
| --- | --- | --- |
| `status` | none | Return daemon status from memory. The request handler still requires `NIRI_SOCKET` to be set because it constructs a niri client before dispatching requests. |
| `reload_config` | none | Load the saved config path, update runtime auto mode from `[daemon]`, then reconcile once. Missing or invalid saved config returns `CONFIG_RELOAD_FAILED`. |
| `set_auto_mode` | `enabled: bool` | Enable or pause automatic profile loading. Enabling clears manual overrides and reconciles once. |
| `activate_profile` | `profile: string` (profile name) | Request a saved profile as the manual profile and reconcile once. The profile must exist and match current outputs; reconciliation only selects enabled profiles. |
| `clear_manual_profile` | none | Clear manual profile/layout state, enable automatic loading, and reconcile once. |
| `preview_profile` | `profile: string` (profile name) | Build a plan for a saved profile and return actions, warnings, and dry-run text without applying. |
| `dry_run_profile` | `profile: string` (profile name) | Same response and behavior as `preview_profile`. |
| `apply_manual_layout` | `placements: array` | Move connected enabled outputs and optionally change their mode, scale, and rotation, normalize the layout to top-left `0,0`, and switch to manual layout mode. |
| `apply_profile_draft` | `profile: object` (a profile, as in the config file) | Apply a profile that does not have to be saved, such as the GUI's unsaved draft, and switch to manual layout mode. The saved config does not change. |

Example requests:

```json
{"type":"set_auto_mode","enabled":false}
```

```json
{"type":"activate_profile","profile":"Home"}
```

```json
{"type":"apply_profile_draft","profile":{"name":"Home","outputs":[{"match":{"connector":"DP-1"},"scale":1.25}]}}
```

```json
{"type":"apply_manual_layout","placements":[{"output":"DP-1","position":{"x":0,"y":0},"mode":"2560x1440@59.951","scale":1.25,"transform":"90"}]}
```

Successful responses have this shape:

```json
{"ok":true,"data":{}}
```

Error responses have this shape:

```json
{"ok":false,"error":{"code":"PLAN_FAILED","message":"..."}}
```

Status data contains:

| Field | Meaning |
| --- | --- |
| `auto_apply` | `true` in automatic mode. See Manual And Automatic Modes. |
| `selected_profile` | Name of the profile selected by the latest reconcile, if any. |
| `manual_profile` | Profile name in manual profile mode, otherwise `null`. |
| `manual_layout` | `true` in manual layout mode. |
| `outputs` | Last daemon-observed niri outputs. |
| `last_apply` | Result of the latest reconcile that selected a profile, with `profile` (the profile name), `success`, `warnings`, and `error` when `success` is `false`. A reconcile with nothing to change counts as a success. |

Preview data contains `profile` (the profile name), `actions`, `warnings`, and `dry_run_output`.

Error codes are serialized as `BAD_REQUEST`, `PROFILE_NOT_FOUND`, `PROFILE_NOT_MATCHING`, `CONFIG_RELOAD_FAILED`, `PLAN_FAILED`, `LAYOUT_OVERLAPS`, `APPLY_FAILED`, `NIRI_ERROR`, or `STATE_LOCK`.

### Manual And Automatic Modes

The daemon is always in one of four modes:

| Mode | Status fields | On output changes |
| --- | --- | --- |
| Automatic | `auto_apply: true` | Selects the best matching profile and applies it. |
| Paused | `auto_apply: false`, `manual_profile: null`, `manual_layout: false` | Selects the best matching profile but does not apply it. |
| Manual profile | `manual_profile: "<name>"` | Like paused, but the manual profile is selected while it is enabled and matches connected outputs. Otherwise normal selection rules apply. The manual profile is applied once, when it is activated. |
| Manual layout | `manual_layout: true` | Does not select or apply profiles, so the next poll does not overwrite the layout. |

Mode changes:

- At startup and on `reload_config`, the mode is automatic if `daemon.auto_apply = true` and paused otherwise. A reload ends a manual profile or manual layout. The GUI relies on this: after it saves a config, its reload request lets the daemon apply the saved profiles.
- `set_auto_mode` with `enabled: true` and `clear_manual_profile` both switch to automatic, then reconcile.
- `set_auto_mode` with `enabled: false` switches automatic to paused. A manual profile or manual layout stays active.
- `activate_profile` switches to manual profile, then reconciles and applies the profile even though automatic loading is paused.
- `apply_manual_layout` and `apply_profile_draft` switch to manual layout.

`apply_manual_layout` supports ad-hoc runtime layouts through the control socket. Each placement has an `output` connector and a `position`. It can also have a `mode`, `scale`, and `transform`, written as in the config file (see [config.md](config.md#output-settings)). A setting that is left out keeps its current value. The daemon rejects these with `BAD_REQUEST`: empty placement lists, duplicate outputs, disconnected outputs, disabled outputs, modes the output does not offer, and scales that are not finite and greater than zero. It then applies mode, scale, rotation, and position actions, in that order. It does not change VRR. A setting that already matches gets no action; a scale within niri's rounding counts as a match. The overlap check uses the size an output has after its new mode, scale, and rotation. If a placement changes the mode but not the scale, niri can pick a new automatic scale, so the size after the apply can differ. Before applying, the daemon normalizes enabled output positions into one enclosing rectangle whose top-left coordinate is `0,0`, so outputs without a placement can move too. Outputs already at their final position get no action. Layouts with positive-area output overlap are rejected with `LAYOUT_OVERLAPS`; edge-touching output borders are allowed. A rejected layout does not change the daemon mode.

`apply_profile_draft` builds the same plan as for a saved profile, with the same safety checks against the saved `[daemon]` settings, and does not check the profile's `enabled` flag, priority, or condition. Plan errors return `PLAN_FAILED` or `LAYOUT_OVERLAPS` and do not change the daemon mode. The result is recorded in `last_apply` under the draft's name.

## Troubleshooting

Service logs: `journalctl --user -u niri-monitorsd.service -f`. For installing and enabling the service, see [installation.md](installation.md#enable-the-daemon).

If startup fails with `NIRI_SOCKET is not set`, start the daemon from inside the niri session or [import the variable](installation.md#enable-the-daemon) into the user service environment.

If startup fails with `XDG_RUNTIME_DIR is not set`, either run with `--no-control-socket`, run `--once`, or start it in a normal user session where `XDG_RUNTIME_DIR` exists.

If startup fails with `failed to read config`, the file exists but cannot be read. Check file permissions or pass `--config PATH`.

If no profile is selected, run `niri-monitorsd --dry-run --once` and compare connected output descriptions against profile matchers. `niri msg --json outputs` shows the raw output data.

If applying a profile would disable every connected output, the daemon refuses the plan unless `prevent_disable_all = false` is set in `[daemon]`.

If a profile is selected but monitors do not change, check the daemon log or the `last_apply.error` field of a `status` request for the plan or apply error.

If `Preview plan` is disabled in the GUI, start the daemon if needed and save pending GUI changes first. The daemon only reads the saved config.
