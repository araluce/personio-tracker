# Build and install personio-tracker.
#
# The layout follows the usual convention, so a distro package can drive it:
#
#   make install                          -> /usr/local/bin (needs sudo)
#   make install PREFIX=$HOME/.local      -> no sudo, no PATH surprises
#   make install DESTDIR=/tmp/pkg PREFIX=/usr   -> staged, for packaging
#
# `cargo install --path .` remains the other way in, and puts the same binary
# in ~/.cargo/bin instead. Pick one: two copies on a PATH is one too many.

# Must match the [[bin]] name in Cargo.toml.
BIN := personio-tracker

PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DESTDIR ?=

CARGO ?= cargo
# Locked, so a package build resolves exactly what Cargo.lock pins.
CARGO_FLAGS ?= --locked
RELEASE := target/release/$(BIN)

.PHONY: help build test lint fmt install uninstall clean

help:
	@echo "targets:"
	@echo "  build      release build"
	@echo "  test       run the test suite"
	@echo "  lint       rustfmt check and clippy, warnings denied"
	@echo "  fmt        format in place"
	@echo "  install    install $(BIN) into $(DESTDIR)$(BINDIR)"
	@echo "  uninstall  remove it again"
	@echo "  clean      cargo clean"

build:
	$(CARGO) build --release $(CARGO_FLAGS)

test:
	$(CARGO) test $(CARGO_FLAGS)

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets $(CARGO_FLAGS) -- -D warnings

fmt:
	$(CARGO) fmt

# `install -d` then `install -m` rather than `install -D`: the latter is a GNU
# extension and would fail on the BSD install that ships with macOS.
install: build
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 755 "$(RELEASE)" "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "installed $(DESTDIR)$(BINDIR)/$(BIN)"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "removed $(DESTDIR)$(BINDIR)/$(BIN)"

clean:
	$(CARGO) clean
