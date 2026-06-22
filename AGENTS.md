# AGENTS.md

Guidance for agents working in this repository.

## Project Shape

This is a Rust crate for `niri-monitors`, centered on a daemon that manages niri monitor profiles and a native Rust GUI for editing them.

Current structure:

```text
README.md                 # User-facing project overview and quick start
src/
  lib.rs                  # Library module exports
  model.rs                # Shared config/runtime data types
  config.rs               # TOML config loading, saving, validation, paths
  matching.rs             # Monitor matcher evaluation and profile selection
  daemon.rs               # Daemon state, reconciliation, polling helpers
  control.rs              # Local JSON control socket protocol and server
  placement.rs            # Monitor rectangle derivation and snapping math
  gui/
    mod.rs
    app.rs                # Main eframe app state and update loop
    canvas.rs             # Monitor placement canvas
    daemon_client.rs      # Control socket client used by the GUI
    output_editor.rs      # Per-output settings strip/panel
    profile_editor.rs     # Condition editor and matcher diagnostics helpers
    profile_list.rs       # Profile creation, duplication, deletion defaults
  niri/
    mod.rs
    ipc.rs                # Niri IPC client trait and implementations
    output.rs             # Niri output JSON parsing and normalization
    apply.rs              # Apply plan construction, validation, execution, dry-run output
  bin/
    niri-monitorsd.rs     # Daemon CLI entrypoint
    niri-monitors-gui.rs  # Native GUI entrypoint
docs/
  config.md               # TOML schema, matching semantics, validation, apply behavior
  daemon.md               # Daemon CLI, reconcile behavior, control socket protocol, systemd
  gui.md                  # GUI usage and troubleshooting notes
contrib/systemd/
  niri-monitorsd.service  # Example systemd user service
```

If this structure changes, update this file in the same change. `AGENTS.md` should stay in sync with the project layout so future agents can orient quickly.

## Development Rules

Keep changes small and scoped. Prefer pure library code with unit tests before wiring behavior into the daemon binary.

Do not parse niri human-readable output. Use niri JSON IPC through the project-owned types in `src/niri/`.

Do not add non-Rust runtime dependencies for daemon behavior.

Do not edit generated/build output under `target/`.

The GUI currently uses the default config path only. The daemon supports `--config PATH`.

Daemon and GUI startup tolerate a missing or empty config by using an empty in-memory `version = 1` config. Do not reintroduce startup failure for a missing config unless the behavior is intentionally changed with tests and docs.

Manual layout behavior remains runtime-only in the daemon/control protocol. The Monitoradlo-style GUI canvas edits the selected saved profile layout draft when a profile exists; with no profiles configured, it edits and applies a live manual-layout draft without changing saved TOML.

Profile selection is deterministic: manual profile if enabled and matching, otherwise enabled matching profile by priority, specificity, then config order.

## Pre-Commit Gate

Before committing any tangible change, run all of:

```sh
cargo fmt
cargo test
cargo clippy
```

If any gate fails, fix the issue and rerun the full gate set before committing.

## Documentation Discipline

Documentation must stay aligned with source code. When changing CLI flags, config semantics, daemon behavior, control socket protocol, systemd integration, or project structure, update the relevant docs in the same change.

Keep these files consistent with implementation details:

```text
README.md
docs/daemon.md
docs/config.md
docs/gui.md
AGENTS.md
contrib/systemd/niri-monitorsd.service
```

When source code behavior and docs disagree, treat source code and tests as the current truth, then update docs to match or explicitly change the source behavior with tests.

## Testing Focus

Use unit tests for:

- Config parsing and validation.
- Niri output normalization.
- Profile matching and deterministic selection.
- Apply plan ordering and safety checks.
- Daemon reconciliation and no-op fingerprint behavior.
- Control request/response behavior and stable error codes.

Manual verification that requires a running niri session should be called out separately because it depends on `NIRI_SOCKET` and local monitor hardware.

## Implementation Cross-Reference

Use these files as source-of-truth anchors before changing docs or behavior:

| Concern | Source files |
| --- | --- |
| Config schema, defaults, validation, save backup | `src/model.rs`, `src/config.rs` |
| Matcher semantics and deterministic selection | `src/matching.rs` |
| Daemon reconcile loop, dry-run, no-op fingerprinting | `src/daemon.rs` |
| Control socket JSON protocol and stable error codes | `src/control.rs`, `src/gui/daemon_client.rs` |
| Niri JSON IPC and output normalization | `src/niri/ipc.rs`, `src/niri/output.rs` |
| Apply plan safety, action ordering, dry-run formatting | `src/niri/apply.rs` |
| Layout rectangles, snapping, overlap, normalization | `src/placement.rs`, `src/gui/canvas.rs` |
| GUI user-visible workflow | `src/gui/app.rs`, `src/gui/profile_list.rs`, `src/gui/output_editor.rs` |

When adding fields to control responses or config structs, update both human docs and the relevant agent-facing notes in this file.
