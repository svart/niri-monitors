# Architecture

`niri-monitors` is one Rust crate. A library (`src/lib.rs`) holds the config model, matching, planning, daemon, and control protocol. Two thin binaries use it:

- `niri-monitorsd` (`src/bin/niri-monitorsd.rs`): parses the CLI, loads the config, and runs the daemon.
- `niri-monitors-gui` (`src/bin/niri-monitors-gui.rs`): starts the eframe window with the `niri-monitors` Wayland app ID.

## Components

```text
                    niri compositor
                    (NIRI_SOCKET, JSON IPC)
                     ^               ^
     outputs, output |               | live outputs,
     actions         |               | direct live-layout apply
                     |               |
        +------------+-----+     +---+---------------+
        |  niri-monitorsd  |<--->| niri-monitors-gui |
        +------------------+     +-------------------+
          ^     control socket       |
          |     $XDG_RUNTIME_DIR/    |
          |     niri-monitors.sock   |
          |                          |
          +----- config.toml <-------+
          reads at start              saves, then sends
          and on reload_config        reload_config
```

- **niri JSON IPC** is the only interface to niri. `src/niri/ipc.rs` sends one JSON request per connection to `NIRI_SOCKET`; `src/niri/output.rs` turns niri's output JSON into `NiriOutput` values sorted by connector.
- **The config file** is the only persistent state. The daemon reads it; only the GUI writes it.
- **The control socket** is a Unix socket with one line of JSON request and one line of JSON response per connection. See [daemon.md](daemon.md#control-socket) for the protocol.

## Daemon

```text
poll niri outputs ──> changed? ──> wait debounce ──> select profile ──> build plan ──> check safety ──> apply ──> re-read outputs
   (poll_interval_ms)                                 (matching.rs)      (niri/apply.rs)  (placement.rs)
```

1. **Poll.** `run_loop` reconciles once at startup, then calls `poll_once` forever. `poll_once` sleeps `daemon.poll_interval_ms`, reads outputs, and compares them with the outputs from the last reconcile. If they differ, it sleeps `daemon.debounce_ms` and reconciles.
2. **Select.** `select_profile` picks the manual profile if it is enabled and matches, otherwise the enabled matching profile with the highest priority, then specificity, then config order.
3. **Plan.** `build_apply_plan` resolves each output entry to exactly one connected output and creates actions only for settings that differ from the current state. Actions are ordered: enable, mode, scale, transform, VRR, position, disable.
4. **Check.** The planner computes the layout after the plan, rejects positive-area overlaps and plans that disable every output, and moves the layout so its top-left corner is `0,0`.
5. **Apply.** Unless the mode is paused or `--dry-run` is set, the daemon sends the actions to niri, records the result in `last_apply`, and reads the outputs again so its own change does not trigger another reconcile.

Plan and apply errors are logged and recorded; the loop keeps running. An error reading outputs from niri ends the process, so systemd can restart it with the next niri session's socket.

### Modes

`DaemonState` holds the config, a `Mode`, the last outputs, the selected profile, and `last_apply`. The mode decides what a reconcile does:

| Mode | Selects a profile | Applies it |
| --- | --- | --- |
| `Auto` | yes | yes |
| `Paused` | yes | no |
| `ManualProfile(name)` | yes, prefers `name` | once, on activation |
| `ManualLayout` | no | no |

Manual profiles and manual layouts exist only in memory. A config reload returns to `Auto` or `Paused` from `daemon.auto_apply`. See [daemon.md](daemon.md#manual-and-automatic-modes) for the transitions.

### Threads

The daemon has two threads that share one `Arc<Mutex<DaemonState>>`:

- The main thread runs the poll loop. It holds the lock only while it reconciles, not while it sleeps.
- The control thread accepts connections one at a time. For each request it locks the state, opens its own niri client, handles the request, and writes the response.

`--once` runs one reconcile on the main thread and starts no control thread.

## GUI

The GUI is an eframe/egui app (`src/gui/app.rs`). The UI thread never blocks on a socket:

- `Poller` (`src/gui/poller.rs`) runs each fetch on a background thread. The GUI polls daemon status and live niri outputs once per second each.
- `DaemonClient` (`src/gui/daemon_client.rs`) sends control requests with a 750 ms timeout.

The GUI edits a draft copy of the config. With profiles, the canvas and inspector edit the selected profile's outputs. `Apply` sends the selected profile draft through the daemon (`apply_profile_draft`) or, when the daemon is offline, directly to niri. `Save` validates the draft, writes `config.toml.bak`, rewrites `config.toml`, and asks the daemon to reload. With no profiles, the canvas edits a runtime-only live layout, which `Apply` sends through the daemon (`apply_manual_layout`) or, when the daemon is offline, directly to niri.

## Source Layout

| Path | Responsibility |
| --- | --- |
| `src/model.rs` | Config and runtime data types, TOML field names. |
| `src/config.rs` | Config path, load, missing or empty config, validation, saved TOML layout, backup. |
| `src/matching.rs` | Matcher evaluation, specificity, profile selection. |
| `src/placement.rs` | Output rectangles, logical size, overlap, normalization to `0,0`, snapping. |
| `src/daemon.rs` | `DaemonState`, modes, reconcile, poll loop, manual layout apply. |
| `src/control.rs` | Control request and response types, stable error codes, socket server. |
| `src/niri/ipc.rs` | `NiriClient` trait and the socket client. |
| `src/niri/output.rs` | niri output JSON parsing and normalization. |
| `src/niri/apply.rs` | Apply plans for profiles and manual layouts, safety checks, action order, execution, dry-run text. |
| `src/gui/app.rs` | GUI state, polling, save, actions, shortcuts, panel layout. |
| `src/gui/header.rs` | Header bar and daemon status pill text. |
| `src/gui/sidebar.rs` | Profile and output lists. |
| `src/gui/canvas.rs` | Monitor tiles, dragging, snapping, overlap highlighting, nudging. |
| `src/gui/inspector.rs` | Output settings and live-layout controls. |
| `src/gui/profile_list.rs` | Defaults for new profiles, profile deletion. |
| `src/gui/daemon_client.rs` | Control socket client and timeouts. |
| `src/gui/poller.rs` | Background fetch threads. |
| `src/gui/theme.rs` | Light and dark palettes, widget styling, shared widgets, stored theme choice. |
| `src/gui/toast.rs` | Transient result messages. |
| `contrib/` | Desktop entry, icon, and example systemd user unit. |

Unit tests live next to the code in each module. They are part of the specified behavior.

## Design Decisions

- **JSON IPC only.** The project never parses human-readable `niri msg` output, so niri's text format can change without breaking it.
- **No non-Rust runtime dependencies** for daemon behavior.
- **Diff-based plans.** The daemon sends only settings that differ from the current state, so an already applied profile produces an empty plan and no niri requests.
- **Safety before apply.** Overlaps, ambiguous matchers, unknown modes, and disabling every output are rejected while planning, before any action is sent.
- **Profiles are identified by name.** There is no separate ID. Control requests and status fields carry names.
- **Strict config.** Config structs use `#[serde(deny_unknown_fields)]`, so a misspelled key is a load error. The GUI writes a fixed TOML layout and does not keep comments; the previous file stays in `config.toml.bak`.
- **Missing config is not an error.** The daemon and GUI start with an empty `version = 1` config.
- **Manual layouts are runtime-only.** They never change the saved TOML.
- **Optional GUI.** The default `gui` feature adds eframe with only Wayland, OpenGL (glow), default fonts, accessibility, and persistence. X11 and web support are left out because the GUI targets niri's Wayland session.

## Development

Run from the repository inside a niri session:

```sh
cargo run --bin niri-monitorsd -- --dry-run --once
cargo run --bin niri-monitorsd -- --log-level debug
cargo run --bin niri-monitors-gui
```

To work on daemon and core code without building the GUI dependencies:

```sh
cargo build --no-default-features --bin niri-monitorsd
cargo test --no-default-features --lib
```

Before every commit, run:

```sh
cargo fmt
cargo test
cargo clippy
```

Change the docs in the same commit as the behavior they describe. [AGENTS.md](../AGENTS.md) lists the rules for contributors and coding agents.
