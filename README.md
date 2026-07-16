# niri-monitors

`niri-monitors` manages monitor profiles for the niri compositor. It provides:

- `niri-monitorsd`: a daemon that polls niri outputs, selects a matching profile, and applies monitor settings through niri JSON IPC.
- `niri-monitors-gui`: a native Rust GUI for creating profiles, arranging profile or live monitor layouts, saving TOML config, and previewing daemon plans.

The project deliberately uses niri JSON IPC. It does not parse human-readable `niri msg` output.

This project is inspired by [monitoradlo](https://github.com/kumekay/monitoradlo) and [kanshi](https://sr.ht/~emersion/kanshi/).

## Requirements

- Rust and Cargo.
- A running niri session.
- `NIRI_SOCKET` set for any process that talks directly to niri. This is normally available to programs launched from inside niri.
- `XDG_RUNTIME_DIR` set when using the daemon control socket or GUI daemon integration.

## Quick Start

The daemon and GUI use one config file:

```text
$XDG_CONFIG_HOME/niri-monitors/config.toml
```

or, if `XDG_CONFIG_HOME` is not set:

```text
$HOME/.config/niri-monitors/config.toml
```

The config file may be missing or empty. In that case, the daemon and GUI start with an empty in-memory config. With no profiles, the daemon observes outputs but does not apply profile-driven monitor changes.

Run a safe one-shot plan without applying changes:

```sh
cargo run --bin niri-monitorsd -- --dry-run --once
```

Run the daemon continuously:

```sh
cargo run --bin niri-monitorsd
```

Open the GUI editor:

```sh
cargo run --bin niri-monitors-gui
```

Arrange outputs directly in the GUI without a profile, or create a profile, arrange outputs on the canvas, save it, then use `Apply Preview` while the daemon is running. The GUI writes `config.toml.bak` before replacing an existing config.

## Commands

| Command | Description |
| --- | --- |
| `cargo run --bin niri-monitorsd -- --dry-run --once` | Select the current profile and print the apply plan without changing monitors. |
| `cargo run --bin niri-monitorsd -- --once` | Reconcile once, apply if needed, then exit. |
| `cargo run --bin niri-monitorsd` | Run continuously with the control socket enabled. |
| `cargo run --bin niri-monitorsd -- --log-level debug` | Run with more verbose tracing output. |
| `cargo run --bin niri-monitors-gui` | Open the native GUI. |
| `cargo test` | Run the Rust test suite. |
| `cargo fmt` | Format the crate. |
| `cargo clippy` | Run lints. |

Install the binaries locally:

```sh
cargo install --path .
```

## Configuration

Profiles are TOML. The smallest valid on-disk config is:

```toml
version = 1
```

Profiles match connected outputs by exact connector, description, make, model, and/or serial fields. Prefer the GUI for first-time setup because it can populate matchers and output settings from live niri data.

See `docs/config.md` for the full schema, defaults, profile matching rules, and safety checks.

## Daemon And GUI

`niri-monitorsd` reads the config, talks to niri through `NIRI_SOCKET`, and exposes a local JSON control socket at:

```text
$XDG_RUNTIME_DIR/niri-monitors.sock
```

The GUI can run without the daemon when it can reach niri directly. If the daemon is running, the GUI uses the control socket for status, preview plans, live manual layout application, and config reload after saving. Manual profile operations remain available through the control socket protocol.

Useful docs:

- `docs/daemon.md`: daemon CLI, control protocol, systemd setup, troubleshooting.
- `docs/gui.md`: GUI workflow, buttons, daemon integration, troubleshooting.
- `docs/config.md`: TOML format and matching/apply semantics.

## Architecture

Core modules live under `src/`:

- `model.rs`: shared config/runtime data types.
- `config.rs`: config paths, load/save, validation, backups.
- `matching.rs`: matcher evaluation and profile selection.
- `daemon.rs`: reconcile loop, daemon state, no-op fingerprinting.
- `control.rs`: local JSON control socket protocol.
- `placement.rs`: monitor rectangle, overlap, normalization, and snapping math.
- `niri/`: niri IPC, output normalization, apply planning and execution.
- `gui/`: eframe/egui application and editor panels.

Agent-facing repository guidance is in `AGENTS.md`. When behavior and docs disagree, treat source and tests as the current truth, then update docs in the same change.

## Development

The `gui` feature is enabled by default, so existing build, run, test, and install commands continue to include both the daemon and GUI. When working only on daemon and core code, skip the GUI dependency graph with:

```sh
cargo build --no-default-features --bin niri-monitorsd
cargo test --no-default-features --lib
```

The native GUI intentionally enables eframe's Wayland, OpenGL, default-font, and accessibility support. X11 and web-only support are excluded because `niri-monitors-gui` targets niri's native Wayland session.

Before committing code or behavior changes, run:

```sh
cargo fmt
cargo test
cargo clippy
```
