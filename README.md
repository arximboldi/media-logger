# media-logger

A tamper-evident log of what media played on this machine, through which output device, at what volume.

Built to answer a specific noise allegation with a specific answer: either nothing was
playing then, or this was playing, at this setting, and here are the conditions reproduced
and measured. See [docs/design.md](docs/design.md) for the reasoning behind every choice
here, including what the log deliberately does *not* record.

## Install

Requires [Nix](https://nixos.org/download) with flakes enabled.

```sh
nix develop          # or direnv; every make target assumes you are inside
make setup
make build
```

## Usage

The daemon writes only to `<root>/<hostname>/`, so several machines can share one
replicated tree without ever contending.

```sh
# run the logger (heartbeat defaults to 10s)
make run ARGS="--root ~/logs run"

# check the hash chain over this host's tree
make run ARGS="--root ~/logs verify"
```

`--root` can also come from `MEDIA_LOGGER_ROOT`, and the host name from
`MEDIA_LOGGER_HOST`. The daemon **refuses to start** when the root is missing, because an
unmounted share is indistinguishable from a fresh install and guessing wrong would start a
new chain while the real history sits unmounted.

### Layout

```
<root>/<host>/2026-10/2026-10-22.jsonl
```

One JSON object per line, chained by SHA-256 over each line's literal bytes, continuing
across day boundaries so that deleting a whole day is detectable.

```json
{"v":1,"ts":"2026-10-22T23:14:07.221+02:00","ts_utc":"2026-10-22T21:14:07.221Z",
 "host":"ce2","type":"heartbeat","data":{...},"prev":"9f2c…"}
```

Heartbeats are written unconditionally, including when nothing is playing. That is the
point: proving no media played at a given time needs positive evidence that the logger was
alive and idle, not merely an absence of records.

A beat whose state matches the previous one is abbreviated to
`{"unchanged":"<fingerprint>"}` rather than skipped — skipping would reintroduce the very
ambiguity heartbeats exist to remove. A full beat is written on every change and at least
once a minute regardless.

## Development

Per [ADR-0002](https://github.com/darkmatter/skills/blob/main/docs/adr/0002-standard-project-command-surface.md),
this repo exposes the standard darkmatter command surface:

| Target | Purpose |
| --- | --- |
| `make install` | Check host prerequisites |
| `make setup` | Fetch dependencies |
| `make run` | Run the CLI (`ARGS="…"`) |
| `make test` | Run the test suite |
| `make build` | Build the release binary |
| `make ci` | Full gate: fmt, clippy, test, build |
| `make console` | Enter the devshell |

Every target assumes it is already running inside the devshell — enter it with
`nix develop` (or direnv) first. `make console` is the only target that invokes nix, since
entering the shell is its whole purpose. GitHub Actions wraps the invocation as
`nix develop -c make ci`, and also runs `nix flake check`.

## Contributing

Personal project. Issues and PRs are not expected, but corrections to the integrity model
are very welcome — that is the part where being wrong is expensive.

Run `make ci` before any commit.

## License

Not licensed for redistribution. All rights reserved.
