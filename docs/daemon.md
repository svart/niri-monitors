# niri-monitorsd

`niri-monitorsd` is the background daemon that reads `niri-monitors` TOML profiles, selects the best matching profile for the currently connected niri outputs, and applies output settings through niri JSON IPC.

The daemon requires `NIRI_SOCKET`. The control socket also requires `XDG_RUNTIME_DIR` unless disabled or running one-shot mode.

## Config Path

By default, the daemon reads the first path implied by the environment:

```text
$XDG_CONFIG_HOME/niri-monitors/config.toml
~/.config/niri-monitors/config.toml
```

Use `--config PATH` to point at another file.

If the config file is missing or empty, the daemon starts with an empty in-memory config. It still polls niri outputs and listens on the control socket, but it will not apply monitor changes until a saved config with matching profiles is loaded. See `docs/config.md` for the config format.

## CLI

| Option | Meaning |
| --- | --- |
| `--config PATH` | Load a specific TOML config instead of the default path. |
| `--dry-run` | Build and record apply plans without sending output actions to niri. With `--once`, the plan is printed to stdout. |
| `--once` | Reconcile once and exit. The control socket is not started in this mode. |
| `--log-level FILTER` | Set the tracing filter. Defaults to `info`; `debug` is useful for troubleshooting. |
| `--no-control-socket` | Run continuously without the local control socket. |

## Manual Run

Print the selected profile plan without applying anything:

```sh
cargo run --bin niri-monitorsd -- --dry-run --once
```

Apply once and exit:

```sh
cargo run --bin niri-monitorsd -- --once
```

Run continuously with the control socket enabled:

```sh
cargo run --bin niri-monitorsd
```

Run continuously without applying changes:

```sh
cargo run --bin niri-monitorsd -- --dry-run
```

Continuous dry-run mode suppresses niri output actions, but it currently does not print every plan. Use `--dry-run --once` when you need readable planned actions.

## Reconcile Behavior

On startup and after detected output changes, the daemon:

- Reads connected outputs from niri JSON IPC.
- Skips automatic profile selection if a manual layout is active and automatic loading is disabled.
- Selects the manual profile if one is set, enabled, and still matches connected outputs.
- Otherwise selects the enabled matching profile with the highest priority, then highest condition specificity, then config order.
- Builds an apply plan and refuses unsafe plans such as overlapping enabled outputs with known live rectangles or disabling every connected output when `prevent_disable_all = true`.
- Applies the plan unless `--dry-run`, automatic loading is disabled, or the same profile/actions/output fingerprint were already applied.

The continuous loop polls every `daemon.poll_interval_ms`. When the output fingerprint changes, it waits `daemon.debounce_ms` before reconciling so transient hotplug states are less likely to be applied.

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
| `activate_profile` | `profile_id: string` | Request a saved profile as the manual profile and reconcile once. The profile must exist and match current outputs; reconciliation only selects enabled profiles. |
| `clear_manual_profile` | none | Clear manual profile/layout state, enable automatic loading, and reconcile once. |
| `preview_profile` | `profile_id: string` | Build a plan for a saved profile and return actions, warnings, and dry-run text without applying. |
| `dry_run_profile` | `profile_id: string` | Same response and behavior as `preview_profile`. |
| `apply_manual_layout` | `placements: array` | Apply position-only changes for connected enabled outputs, normalize the layout to top-left `0,0`, pause automatic loading, and mark runtime state as manual layout. |

Example requests:

```json
{"type":"set_auto_mode","enabled":false}
```

```json
{"type":"activate_profile","profile_id":"home"}
```

```json
{"type":"apply_manual_layout","placements":[{"output":"DP-1","position":{"x":0,"y":0}}]}
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
| `auto_apply` | Current runtime automatic profile loading state. |
| `selected_profile` | Profile selected by the latest reconcile, if any. |
| `manual_profile` | Requested manual profile id, if any. |
| `manual_layout` | Whether a manual runtime layout is active. |
| `outputs` | Last daemon-observed niri outputs. |
| `last_apply` | Last apply attempt summary with `profile_id`, `success`, and `warnings`. |

Preview data contains `profile_id`, `actions`, `warnings`, and `dry_run_output`.

Error codes are serialized as `BAD_REQUEST`, `PROFILE_NOT_FOUND`, `PROFILE_NOT_MATCHING`, `CONFIG_RELOAD_FAILED`, `PLAN_FAILED`, `LAYOUT_OVERLAPS`, `APPLY_FAILED`, `NIRI_ERROR`, or `STATE_LOCK`.

### Manual And Automatic Modes

`set_auto_mode` controls runtime automatic profile loading. Enabling it clears any manual profile or manual layout override, then reconciles to the currently matching profile. Disabling it leaves the daemon polling output state but prevents condition-selected profiles from being applied automatically.

`activate_profile` is an explicit manual profile request. It pauses automatic profile loading and reconciles immediately. The selected manual profile wins only while it is enabled and matches connected outputs; otherwise normal enabled-profile selection rules decide whether another profile is selected.

`apply_manual_layout` supports ad-hoc runtime layouts through the control socket. It accepts connector/position pairs, rejects empty placement lists, duplicate outputs, disconnected outputs, and disabled outputs, then applies position actions only. Before applying, the daemon normalizes enabled output positions into one enclosing rectangle whose top-left coordinate is `0,0`. Layouts with positive-area output overlap are rejected with `LAYOUT_OVERLAPS`; edge-touching output borders are allowed.

Manual layout mode pauses automatic loading so the next daemon poll does not immediately overwrite the layout. Re-enable automatic loading with `set_auto_mode` or `clear_manual_profile`.

## Agent Reference

Implementation entrypoints:

- CLI parsing and startup: `src/bin/niri-monitorsd.rs`.
- Daemon state and reconcile loop: `src/daemon.rs`.
- Profile selection: `src/matching.rs`.
- Apply plan validation, ordering, dry-run formatting, and execution: `src/niri/apply.rs`.
- Control request/response types and handlers: `src/control.rs`.

Tests in these modules document intended behavior. If this page and tests disagree, update this page or intentionally change code with tests.

## systemd User Service

Build and install the daemon binary somewhere stable. For a cargo user install:

```sh
cargo install --path .
```

Install the example service:

```sh
mkdir -p ~/.config/systemd/user
cp contrib/systemd/niri-monitorsd.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now niri-monitorsd.service
```

Check logs:

```sh
journalctl --user -u niri-monitorsd.service -f
```

A user service must be able to see `NIRI_SOCKET`. Depending on how niri starts your user services, you may need to import the environment from inside the niri session before starting the service:

```sh
systemctl --user import-environment NIRI_SOCKET XDG_RUNTIME_DIR WAYLAND_DISPLAY DISPLAY
```

## Troubleshooting

If startup fails with `NIRI_SOCKET is not set`, start the daemon from inside the niri session or import the variable into the user service environment.

If startup fails with `XDG_RUNTIME_DIR is not set`, either run with `--no-control-socket`, run `--once`, or start it in a normal user session where `XDG_RUNTIME_DIR` exists.

If startup fails with `failed to read config`, the file exists but cannot be read. Check file permissions or pass `--config PATH`.

If no profile is selected, run `niri-monitorsd --dry-run --once` and compare connected output descriptions against profile matchers. `niri msg --json outputs` shows the raw output data.

If applying a profile would disable every connected output, the daemon refuses the plan unless `prevent_disable_all = false` is set in `[daemon]`.

If `Apply Preview` is disabled in the GUI, start the daemon if needed and save pending GUI changes first. The daemon only reads the saved config.
