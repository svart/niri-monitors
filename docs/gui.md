# niri-monitors GUI

`niri-monitors-gui` is a native eframe/egui editor for the same TOML config used by `niri-monitorsd`. The window title and main editor layout follow the Monitoradlo-style profile toolbar, monitor canvas, and selected-output control strip.

Run the native editor with:

```sh
cargo run --bin niri-monitors-gui
```

The GUI loads the native TOML config from the same default path as the daemon:

```text
$XDG_CONFIG_HOME/niri-monitors/config.toml
```

or, when `XDG_CONFIG_HOME` is not set:

```text
$HOME/.config/niri-monitors/config.toml
```

If the config file does not exist or is empty, the GUI starts with an empty draft instead of failing. You can arrange and apply the live monitor layout without creating a profile, or create a profile and save it to write the file.

The GUI currently has no CLI flag for an alternate config path.

## Typical Workflow

1. Start niri so `NIRI_SOCKET` is available.
2. Run `cargo run --bin niri-monitors-gui`.
3. With no profiles configured, drag monitor rectangles in the canvas and use `Apply Changes` to apply the live layout.
4. Use `+ New` if you want to save the current outputs as a reusable profile.
5. Select a profile from the `Profile` dropdown.
6. Drag monitor rectangles in the canvas or edit the selected output in the bottom strip.
7. Use `Rename` to edit the selected profile name.
8. Use `Save` or `Ctrl`/`Cmd`+`S` to write the TOML config.
9. Start `niri-monitorsd` if you want `Apply Preview` to ask the daemon for the saved profile's plan.

## Layout

- Top: `Profile` dropdown plus `+ New`, `Rename`, `Delete`, and `Save` actions.
- Middle: monitor arrangement canvas. With a selected profile, dragging updates that output's saved profile position draft. Without a profile, dragging updates a live manual layout draft.
- Bottom: selected-output controls for profile mode, or live-layout `Apply Changes` and `Reset Draft` controls when no profile is selected.

The interface adapts to window size: large windows use taller strips, larger text, and larger buttons; narrow windows use compact spacing and smaller controls.

## Editing

- Profile edits are made in memory first.
- `Save` or `Ctrl`/`Cmd`+`S` writes the TOML config.
- `Ctrl+Q` exits the GUI.
- Saving an existing config first writes `config.toml.bak` next to the config file.
- After a successful save, the GUI asks a running daemon to reload the saved config.
- `+ New` creates a profile from live outputs when available: all connected outputs become `all_connected` matchers, output settings copy current niri state, and priority is one higher than the current maximum.
- `Rename` replaces the profile dropdown with an inline name field. Press `Enter`, press `Esc`, or move focus away to finish renaming.
- `Delete` asks for confirmation before removing a profile from the draft.
- Dragging monitors edits the selected profile's saved output positions in the in-memory TOML draft when a profile is selected.
- With no profiles configured, dragging monitors edits a runtime-only live layout draft. `Apply Changes` applies changed positions through the daemon when it is running, otherwise directly through niri IPC.
- `Reset Draft` discards unapplied live-layout drag changes.
- Overlapping monitor rectangles are highlighted. Save is blocked until config validation passes.

## Profile Actions

`Apply Preview` requires a running daemon and no unsaved GUI changes because the daemon previews the saved config. It asks the daemon for the selected profile's planned actions and warnings without applying them.

The displayed dry-run panel shows the selected profile id, action count, warnings, and formatted dry-run text.

## Daemon Integration

When `niri-monitorsd` is running with the control socket enabled, the GUI polls:

```text
$XDG_RUNTIME_DIR/niri-monitors.sock
```

The GUI polls daemon status once per second. It can request preview plans and asks a running daemon to reload after saving.

If the daemon is unavailable, profile editing and direct live-layout apply stay available as long as the GUI can reach niri through `NIRI_SOCKET` or has existing profile data. `Apply Preview` stays disabled until the daemon is running and the current draft is saved.

## Agent Reference

Implementation entrypoints:

- GUI app state, polling, save, profile actions, and panel layout: `src/gui/app.rs`.
- Control socket client and timeouts: `src/gui/daemon_client.rs`.
- Profile canvas, dragging, snapping, overlap checks, and normalization: `src/gui/canvas.rs`.
- Profile creation and deletion defaults: `src/gui/profile_list.rs`.
- Output setting editor and per-output warnings: `src/gui/output_editor.rs`.

Keep this page aligned with user-visible labels and button behavior when editing GUI code.

## Troubleshooting

- `Config load failed`: fix the TOML syntax or validation error and restart the GUI.
- `Apply Preview` disabled: start `niri-monitorsd`, save GUI changes, then retry.
- Missing connected outputs: confirm the GUI process can reach niri through `NIRI_SOCKET`; if direct niri IPC is unavailable, start the daemon so the GUI can use daemon-reported outputs.
