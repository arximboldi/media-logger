# media-logger — design

- **Status:** draft (language choice pending confirmation, see [Open questions](#open-questions))
- **Date:** 2026-10-22
- **Author:** Juan Pedro Bolívar Puente

## Context

A neighbour has made unspecific noise complaints and has implied she is keeping
a *Lärmprotokoll*. If those complaints ever become specific — a date, a time, an
allegation of volume — there is currently no way to answer them with anything
but memory.

This tool keeps a continuous, tamper-evident record of what media was played on
my machines, through which output device, and at what volume setting, so that a
specific allegation can be met with a specific answer: either *nothing was
playing then*, or *this was playing, at this setting, and here is the setup
reproduced and measured*.

The log is written for an audience that is not me: a landlord, a lawyer,
possibly a court. That shapes two requirements that a personal listening log
would not have — it must be credible as a record produced at the time, and it
must be complete enough to *reproduce* the conditions it describes.

### What the log must be able to answer

| Allegation | What answers it |
| --- | --- |
| "There was loud music on the night of the 3rd at 23:40." | Heartbeat records showing no player active in that window. |
| "It is loud every evening." | Timeline across weeks showing playback distribution against Ruhezeiten. |
| "The music on the 3rd was unbearably loud." | Track, output device, and volume for that moment — reproduced and measured with an SPL meter under controlled conditions. |
| "You wrote this log afterwards." | Hash chain anchored to an RFC 3161 timestamp from a third party, dated before the dispute. |

### What it explicitly cannot do

It cannot establish what the sound level was at the neighbour's wall at the
time. Nothing short of a calibrated measurement in her flat could, and that is
not something this tool attempts. It establishes *what was played and how*, from
which the conditions can be rebuilt and then measured.

## Non-goals

These were considered and deliberately rejected. They are recorded here so they
are not silently reintroduced later.

**Continuous loudness measurement.** An earlier design sampled RMS/LUFS from the
sink monitor. Rejected: digital sink level is a deterministic function of
(track, volume, sink), all of which are already logged, so it carries no
independent information. It also cannot be converted to room SPL without a
calibrated measurement, which makes it unanchorable — a stream of numbers whose
scale nobody can interpret. Reproduction with a real SPL meter is strictly
better evidence. Dropping it also removes any audio capture from the design,
and with it the §201 StGB exposure of recording in a shared building.

**Per-application audio stream logging.** PipeWire sees every stream — Telegram
notifications, Slack pings, video calls. Those are not what generates
complaints, and logging them produces a detailed record of private
communications activity for no evidentiary gain. Scope is deliberately limited
to media playback as reported via MPRIS.

**Any capture of audio samples.** Only metadata and numeric settings are stored.
No audio is recorded, buffered, or analysed at any point.

## Architecture

Three sources feed one append-only log.

```
  MPRIS (D-Bus session bus)        PipeWire (pw-dump)         wall clock
   player appear/disappear          default sink identity       heartbeat
   playback status                  sink volume (linear)        every 60s
   track metadata                   device API / BT profile
   player-stage volume
            │                              │                        │
            └──────────────┬───────────────┴────────────────────────┘
                           ▼
                   event normalisation
                           ▼
         append-only JSONL  +  SHA-256 hash chain
                           ▼
              daily rollover → RFC 3161 timestamp
                           ▼
                    query / report CLI
```

### MPRIS watcher

The primary sensor. Watches the session bus directly — **not** via `playerctld`,
which exposes only the currently active player and would silently drop a second
player running at the same time.

- At startup, `ListNames` and attach to every existing `org.mpris.MediaPlayer2.*`.
- Subscribe to `NameOwnerChanged` to catch players appearing and disappearing.
- Per player, subscribe to `PropertiesChanged` on `org.mpris.MediaPlayer2.Player`.

Two MPRIS behaviours to handle explicitly:

- `Position` is excluded from `PropertiesChanged` by the specification. It must
  be fetched with an explicit `Get` — done at heartbeat time, not polled tightly.
- Some players emit metadata changes without a corresponding status change.
  Treat metadata and status as independent streams.

**Player coverage, verified on this machine (2026-10-22):**

| Player | MPRIS | Note |
| --- | --- | --- |
| mpd (via mpdris2-rs) | yes | currently the only player on the bus |
| VLC 3.0.23 | yes | `libdbus_plugin.so` present; metadata sparse for untagged files |
| SMPlayer 25.6.0 | yes, gated | binary exports `Mpris2`, `use_mpris2`; must be enabled in Interface preferences |
| Zen / Firefox | yes | `media.hardwaremediakeys` unset → default enabled |
| Chromium | yes | enabled by default on Linux |
| **mpv 0.41.0** | **no** | **`mpv-with-scripts` has an empty scripts dir and no mpris references — currently invisible to the logger** |

The mpv gap is a prerequisite, not a future improvement. Fix:

```nix
mpv-unwrapped.wrapper { scripts = [ pkgs.mpvScripts.mpris ]; }
```

### PipeWire state sampler

Secondary. Sampled on demand — at each MPRIS event and at each heartbeat — not
streamed continuously. `pw-dump` emits JSON; no PipeWire bindings are required.

Captures: default sink `node.name`, `channelVolumes`, `mute`, `device.api`, and
for Bluetooth sinks the MAC and active profile.

### Event log

Append-only JSONL, one file per day per host, with a SHA-256 chain across lines
and across day boundaries.

### Integrity anchoring

Daily rollover computes the chain head and obtains an RFC 3161 timestamp token
from a third-party TSA. See [Integrity](#integrity).

### Query / report CLI

Reads the JSONL, verifies the chain, reconstructs state at a point in time, and
renders. A read-only consumer of the log — never a writer.

## Event schema

JSONL. One event per line. Every line carries `prev`, the SHA-256 of the
previous line's exact bytes.

```json
{
  "v": 1,
  "ts": "2026-10-22T23:14:07.221+02:00",
  "ts_utc": "2026-10-22T21:14:07.221Z",
  "host": "ce2",
  "type": "playback",
  "data": { },
  "prev": "9f2c…"
}
```

`v` is the schema version. Consumers must ignore unknown fields in `data` and
unknown `type` values, so new event types can be added without invalidating
older logs or breaking existing readers.

Both local and UTC timestamps are recorded. Ruhezeiten are local-wall-clock
boundaries, so local time is what matters for reporting; UTC disambiguates the
repeated hour at the DST transition, which is exactly the kind of detail an
opposing reading would pick at.

### Event types

| `type` | Emitted when | Key fields |
| --- | --- | --- |
| `playback` | Track, status, or volume changes | `player`, `status`, `track`, `player_volume`, `sink` |
| `heartbeat` | Every 60s unconditionally | `players[]` (empty when idle), `sink`, `position` |
| `player_appeared` / `player_gone` | Bus name acquired / lost | `player`, `bus_name` |
| `sink_changed` | Default sink or its volume changes | `sink`, `previous` |
| `daemon_start` / `daemon_stop` | Service lifecycle | `version`, `reason` |
| `suspend` / `resume` | systemd sleep transitions | — |
| `rollover` | Day boundary | `chain_head`, `line_count`, `date` |
| `anchor` | Timestamp token obtained | `chain_head`, `tsa`, `stamped_at`, `queued` |

### Heartbeats

The heartbeat is the load-bearing design element, and the reason the log can
prove a negative.

With change-events only, a gap in the file is ambiguous — it could mean silence,
or it could mean the daemon was dead. "Nothing was playing at 23:40" is the most
valuable claim this log can support, and only a positive record saying *the
logger was alive at 23:40 and nothing was playing* supports it. `daemon_start`,
`daemon_stop`, `suspend`, and `resume` close the remaining gaps.

Cost at 60s: ~1440 lines/day, roughly 300–500 KB/day, ~150 MB/year. Acceptable.

## Reproduction fidelity

Because reproduction is the evidentiary mechanism, the log must capture
everything needed to rebuild the conditions exactly. Four ways that silently
breaks:

**1. Volume scale.** PipeWire's `channelVolumes` is *linear gain*; every UI
displays a *cubic* scale. They agree only at unity, which is why the discrepancy
is easy to miss — a slider at "50%" is `0.125` linear. Store the raw linear
float and record the scale explicitly. Logging a percentage and later
reproducing by dragging a slider to that percentage can be off by ~18 dB.

**2. Two gain stages, multiplied.** The player has its own volume (mpd's
`volume`, mpv `--volume`, VLC's slider, a browser tab's) and the sink has
another. MPRIS reports the first, PipeWire the second. Either alone reproduces
wrong. Capture both.

**3. Track identity.** Title and artist are ambiguous — the same work exists in
masters that differ substantially in level. Record `xesam:url` and
`mpris:trackid` alongside the human-readable tags. For streamed content capture
the URL/ID, accepting that it may rot.

**4. Sink identity and chain state.** Record the stable `node.name`
(`alsa_output.usb-Focusrite_...`), never the node index, which changes across
reboots. For Bluetooth record MAC and profile — `a2dp-sink` versus headset is a
large level difference. Also record mpd's `replaygain` mode: it is off today, but
if enabled the output level becomes track-dependent and reproduction breaks
without any visible change to the logged volume.

The current gain chain on `ce2` is clean — no EasyEffects, no filter chains, mpd
replaygain unset. If a DSP stage is ever added it must be captured, or
reproduction silently diverges.

### A note on the two machines

`ce2` runs a Focusrite Scarlett 4i4 in pro-audio mode: PipeWire volume is pinned
at `1.00` and loudness is governed by the analog monitor knob, which is
invisible to software. On that host the logged volume is **not** informative
about loudness, and reproduction requires the knob position to be noted
manually.

The primary machine uses Bluetooth speakers, where AVRCP absolute volume maps
the software volume onto the speaker's own gain. There the logged volume is
meaningful. The same recorded "60%" therefore means different things on the two
hosts — which is why `host` and `device.api` are mandatory fields on every
record.

## Integrity

A hash chain alone proves nothing against the person who holds all the inputs —
it can always be recomputed. Its value is entirely in anchoring the head to a
third party. The chain is therefore kept cheap; the anchoring is where the
evidentiary weight sits.

### Chain

Each line's hash is SHA-256 over its **exact on-disk UTF-8 bytes**, excluding
the trailing newline. The next line's `prev` is that hash.

Hashing the literal bytes rather than a re-serialised object avoids
canonicalisation entirely — no stable key ordering to maintain, no separator
conventions to preserve across versions, and no risk of the verifier disagreeing
with the writer about a log that is in fact intact. It also decouples the chain
from the schema, so new fields never affect verification.

The chain **crosses day boundaries**: the first line of each day carries the
last hash of the previous day. Per-file hashes would not detect the deletion of
an entire inconvenient day; a continuous chain does.

Each append is `fsync`'d. A hole torn in the chain by a power cut is
indistinguishable from tampering, and that is not a conversation worth having.

### Anchoring

Daily, at rollover, the chain head is timestamped under RFC 3161. Only a 70-byte
SHA-256 digest leaves the machine — the log contents never do. This was the
decisive advantage over committing and pushing to a git remote, which would
publish listening history to a third party.

```sh
# stamp
openssl ts -query -data head.txt -sha256 -cert -out req.tsq
curl -H "Content-Type: application/timestamp-query" \
     --data-binary @req.tsq http://zeitstempel.dfn.de -o 2026-10-22.tsr

# verify, offline, any time later
openssl ts -verify -data head.txt -in 2026-10-22.tsr -CAfile dfn-chain.pem
```

**Primary TSA: `zeitstempel.dfn.de`.** Verified working 2026-10-22; signs as
*DFN-CERT Services GmbH, Hamburg*, chaining to *DFN-Verein Community Root CA
2022* — a named German institution with a legal entity behind it, which is what
matters when the audience is a German landlord or court.

**Secondary TSA: `timestamp.digicert.com`.** One extra request gives a second
independent attestation in another jurisdiction, removing the single point of
trust for no meaningful cost. (`freetsa.org` also works but its certificate
lists a private `mailbox.org` address, which undercuts the institutional
credibility that is the entire point.)

Operational requirements:

- **Pin the CA chain in-repo** as `anchors/dfn-chain.pem`. Verification months
  later must not depend on hunting for a root certificate.
- **Keep `-cert`** in the query so the signer certificate is embedded in the
  token and it stays self-contained after "Zeitstempel 2026" expires.
- **If offline at rollover, queue — never backdate.** Stamp when connectivity
  returns and record the true stamp time with `queued: true`. A log that
  silently claims same-day stamping it did not get is worse than one with an
  honest gap.

**Limitation, stated plainly:** these are not *qualified* trust service
providers under eIDAS, so the timestamps carry no automatic legal presumption.
They are signed third-party attestations from credible institutions — far short
of notarisation, far beyond nothing, and proportionate to a noise dispute.

## Reporting

The output that persuades is not a spreadsheet. It is a per-day timeline with
Ruhezeiten shaded and playback periods overlaid, rendered to PDF.

Ruhezeiten are configurable, defaulting to 22:00–06:00. Mittagsruhe
(13:00–15:00) and Sunday rules vary by *Hausordnung* and must be set from the
actual house rules rather than assumed.

### Commands

Two verbs beyond the daemon itself.

```
media-logger run                                   # the daemon
media-logger verify [<when>]                       # chain + anchor integrity
media-logger show <when> [--format F] [-o FILE]    # everything else
```

Querying, inspecting a moment, rendering a timeline, and exporting are the
same operation at different widths and output formats, so they are one command
rather than three. `show` always performs the same three steps:

1. **Select** a window from `<when>`.
2. **Reconstruct** the state in force at the window's start, by replaying
   events up to it — what was playing, which sink, both gain stages.
3. **Render** the reconstructed state plus the events within the window, in the
   requested format.

`<when>` accepts an instant (`2026-10-22T23:14`), a day (`2026-10-22`), a range
(`2026-10-01..2026-10-31`), or a shorthand (`yesterday`, `last-night`). An
instant is simply a zero-width window, which is why "what was playing at 23:14"
needs no separate command — it is the reconstruction step with nothing to list.

`--format` is `text` (default), `json`, `jsonl`, `csv`, `html`, `pdf`, or
`xlsx`. The timeline is not a distinct query but a *rendering*: `--format pdf`
over a multi-day window produces the Ruhezeiten view. `xlsx` stays deferred
until someone asks, but it is now a renderer to add rather than a command to
design.

**Every rendered output carries a verification footer** — chain status, the
anchoring TSA, and the timestamp token's time. A PDF handed to a landlord
should state on its face that it is verifiable and by whom; a report that
requires a separate `verify` run to be trusted will be circulated without one.

## Decisions

| Decision | Rationale |
| --- | --- |
| MPRIS as primary sensor, not PipeWire streams | Covers music/film/browser video — the complaint-generating set — without recording private communications activity. |
| Watch the bus directly, not `playerctld` | `playerctld` exposes only the active player; concurrent players would be lost. |
| No loudness measurement | Redundant given (track, volume, sink); uninterpretable without calibration; reproduction with an SPL meter is better evidence. |
| JSONL, not SQLite | Append-only, greppable, diffable, trivially extensible; byte-level stability is what makes the hash chain simple. An index can be derived later if queries get slow. |
| Hash over literal bytes | Eliminates canonicalisation as a failure mode and decouples integrity from schema evolution. |
| Chain across days | Detects deletion of a whole day, which per-file hashing cannot. |
| RFC 3161 over git push | Only a hash leaves the machine; no listening history published. |
| DFN as primary TSA | Named German institution; legible to the actual audience. |
| `pw-dump` subprocess over PipeWire bindings | Only point-in-time sink state is needed; bindings would be cost without benefit. |
| Heartbeat every 60s | The only way to prove absence of playback, which is the most valuable claim available. |
| One `show` command, not `show`/`timeline`/`export` | All three are select → reconstruct → render at different widths and formats. Separate commands would duplicate the first two steps and make a new output format a new command rather than a new renderer. |
| NixOS module, not home-manager | Not in use. The unit must be a *user* service to reach the session bus, which a NixOS module can declare. |
| Verification footer on every rendered output | A report that needs a separate `verify` run to be trusted will be circulated without one. |

## Implementation plan

### Prerequisites

Host configuration changes, independent of the code. Both are silent failures —
the daemon runs correctly and records nothing — so they must land before the
log is relied upon:

1. **Add `mpvScripts.mpris` to the mpv wrapper on both hosts.** Until this
   lands, mpv playback leaves no trace.
2. **Enable MPRIS2 in SMPlayer preferences.** Compiled in, off by default.

### Phase 1 — daemon core (1–2 days)

Repo layout follows `nix-flake-organization` (thin `flake/`, implementation
under `src/`) with the Cargo workspace rooted at the repo root:

```
flake.nix                      # minimal entrypoint
flake/
  packages/default.nix
  modules/nixos/default.nix
  checks/default.nix
Cargo.toml                     # workspace
src/media-logger/
  Cargo.toml
  src/
    main.rs                    # CLI dispatch, daemon wiring
    mpris.rs                   # player discovery + property streams
    audio.rs                   # pw-dump sampling → sink snapshot
    event.rs                   # schema types + serialisation
    log.rs                     # append, chain, rollover, verify
    anchor.rs                  # RFC 3161 stamp + verify
    query.rs                   # window selection + state reconstruction
    render.rs                  # output formats behind one interface
scripts/                       # install, setup, run, test, build, ci (ADR-0002)
docs/design.md
anchors/dfn-chain.pem
README.md                      # Standard Readme
```

Each module owns a complete operation — `log.rs` exposes `append`, `verify`,
`roll` rather than exposing chain internals to callers.

Deliverable: daemon writes a valid chained JSONL with heartbeats on `ce2`.

### Phase 2 — packaging and deployment (half day)

Flake package via `crane`, plus a **NixOS module** declaring
`systemd.user.services.media-logger` with `Restart=always`, and log directory
and Ruhezeiten as module options.

It must be a *user* service: the daemon reads the session bus, which a
system-level service cannot reach. A NixOS module can still define user
services, so dropping home-manager costs nothing here — the unit is declared
system-wide and instantiated per session.

Deliverable: running unattended on both machines, surviving reboot and suspend.

### Phase 3 — integrity anchoring (half day)

Rollover, stamping against both TSAs, the offline queue, `verify`, and the
pinned CA chain. Shells out to `openssl ts` rather than taking an RFC 3161 crate
— proven working, zero dependency risk, and a third party verifies with the same
standard command.

Deliverable: `media-logger verify` validates chain and anchors end to end.

### Phase 4 — reporting (1 day)

`show` end to end: window selection, state reconstruction, Ruhezeiten
configuration, and the `text` / `json` / `csv` / `html` / `pdf` renderers behind
one interface, each emitting the verification footer.

Deliverable: a PDF that can be handed to a landlord without further explanation.

### Phase 5 — deferred

`xlsx` renderer, richer search, additional event types. Build on demand,
not speculatively.

### Testing

Per `when-to-write-tests`, the default is no test and verification is running
the thing. Tests are warranted for exactly two contracts here, both of which are
public, both of which fail silently if wrong:

- **Chain verification** — append a known sequence, verify; mutate one line,
  confirm verification fails at that line; confirm a day-boundary deletion is
  detected.
- **Volume scale conversion** — linear↔cubic round-trip, since a silent error
  here is an ~18 dB reproduction error with no visible symptom.

## Legal and privacy notes

Not legal advice.

- **No audio is recorded.** Metadata and numeric settings only. This keeps the
  tool clear of §201 StGB, which would be a live concern for anything capturing
  sound in a shared building.
- **The log is personal data about me** and a detailed record of my habits. It
  stays local; only hashes are transmitted. It should only be disclosed
  selectively — a single day's extract plus its anchor, not the archive.
- **The record is double-edged.** If playback did occur late at night, the log
  documents that faithfully. That is the price of a log that is credible at all,
  and it is the right trade, but it should be understood before relying on it.
- **Highest-leverage step is not technical:** ask the landlord to put complaints
  in writing with dates and times. Vague complaints cannot be rebutted; specific
  ones can. This tool is only useful against specific ones.

## Open questions

1. **Language — needs confirmation.** The plan above assumes **Rust with
   `zbus`**. Rationale: the work is D-Bus-dominated and zbus's `#[proxy]` macro
   generates typed MPRIS clients, making that layer *smaller* than the Python
   equivalent; it is at 6.0.0 with ~33M recent downloads and active maintenance;
   a single binary suits a 24/7 unattended daemon on two hosts; and the house
   skills repo carries Rust guidance and none for Python. Against: Python
   iterates faster for casual self-extension, and `dbus-fast` (2.46.3) is a
   solid foundation — though it is the *only* maintained D-Bus library in
   nixpkgs, with `pydbus`, `dbus-next`, and `dasbus` all effectively dead.
   **Switch to Python if casual hacking on this matters more than deployment
   robustness.**
2. Ruhezeiten for this building — needs the actual *Hausordnung*, particularly
   whether Mittagsruhe and all-day Sunday rules apply.
3. Retention. Indefinite is the default; there is no reason to discard, but a
   policy should be stated rather than drifted into.
4. Whether to log the Scarlett monitor knob position manually on `ce2` when it
   changes, given software cannot see it.
