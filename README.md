# media-logger

A tamper-evident log of what media played on this machine, through which output device, at what volume.

Built to answer a specific noise allegation with a specific answer: either nothing was
playing then, or this was playing, at this setting, and here are the conditions reproduced
and measured. See [docs/design.md](docs/design.md) for the reasoning behind every choice
here, including what the log deliberately does *not* record.

## Install

Requires [Nix](https://nixos.org/download) with flakes enabled.

### As a service (NixOS)

```nix
{
  inputs.media-logger.url = "github:arximboldi/media-logger";

  # in your configuration
  imports = [ inputs.media-logger.nixosModules.default ];

  services.media-logger = {
    enable = true;
    logsRoot = "%h/sync/media-logs";   # systemd specifiers expand
    heartbeat = 10;                     # seconds
  };
}
```

Runs as a *user* service, because the daemon reads the session bus. `logsRoot` is
created if missing, and the creation is recorded in the log as a `chain_restart`
so a start in the wrong place is visible rather than silent.

### From source

```sh
nix build .#media-logger     # wrapped binary at ./result/bin/media-logger

nix develop                  # or direnv; every make target assumes you are inside
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
`MEDIA_LOGGER_HOST`. A missing root is created; whenever a chain begins without a
predecessor the first line is a `chain_restart` naming the reason, so `verify` reports a
declared discontinuity rather than corruption.

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

### Integrity

Each closed day is timestamped by two authorities and the tokens are stored
beside it:

```
2026-10-21.jsonl  2026-10-21.dfn.tsr  2026-10-21.digicert.tsr
```

Only a SHA-256 digest is sent; the log content never leaves the machine. Days
are stamped at startup and hourly, so an offline machine simply stays pending
and is picked up later — nothing is backdated.

```sh
make run ARGS="--root ~/logs verify"
```

```
chain intact

anchors:
  2026-10-21  dfn OK 2026-10-22T00:40:00Z | digicert OK 2026-10-22T00:40:01Z
  2026-10-22  dfn — | digicert —  (today, still open)
```

Editing an anchored day fails the chain check *and* both tokens, independently.
See [anchors/README.md](anchors/README.md) for the pinned roots and why the two
authorities are not interchangeable.

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
