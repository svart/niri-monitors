# niri-monitors

Monitor profiles for the [niri](https://github.com/YaLTeR/niri) Wayland compositor.

`niri-monitors` remembers how you arrange your monitors and applies the right arrangement when monitors are connected or disconnected. It has two programs:

- `niri-monitorsd`: a daemon that watches niri outputs, selects the matching profile, and applies it.
- `niri-monitors-gui`: a native editor to arrange monitors on a canvas, save profiles, and preview what the daemon will do.

The project is inspired by [kanshi](https://sr.ht/~emersion/kanshi/) and [monitoradlo](https://github.com/kumekay/monitoradlo).

## Install

You need a Rust toolchain and a running niri session.

```sh
git clone https://github.com/svart/niri-monitors
cd niri-monitors
make            # cargo build --release --locked
make install    # installs into ~/.local, no root needed
systemctl --user daemon-reload
systemctl --user enable --now niri-monitorsd.service
```

This installs both binaries, a desktop entry, an icon, and a systemd user unit, then starts the daemon with each niri session. See [docs/installation.md](docs/installation.md) for system-wide installs, packaging, `cargo install`, and environment problems.

## Usage

1. Start **Niri Monitors** from your app launcher, or run `niri-monitors-gui`.
2. Drag monitors on the canvas or edit them in the inspector, then click `Apply`.
3. Click `+ Save as profile`, give the profile a name, and click `Save`.
4. For each other monitor setup (docked, projector, laptop only), connect it, click `+ New profile`, arrange the monitors, and click `Save`.

After each save, the daemon reloads the config and applies the matching profile whenever monitors change. To see what the daemon would do, without changing anything:

```sh
niri-monitorsd --dry-run --once
```

## How It Works

```text
         JSON IPC                           control socket (JSON)
niri  <----------->  niri-monitorsd  <------------------------->  niri-monitors-gui
  ^                        |                                            |      |
  |                        +<--- reads --- config.toml <--- writes -----+      |
  +------------------------------- JSON IPC (live outputs) --------------------+
```

- The daemon polls niri outputs (every 1.5 s by default). When they change, it waits for hotplug to settle, then selects a profile.
- A profile matches when every monitor it lists is connected. The highest priority wins; ties go to the more specific profile, then to config order.
- The daemon plans only the settings that differ from the current state. It refuses unsafe plans, such as overlapping monitors or all monitors off, and applies the rest through niri JSON IPC. It never parses human-readable `niri msg` output.
- The GUI edits `~/.config/niri-monitors/config.toml` and asks the daemon to reload after each save. Without the daemon, the GUI still edits profiles and applies live layouts through niri directly.

## Documentation

| Document | Contents |
| --- | --- |
| [Installation](docs/installation.md) | Requirements, install options, systemd service, uninstall, packaging. |
| [Configuration](docs/config.md) | TOML schema, profile matching, apply order, validation and safety checks. |
| [Daemon](docs/daemon.md) | CLI options, modes, reconcile behavior, control socket protocol, troubleshooting. |
| [GUI](docs/gui.md) | Editor workflow, canvas, inspector, keyboard shortcuts, troubleshooting. |
| [Architecture](docs/architecture.md) | Components, data flow, source layout, design decisions, development. |

Contributors and coding agents also read [AGENTS.md](AGENTS.md).

## License

MIT. See [LICENSE](LICENSE).
