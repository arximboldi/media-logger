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

It is a *user* service — the daemon reads the session bus, which a system
service cannot reach — and it comes up when the configuration is switched, not
only at the next login. A switch restarts it only if its command line changed,
so rebuilding for unrelated reasons leaves no gap in the log.

Runs as a *user* service, because the daemon reads the session bus. `logsRoot` is
created if missing, and the creation is recorded in the log as a `chain_restart`
so a start in the wrong place is visible rather than silent.

### From source

```sh
nix build .#media-logger          # wrapped binary at ./result/bin/media-logger
install -Dm755 result/bin/media-logger ~/.local/bin/media-logger
```

The wrapper puts PipeWire, openssl, curl and weasyprint on `PATH`, so the
binary works wherever you put it. The examples below assume it is on yours.

## Usage

The daemon writes only to `<root>/<hostname>/`, so several machines can share one
replicated tree without ever contending.

```sh
# run the logger (heartbeat defaults to 20s)
media-logger --root ~/logs run

# check the hash chain and the anchors over this host's tree
media-logger --root ~/logs verify
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

A run of unchanged beats collapses into a single line, rewritten in place as the run grows,
naming how many samples it stands for and when the run began:

```json
{"type":"heartbeat","data":{"players":[],"sink":{…},"beats":15,"since":"…23:40:00+02:00"}}
```

Fifteen identical lines say no more than one line saying "fifteen samples, no change", and
cost fifteen times as much to keep. Because the line is rewritten on every beat, the log is
never more than one interval behind — a crash loses a single beat, not the run.

Only the open day's last line is ever rewritten. A closed day is immutable, and it is
closed days that get stamped.

### Reporting

```sh
media-logger --root ~/logs show last-night
media-logger --root ~/logs show 2026-10-20 --format pdf -o oct20.pdf
media-logger --root ~/logs show 2026-10-01..2026-10-31 --format csv
media-logger --root ~/logs show 2026-10 --web
```

`<when>` is an instant (`2026-10-22T23:14`), a day, a month (`2026-10`), a range
(`a..b`, mixing any of those), or `today` / `yesterday` / `last-night`. Leave it out —
or say `all` — for everything the tree holds. All four are the same operation at
different widths: select a window, reconstruct the state in force when it
opened, render what happened inside. `--format` is `text` (default), `json`,
`jsonl`, `csv`, `html`, `pdf` or `xlsx`; `pdf` and `xlsx` need `-o`. `--web` is
`--format html` into a temporary file, opened in a browser — the view with the
filters and the hover readouts, for looking rather than sending.

Reports cover **every host** in the tree by default — a document that quietly omitted a
machine would be worse than one that is too broad. Narrow with `--host`, `--player` or
`--device`, each repeatable, and `--during quiet` for only the playback that
touched Ruhezeiten — which is usually the document someone else wants to see.

A report says what it can speak for. The daemon beats whether or not anything is
playing, so a day of beats with no playback is evidence of silence — but a day with
no beats is only evidence that nothing was running. Days with no record are hatched
in the calendar, counted above the data, and never described as quiet.

A window longer than a week also gets a calendar above the timeline, one cell per day
shaded by the loudest level held for five minutes. The shading spans the days on
screen and follows the filters, so the ramp is spent on the differences actually
present rather than on a guessed interval; `--gain-scale 0.3:0.7` fixes the bounds
instead, for when two reports have to be compared to each other. The timeline is never suppressed; in
HTML, clicking a day narrows the view to it.

Quiet hours default to German Nachtruhe (22:00–06:00). Mittagsruhe and Sunday
rules come from your Hausordnung and are not assumed — add them with
`--quiet-hours '13:00-15:00' --quiet-hours 'sun 00:00-24:00'`, or set
`services.media-logger.quietHours` and they reach the shell too.

Every format carries the integrity footer, so a report is checkable on its face.

[`examples/`](examples/) holds generated reports in every format, built from a synthetic
tree — `make examples` regenerates them. The data there is fabricated and is labelled as
such.

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
media-logger --root ~/logs verify
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
| `make test` | Run the test suite |
| `make build` | Build the release binary |
| `make examples` | Regenerate the checked-in example reports |
| `make ci` | Full gate: fmt, clippy, test, build |
| `make console` | Enter the devshell |

Every target assumes it is already running inside the devshell — enter it with
`nix develop` (or direnv) first. `make console` is the only target that invokes nix, since
entering the shell is its whole purpose. GitHub Actions wraps the invocation as
`nix develop -c make ci`, and also runs `nix flake check`.

There is no `make run`, which ADR-0002 would otherwise call for: wrapping a CLI in
`make run ARGS="…"` is worse than calling it, and a target nobody should use invites
being used. In the devshell, `cargo run -- …`; otherwise the built binary.

## Contributing

Personal project. Issues and PRs are not expected, but corrections to the integrity model
are very welcome — that is the part where being wrong is expensive.

Run `make ci` before any commit.

## License

MIT. See [LICENSE](LICENSE).
