# Installation

## Requirements

- Rust and Cargo. The crate uses Rust edition 2024.
- A running niri session.
- `NIRI_SOCKET` set for every process that talks to niri. Programs started from inside niri normally have it.
- `XDG_RUNTIME_DIR` set for the daemon control socket, which the GUI uses to talk to the daemon.

## Install With make

The `Makefile` installs both binaries, a desktop entry for app launchers, an icon, and the systemd user unit:

```sh
make            # cargo build --release --locked
make install    # PREFIX=~/.local, no root needed
```

`make install` does not build, so run `make` first. It also does not enable the daemon; see [Enable the Daemon](#enable-the-daemon).

| Installed file | Path (default `PREFIX` is `~/.local`) |
| --- | --- |
| `niri-monitorsd`, `niri-monitors-gui` | `$(PREFIX)/bin/` |
| Desktop entry | `$(PREFIX)/share/applications/niri-monitors.desktop` |
| Icon | `$(PREFIX)/share/icons/hicolor/scalable/apps/niri-monitors.svg` |
| systemd user unit | `$(SYSTEMDUSERUNITDIR)/niri-monitorsd.service`, default `$(PREFIX)/share/systemd/user` |

The installed desktop entry and unit use absolute binary paths. Programs that niri starts get niri's environment, and that `PATH` often does not contain `~/.cargo/bin` or `~/.local/bin`. To run the binaries from a shell, add `~/.local/bin` to your shell's `PATH`.

### System-Wide Install

Set the prefix and use sudo. Because `make install` does not build, cargo does not run as root:

```sh
make
sudo make install PREFIX=/usr/local
```

### Packaging

Use `DESTDIR` and the system unit directory:

```sh
make DESTDIR="$pkgdir" PREFIX=/usr SYSTEMDUSERUNITDIR=/usr/lib/systemd/user install
```

## Enable the Daemon

```sh
systemctl --user daemon-reload
systemctl --user enable --now niri-monitorsd.service
```

This starts the daemon now and, through `graphical-session.target`, with each later niri session. The unit restarts the daemon on failure.

Check the logs:

```sh
journalctl --user -u niri-monitorsd.service -f
```

The service must see `NIRI_SOCKET`. Depending on how niri starts your user services, you may need to import the environment from inside the niri session before you start the service:

```sh
systemctl --user import-environment NIRI_SOCKET XDG_RUNTIME_DIR WAYLAND_DISPLAY DISPLAY
```

niri creates a new socket path each time it starts, so the daemon must restart with each niri session to pick up the new `NIRI_SOCKET`.

## Install With Cargo

`cargo install --path .` installs only the two binaries into `~/.cargo/bin`. It does not install the desktop entry, icon, or unit. To run the daemon as a service, copy the example unit, whose `ExecStart` is `~/.cargo/bin/niri-monitorsd`:

```sh
cargo install --path .
mkdir -p ~/.config/systemd/user
cp contrib/systemd/niri-monitorsd.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now niri-monitorsd.service
```

A unit in `~/.config/systemd/user/` overrides an installed unit with the same name. If you change from the cargo install to `make install`, remove the copied unit.

## Try Without Installing

From the repository, inside a niri session:

```sh
cargo run --bin niri-monitorsd -- --dry-run --once   # print the plan, change nothing
cargo run --bin niri-monitors-gui                     # open the editor
```

## Uninstall

Run `make uninstall` with the same variables you used for `make install`:

```sh
systemctl --user disable --now niri-monitorsd.service
make uninstall                          # or: sudo make uninstall PREFIX=/usr/local
```

`make uninstall` does not remove your config in `~/.config/niri-monitors/` or the GUI theme setting in `~/.local/share/niri-monitors/`.

## Troubleshooting

- The launcher entry does nothing, or is missing: check that `niri-monitors.desktop` is in `$(PREFIX)/share/applications`, and that its `Exec=` line has an absolute path. If you edit the file by hand, do not use a bare `niri-monitors-gui`, because niri's `PATH` usually does not contain `~/.cargo/bin` or `~/.local/bin`. For a `/usr/local` or `~/.local` install, make sure your launcher searches `XDG_DATA_DIRS` and `XDG_DATA_HOME`. Most launchers do this by default.
- The service fails with `NIRI_SOCKET is not set`: import the environment as shown in [Enable the Daemon](#enable-the-daemon), then restart the service.
- For other daemon errors, see [Daemon troubleshooting](daemon.md#troubleshooting).
