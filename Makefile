# Standard darkmatter command surface (ADR-0002), as make targets.
# Every recipe runs inside the devshell unless already in one.
SHELL := /usr/bin/env bash
NIX := $(if $(IN_NIX_SHELL),,nix develop -c)

.PHONY: install setup run test build ci console fmt lint clean

## Check host prerequisites. Nix supplies everything else.
install:
	@command -v nix >/dev/null || { echo "nix is required: https://nixos.org/download"; exit 1; }
	@echo "nix present; toolchain comes from the devshell (make console)"

## Fetch dependencies. Re-runnable.
setup:
	$(NIX) cargo fetch

## Run the CLI: make run ARGS="--root ~/logs run"
run:
	$(NIX) cargo run --quiet -- $(ARGS)

test:
	$(NIX) cargo test

build:
	$(NIX) cargo build --release

fmt:
	$(NIX) cargo fmt --all

lint:
	$(NIX) cargo clippy --all-targets -- -D warnings

## Full pre-commit gate. Same steps CI runs.
ci:
	$(NIX) bash -euo pipefail -c '\
		cargo fmt --all -- --check && \
		cargo clippy --all-targets -- -D warnings && \
		cargo test && \
		cargo build --release'

console:
	nix develop

clean:
	rm -rf target
