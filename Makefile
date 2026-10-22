# Standard darkmatter command surface (ADR-0002), as make targets.
#
# Every recipe assumes it is already running inside the devshell
# (`nix develop`, or direnv). Nothing here shells out to nix; CI wraps the
# whole invocation instead.
SHELL := /usr/bin/env bash

# The window the checked-in examples cover.
RANGE := 2026-10-18..2026-10-20

.PHONY: install setup test build ci console fmt lint examples clean

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

## Regenerate the checked-in example reports from the synthetic tree.
##
## The tree is written through the real Log, so it is a genuine chained log
## rather than an imitation that drifts when the schema moves.
examples: build
	rm -rf examples/log
	cargo run --quiet --release --example demo -- examples/log
	target/release/media-logger --root examples/log verify
	target/release/media-logger --root examples/log show $(RANGE) > examples/report.txt
	target/release/media-logger --root examples/log show $(RANGE) --format html -o examples/report.html
	target/release/media-logger --root examples/log show $(RANGE) --format pdf  -o examples/report.pdf
	target/release/media-logger --root examples/log show $(RANGE) --format csv  > examples/report.csv
	target/release/media-logger --root examples/log show $(RANGE) --format xlsx -o examples/report.xlsx
	target/release/media-logger --root examples/log show 2026-10-19 --format pdf -o examples/one-day.pdf
	target/release/media-logger --root examples/log show '2026-10-19T22:30' > examples/instant.txt

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
