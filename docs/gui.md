# niri-monitors GUI

`niri-monitors-gui` is a native eframe/egui editor for the same TOML config used by `niri-monitorsd`. It has a header with the daemon status, a profile sidebar, a monitor canvas, and an inspector for the selected output.

Run the native editor with:

```sh
cargo run --bin niri-monitors-gui
```

After `make install` (see the README), you can also start `Niri Monitors` from an app launcher. The installed desktop entry is `niri-monitors.desktop`. The window's Wayland app ID is `niri-monitors`. This value is the same as the desktop entry name and the icon name, so launchers, docks, and niri can link the window to them. To match the window in a niri window rule, use `match app-id="^niri-monitors$"`.

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
2. Start `Niri Monitors` from your app launcher, or run `cargo run --bin niri-monitors-gui`.
3. With no profiles configured, drag monitors on the canvas or change a monitor's parameters in the inspector, then use `Apply` to apply the live layout.
4. Use `+ Save as profile` (or `+ New profile` once profiles exist) to save the current outputs as a reusable profile. The new profile starts in rename mode.
5. Select a profile in the sidebar.
6. Drag monitors on the canvas, or edit the selected output in the inspector.
7. Use `Save` in the header or `Ctrl`/`Cmd`+`S` to write the TOML config.
8. Start `niri-monitorsd` if you want `Preview plan` to ask the daemon for the saved profile's plan.

## Layout

- Header: the app name, a daemon status pill, a `niri unreachable` pill when niri cannot be reached, `Save`, and the theme button. Hover a pill for details.
- Sidebar (left): the profile list, or the connected outputs (including outputs that are off) when no profiles exist.
- Canvas (center): the monitor arrangement. A dot grid fills the background and a small crosshair marks logical `0, 0`.
- Inspector (right): settings for the selected output, the layout check, and the plan preview. Below 1000 px of window width, the inspector moves under the canvas.

The GUI starts in the light theme. The sun or moon button at the right end of the header switches between the light and dark themes. The GUI keeps your choice and uses it again at the next start. It stores the choice in `$XDG_DATA_HOME/niri-monitors/app.ron`, or in `$HOME/.local/share/niri-monitors/app.ron` when `XDG_DATA_HOME` is not set. eframe gets this directory name from the window's app ID. To go back to the light default, delete that file. The GUI does not keep the window size or position.

### Daemon status pill

| Pill | Meaning |
| --- | --- |
| `Automatic · <profile>` | The daemon applies the best matching profile when monitors change. |
| `Paused` | The daemon selects profiles but does not apply them. |
| `Manual profile · <profile>` | A profile was activated through the control socket and is kept. |
| `Manual layout` | A layout applied from this window is active; profiles are not applied. |
| `Daemon offline` | The control socket cannot be reached. Hover for the reason. |

The pill turns red when the daemon's last apply failed; hover it for the error.

### Sidebar

Each profile row shows a dot (green when the profile matches the connected monitors), its priority, whether it matches, and an `Active` badge on the profile the daemon has selected. Double-click a row to rename it; right-click for `Rename` and `Delete…`.

### Canvas

Each monitor tile shows the connector, the make and model, and its logical size, scale, and refresh rate after the profile is applied. Tiles use the profile's mode, scale, and rotation, so they show the layout the profile produces, not the current one.

- The selected tile has an accent outline and a shadow.
- Overlapping tiles have a red fill. The inspector names the overlapping outputs.
- Outputs that the profile turns off, and profile outputs that are not connected, have a dashed outline.
- Dragging snaps edges and centers to other monitors; dashed accent lines show the snap.

## Editing

- Profile edits are made in memory first. `Unsaved changes` appears next to `Save` until you save.
- Saving an existing config first writes `config.toml.bak` next to the config file.
- After a successful save, the GUI asks a running daemon to reload the saved config.
- `+ New profile` creates a profile from live outputs when available: all connected outputs become `all_connected` matchers, output settings copy current niri state, and priority is one higher than the current maximum.
- Deleting a profile asks for confirmation in a dialog.
- Results such as `Saved` or `Applying failed` appear as toasts in the bottom-right corner. Click a toast to dismiss it.
- Config load and validation errors appear in a red banner under the header. Save stays disabled until the config is valid.
- The GUI does not edit profile conditions, priority, or the profile `enabled` flag. Edit those in the TOML file; see `docs/config.md`.

### Inspector

The inspector edits the selected output of the selected profile:

