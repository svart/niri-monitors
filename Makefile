# Build as your user, then install. The default PREFIX is ~/.local, so no root
# is needed:
#
#   make
#   make install
#
# System-wide install:
#
#   sudo make install PREFIX=/usr/local
#
# Packagers: make DESTDIR="$pkgdir" PREFIX=/usr SYSTEMDUSERUNITDIR=/usr/lib/systemd/user install
#
# `install` does not build, so `sudo make install` never runs cargo as root.

PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
SYSTEMDUSERUNITDIR ?= $(DATADIR)/systemd/user

APP_ID := niri-monitors
RELEASE := target/release

.PHONY: all build install uninstall

all: build

build:
	cargo build --release --locked

# The desktop entry and service get absolute binary paths because niri's
# systemd environment usually lacks ~/.cargo/bin and ~/.local/bin on PATH.
install:
	install -Dm755 $(RELEASE)/niri-monitorsd "$(DESTDIR)$(BINDIR)/niri-monitorsd"
	install -Dm755 $(RELEASE)/niri-monitors-gui "$(DESTDIR)$(BINDIR)/niri-monitors-gui"
	install -dm755 "$(DESTDIR)$(DATADIR)/applications" "$(DESTDIR)$(SYSTEMDUSERUNITDIR)"
	sed 's|=niri-monitors-gui$$|=$(BINDIR)/niri-monitors-gui|' contrib/desktop/$(APP_ID).desktop \
		> "$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop"
	install -Dm644 contrib/desktop/$(APP_ID).svg \
		"$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg"
	sed 's|^ExecStart=.*|ExecStart=$(BINDIR)/niri-monitorsd|' contrib/systemd/niri-monitorsd.service \
		> "$(DESTDIR)$(SYSTEMDUSERUNITDIR)/niri-monitorsd.service"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/niri-monitorsd" \
		"$(DESTDIR)$(BINDIR)/niri-monitors-gui" \
		"$(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop" \
		"$(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg" \
		"$(DESTDIR)$(SYSTEMDUSERUNITDIR)/niri-monitorsd.service"
