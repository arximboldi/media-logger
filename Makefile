# Standard darkmatter command surface (ADR-0002), as make targets.
#
# Every recipe assumes it is already running inside the devshell
# (`nix develop`, or direnv). Nothing here shells out to nix; CI wraps the
# whole invocation instead.
SHELL := /usr/bin/env bash

.PHONY: install setup test build ci console fmt lint clean

## Check host prerequisites. Nix supplies the toolchain itself.
install:
	@command -v nix >/dev/null || { echo "nix is required: https://nixos.org/download"; exit 1; }
	@command -v cargo >/dev/null || { echo "not in the devshell — run: nix develop"; exit 1; }
	@echo "toolchain present"

## Fetch dependencies. Re-runnable.
setup:
	cargo fetch

test:
	cargo test

build:
	cargo build --release

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

## Full pre-commit gate. Same steps CI runs.
ci:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings
	cargo test
	cargo build --release

## The one target that enters nix rather than assuming it: that is its job.
console:
	nix develop

clean:
	rm -rf target