- `Turn on`, `Mode`, and `Variable refresh rate` in the `Display` card. A mode the connected output does not offer shows a warning. For an output that is not connected, the mode is a text field (`WIDTHxHEIGHT@HZ`).
- `Scale` with presets (1×, 1.25×, 1.5×, 1.75×, 2×) and a custom value.
- `Rotation` (0°, 90°, 180°, 270°) and `Mirrored`.
- `Position` of the top-left corner.

A setting that the profile leaves unset keeps the output's current value when the profile is applied, and the inspector shows that value as `keeps …`. `Reset` unsets a setting.

### Keyboard

| Keys | Action |
| --- | --- |
| `Ctrl`/`Cmd`+`S` | Save. |
| `Ctrl`+`Q` | Quit. |
| Arrow keys | Move the selected monitor by 1 px. |
| `Shift`+arrow keys | Move the selected monitor by 10 px. |
| `Delete` | Ask to delete the selected profile. |

Keys other than `Ctrl` shortcuts are ignored while a text field has focus.

## Profile Actions

`Preview plan` requires a running daemon and no unsaved GUI changes because the daemon previews the saved config. It shows the actions the daemon would send and any warnings, without applying them.

## Live Layout

With no profiles configured, the canvas edits a runtime-only draft of the live layout. Drag monitors to move them, or select a monitor and edit it in the `Parameters` card of the inspector:

- `Mode`: a list of the modes the monitor offers, the largest resolution first, then the fastest refresh rate.
- `Scale`: `−` and `+` go to the previous or next multiple of 0.25. You can also drag the value, or click it and type a scale from 0.25 to 4.
- `Rotation`: `↱` on the left turns the output 90° clockwise. `↰` on the right turns it 90° counterclockwise. niri counts angles counterclockwise, so `↰` adds 90° to the angle. A mirrored output stays mirrored.
- `Position`: `x` and `y` of the top-left corner.

The canvas shows the changes at once, so you can see and fix overlaps before you apply. `Apply` becomes available when a parameter is different from the value that niri reports. Positions are compared after the layout is moved to start at `0, 0`, so if you move all monitors by the same distance, there is no change to apply.

`Apply` sends the changed parameters through the daemon when it is running, which pauses automatic profile loading, or directly through niri IPC otherwise. After the apply, the canvas shows the layout that niri reports. If you change the mode but not the scale, niri can pick a new automatic scale, so the result can differ from the draft. `Reset` discards all changes. Outputs that are off appear in the sidebar but cannot be placed. Nothing is written to the config file.

## Daemon Integration

When `niri-monitorsd` is running with the control socket enabled, the GUI polls:

```text
$XDG_RUNTIME_DIR/niri-monitors.sock
```

The GUI polls daemon status once per second. It can request preview plans and asks a running daemon to reload after saving.

If the daemon is unavailable, profile editing and direct live-layout apply stay available as long as the GUI can reach niri through `NIRI_SOCKET` or has existing profile data. `Preview plan` stays disabled until the daemon is running and the current draft is saved.

## Agent Reference

Implementation entrypoints:

- GUI app state, polling, save, actions, shortcuts, and panel layout: `src/gui/app.rs`.
- Palettes, widget styling, shared widgets (cards, pills, buttons), and the stored theme choice: `src/gui/theme.rs`.
- Header and daemon status pill text: `src/gui/header.rs`.
- Profile and output lists: `src/gui/sidebar.rs`.
- Output settings and live-layout controls: `src/gui/inspector.rs`.
- Canvas tiles, dragging, snapping, overlap highlighting, and nudging: `src/gui/canvas.rs`.
- Toasts: `src/gui/toast.rs`.
- Control socket client and timeouts: `src/gui/daemon_client.rs`.
- Profile creation and deletion defaults: `src/gui/profile_list.rs`.

Keep this page aligned with user-visible labels and button behavior when editing GUI code.

## Troubleshooting

- `The config could not be loaded`: fix the TOML syntax or validation error and restart the GUI.
- `Preview plan` disabled: hover it for the reason. Start `niri-monitorsd`, save GUI changes, then retry.
- `niri unreachable` or `Cannot reach niri` on the canvas: the GUI cannot reach niri through `NIRI_SOCKET`. Start it from inside the niri session, or start the daemon so the GUI can use daemon-reported outputs.
- The launcher entry does nothing, or is missing: check that `niri-monitors.desktop` is in `$(PREFIX)/share/applications`, and that its `Exec=` line has an absolute path. If you edit the file by hand, do not use a bare `niri-monitors-gui`, because niri's `PATH` usually does not contain `~/.cargo/bin` or `~/.local/bin`. For a `/usr/local` or `~/.local` install, make sure your launcher searches `XDG_DATA_DIRS` and `XDG_DATA_HOME`. Most launchers do this by default.
