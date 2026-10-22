# media-logger — design

- **Status:** accepted
- **Implementation:** Rust; phases 1–6 complete
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
   playback status                  sink volume (linear)        every 20s
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

**Sampled, not subscribed — and the log must not imply otherwise.** The sink is
read at playback events and at heartbeats, so `sink_changed` carries the time
the change was *detected*, bounded by the heartbeat interval, not the time it
occurred. A volume change that happens and reverts between two samples leaves
no trace at all. In a record meant to withstand being picked at, implied
precision that is not there is a liability, so this is stated rather than left
to be inferred.

The volume in force at any moment is therefore carried on **every** `playback`
and full `heartbeat` record, not only on `sink_changed`. The setting when a
track started is exactly what reproduction needs, and a transition-only event
would not record it.

Subscribing via `pw-mon` would make the timestamps mean what they appear to
mean, and is the natural way to close this if the 20s window proves too wide.

### Event log

Append-only JSONL, one file per day per host, with a SHA-256 chain across lines
and across day boundaries. See [Log layout](#log-layout).

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
| `heartbeat` | Every 20s; a run of unchanged beats is one line, rewritten | `players[]` (empty when idle), `sink`, `state`, `beats`, `since` |
| `player_appeared` / `player_gone` | Bus name acquired / lost | `player`, `bus_name` |
| `sink_changed` | A sample differs from the previous one | `sink`, `previous`. Timestamp is **detection** time, not occurrence — see below |
| `daemon_start` / `daemon_stop` | Service lifecycle | `version`, `reason` |
| `suspend` / `resume` | systemd sleep transitions | — |
| `rollover` | Day boundary | `chain_head`, `line_count`, `date` |
| `anchor` | Timestamp token obtained | `chain_head`, `tsa`, `stamped_at`, `queued` |
| `chain_restart` | No prior chain head found at startup | `reason`, `last_known_head` (if any) |

### Heartbeats

The heartbeat is the load-bearing design element, and the reason the log can
prove a negative.

With change-events only, a gap in the file is ambiguous — it could mean silence,
or it could mean the daemon was dead. "Nothing was playing at 23:40" is the most
valuable claim this log can support, and only a positive record saying *the
logger was alive at 23:40 and nothing was playing* supports it. `daemon_start`,
`daemon_stop`, `suspend`, and `resume` close the remaining gaps.

#### Collapsing runs into one rewritten line

A run of unchanged beats is a single line, replaced in place as the run
grows, naming how many samples it stands for and when it began:

```json
{"type":"heartbeat","data":{"players":[],"sink":{…},"beats":15,"since":"…23:40:00+02:00"}}
```

Fifteen identical lines said no more than one line saying *fifteen samples,
no change*, and cost fifteen times as much to keep. Because the line is
rewritten on every beat, the log is never more than one interval behind: a
crash costs a single beat, not the run. ~24 KB/day against ~2.37 MB/day when
every beat was its own line, with the same 20s resolution and one parameter
instead of two.

**Only the open day's last line is ever rewritten**, and only while it is
still the last line — any other event appended after it ends the run. A
closed day is immutable, and closed days are what get stamped.

An intermediate design wrote a fresh line every N seconds and collapsed
whatever had happened in between. It was discarded: it lost up to N seconds
of liveness to a crash, and it needed a second interval to tune, which made
the timing harder to reason about for no gain.

A run is also capped at an hour, in code rather than configuration. A crash
during the replacing write corrupts that line and takes the run with it —
the window is one `write` wide and the risk small, but an unbounded run
would put a whole undisturbed night in a single line. An hourly ceiling caps
what one mishap can cost, for 24 lines a day.

#### Is a rewritten line still evidence?

Rewriting was initially rejected on the grounds that *"every line, once
written, never changes"* is a simple claim a lay reader can check, and
caveating it costs credibility.

That reasoning does not survive contact with what the log already does.
`resume` truncates a damaged trailing line after a crash, and the open day is
appended to continuously — the tail was never immutable. The claim that
actually carries weight is narrower and still exactly true: **a day, once
closed and stamped, never changes.** Rewriting the open tail does not touch
it, and the chain still detects any edit to a line that is no longer last,
which is every line that matters.

## Log layout

```
<logs-root>/<host>/2026-10/2026-10-22.jsonl
                           2026-10-22.dfn.tsr
                           2026-10-22.digicert.tsr
```

Monthly directories keep a browsable ~30 entries each and make a month the
natural unit to archive or hand over. All components are zero-padded so
lexicographic order is chronological order.

**Host is the top level** because each host runs its own daemon with its own
independent hash chain; they cannot share a namespace. It also makes each
host's tree self-contained for verification.

**The date is spelled in full in the filename**, redundantly with the
directory. These files get detached from their context — attached to an email,
handed to a lawyer, quoted in a reply to the landlord — and `22.jsonl` means
nothing on its own.

**Timestamp tokens sit beside the log, not in a separate tree.** Browsing a
month shows each day with its proof, and extracting a day for a third party is
then copying adjacent files rather than remembering to fetch the proof from
somewhere else. A proof that is easy to leave behind gets left behind.

**No per-run file suffix.** Restarts are recorded *in* the log as
`daemon_stop` / `daemon_start` with heartbeats bracketing the gap; a filename
cannot say why the daemon stopped, and encoding runs in the name would
duplicate that for nothing. A suffix would also break chronological sorting —
`-` (0x2D) sorts before `.` (0x2E), so `22-2.jsonl` precedes `22.jsonl`, which
would make an intact chain look corrupt. The daemon appends to the day's file
across restarts, reading the last line's hash on startup. A partial line left
by a crash is detected and truncated at startup rather than starting a new
file.

Files are named by **local** date, since Ruhezeiten are local-wall-clock
boundaries. The October DST file therefore contains 25 hours and the March one
23; `ts_utc` on each record disambiguates the repeated hour.

### Replication

`<logs-root>` lives in a Syncthing share, which gives offsite replication and
makes cross-host reporting a local read.

**One Syncthing folder per host tree**, configured *Send Only* on the host that
owns it and *Receive Only* everywhere else. One folder covering all hosts would
also work, but per-host folders make it structurally impossible for one machine
to alter another's records — worth having for an archive whose value depends on
not having been edited. It also removes conflict files entirely, since exactly
one writer exists per folder.

Enable **staggered versioning** on the receive-only replicas. If a file is ever
altered, the prior version survives on another machine, independently of the
hash chain.

**Syncthing is replication, not integrity and not backup.** A deletion
propagates. Integrity rests entirely on the hash chain and the RFC 3161
anchors; Syncthing only ensures the record exists in more than one place and is
convenient to read from either.

The current day's file changes on every heartbeat, so it re-syncs about once a
minute. Syncthing transfers only changed blocks, so an append costs little.

### No durable local state

**The log tree is the only durable state.** `$XDG_STATE_HOME/media-logger/`
holds a lock file to prevent two daemons writing one tree, and nothing else.
Losing or resetting it costs nothing.

Two things that look like they need storing do not:

- **The chain head** is the hash of the last line of the most recent log file.
  Reading one line at startup is free. A cached copy would buy nothing and
  introduce a failure mode where losing it silently restarts the chain —
  indistinguishable from tampering.
- **Pending anchors** are any `YYYY-MM-DD.jsonl` without a matching `.tsr`
  beside it. The filesystem already records this; a queue file would duplicate
  it and could disagree with it.

Deriving both from the tree means there is no second source of truth to fall
out of sync with the first.

### Losing the log tree

Disk restore, an unmounted share, a fresh host. The anchors make this bounded
rather than catastrophic: each day's head is timestamped independently, so
every previously anchored day stays fully verifiable on its own. The chain
links days for ordering, but losing continuity does not retroactively weaken
what was anchored before the gap.

**A discontinuity is declared in-band.** With no prior head, the first line is a
`chain_restart` event recording the reason. Verification then reports *"chain
restarted 2026-11-03, prior segment anchored through 2026-11-02"* rather than
*"corrupt"*. The gap is visible either way, so there is nothing to gain by
hiding it — an unexplained break invites suspicion, a declared one is just
history.

**A missing `<logs-root>` is created, and the creation is recorded.** An earlier
draft refused to start, on the theory that an unmounted share is
indistinguishable from a fresh install. That guard defends against a failure
this deployment does not have: a Syncthing folder is a plain directory that
persists on disk, with no mount step to race, and each host owns its own
subtree Send Only so its history never waits on a receive. Refusing would mean
a machine logs nothing until somebody notices — and nobody checks a logger
until they need it, which is exactly too late.

What matters is not avoiding a fresh start but avoiding a *silent* one. Both a
created root and a missing host subtree begin at genesis with a logged
`chain_restart` naming which it was, so `verify` reports a declared
discontinuity. A root that exists but is not a directory is still refused,
since that is a configuration error rather than a first run.

Sync lag cannot cause a spurious restart: the daemon writes only its own host
subtree, which is Send Only, so that tree is always locally authoritative and
never awaiting a receive. Only real local data loss can trigger a restart.

## Reproduction fidelity

Because reproduction is the evidentiary mechanism, the log must capture
everything needed to rebuild the conditions exactly. Four ways that silently
breaks:

**1. Volume scale.** PipeWire's `channelVolumes` is *linear gain*; every UI
displays a *cubic* scale. They agree only at unity, which is why the discrepancy
is easy to miss — a slider at "50%" is `0.125` linear. Store the raw linear
float and record the scale explicitly. Logging a percentage and later
reproducing by dragging a slider to that percentage can be off by ~18 dB.

This includes readouts that are not on this machine at all: a Bluetooth
speaker's own front panel is a cubic UI like any other. `wpctl` is too — it
prints the cube root, so comparing `wpctl` output against `channelVolumes`
compares two different scales.

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
meaningful — measured on 2026-10-22 against a Teufel ULTIMA 25, where the
logged linear gain, the AVRCP step and the speaker's own display all agree
(see the correction under 6a). PipeWire applies no gain of its own on that
path, so what is logged is the setting the speaker is attenuating by.

The same recorded "60%" therefore means different things on the two hosts —
which is why `host` and `device.api` are mandatory fields on every record.

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
media-logger show [<when>] [--format F] [-o FILE]   # everything else
media-logger show [<when>] --web                   # ... in a browser
```

Querying, inspecting a moment, rendering a timeline, and exporting are the
same operation at different widths and output formats, so they are one command
rather than three. `show` always performs the same three steps:

1. **Select** a window from `<when>`.
2. **Reconstruct** the state in force at the window's start, by replaying
   events up to it — what was playing, which sink, every gain stage.
3. **Render** the reconstructed state plus the events within the window, in the
   requested format.

The reconstruction always happens; it is only *shown* when it says something.
For an instant it is the whole answer, including when the answer is that
nothing was playing. For a window it is worth a heading only when playback
was already under way as the window opened — a session carried over from the
night before. A window starting at midnight almost never is, so the section
was a heading above the words "nothing playing" on every report, which
teaches a reader to skip the top of the page.

`<when>` accepts an instant (`2026-10-22T23:14`), a day (`2026-10-22`), a
month (`2026-10`), a range mixing any of those (`2026-09..2026-10-05`), or a
shorthand (`yesterday`, `last-night`). An instant is simply a zero-width
window, which is why "what was playing at 23:14" needs no separate command —
it is the reconstruction step with nothing to list.

It is also **optional**: with no window, or `all`, the report covers every
day the tree holds, derived from the day files themselves. Asking for a
report of everything should not require knowing when everything started. A
bare month or day at the end of a range means the whole of it, so
`2026-08..2026-09` ends on the last day of September rather than its first.

`--format` is `text` (default), `json`, `jsonl`, `csv`, `html`, `pdf`, or
`xlsx`, and `--web` is `--format html` written to a temporary file and handed
to `xdg-open`. HTML is the only format that filters, and typing a path for a
page you are about to read once and discard is friction in the way of the
thing the filters exist for; the path is still printed, so the file is not
lost if no browser answers.

The timeline is not a distinct query but a *rendering*: `--format pdf` over a
multi-day window produces the Ruhezeiten view. `xlsx` stays deferred
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
| `<host>/<YYYY-MM>/<YYYY-MM-DD>.jsonl` | Host-first because chains are per-host; full date because evidence files get detached from their directory. |
| No per-run file suffix | Restarts are already logged as events; a suffix breaks chronological sorting (`-` < `.`) and would make an intact chain look corrupt. |
| One Syncthing folder per host, Send/Receive Only | Makes it structurally impossible for one machine to alter another's records, and eliminates conflict files by having exactly one writer. |
| No durable state outside the log tree | Chain head and pending anchors are both derivable from the tree; a cache would be a second source of truth that can disagree with the first, and losing it would silently restart the chain. |
| `chain_restart` declared in-band | The gap is visible either way. An unexplained break invites suspicion; a declared one is history. |
| Rust with `zbus` | The work is D-Bus-dominated and zbus's `#[proxy]` generates typed MPRIS clients, making that layer *smaller* than the Python equivalent. A single binary suits a 24/7 unattended daemon. Python's D-Bus ecosystem has exactly one maintained library (`dbus-fast`); `pydbus`, `dbus-next` and `dasbus` are all effectively dead. |
| A run of unchanged beats is one line, rewritten in place | Fifteen identical lines say no more than one saying "fifteen samples, no change". Rewriting keeps the log one beat behind rather than one batch behind, and needs no second interval. |
| Only the open day's tail is rewritten | The tail was never immutable — `resume` already truncates a damaged one. The load-bearing claim is that a closed, stamped day never changes, and that is untouched. |
| Runs capped at an hour, in code not configuration | Bounds what a crash during the replacing write can cost, without adding a knob nobody wants to reason about. |
| Position excluded from the beat fingerprint | It advances continuously, so including it would defeat abbreviation exactly while a track plays and lines are largest. |
| Effective sink gain is the route, not node × route | Node and route are one control seen from two objects: measured mirroring each other on both a Bluetooth and an analog sink, and moving together. Multiplying squares the gain — 41 dB low on the speaker that falsified it. Where they disagree, a node at 1.0 means no node-stage attenuation, not a second stage at unity. |
| Sink carried on every playback record | The volume when a track started is what reproduction needs; a transition-only event would not record it. |
| Exclusive lock per (root, host), in `$XDG_STATE_HOME` | Two writers would interleave lines and break the chain exactly as tampering would. Not in the tree, which is replicated and where a lock means nothing on the receiving host. |
| Days with no record are marked, never described as quiet | The heartbeat's whole purpose is that silence in the log means silence in the room, and that holds only while the daemon runs. A blank day and an unrecorded day looked identical in a report, so a reader could take "the logger was off all September" for "nothing played all September" — the exact inference this log exists to *prevent* an opponent from making. |
| Reports name the program, the log names the bus | A browser owns `org.mpris.MediaPlayer2.firefox.instance_1_181`, and the instance changes every restart. In a report that is one player per session: a filter list that grows a line a day, a new colour each time, and `--player firefox` matching nothing. The log keeps the full name, because that is what was on the bus. |
| Package wraps PipeWire onto `PATH` | Otherwise it installs fine and silently records no device — a defect shaped like missing data rather than a missing dependency. |
| Create a missing `<logs-root>`, and record it | A Syncthing folder is a plain directory with no mount step to race, so refusing would only mean a machine silently logs nothing. The risk worth guarding is a *silent* fresh start, which `chain_restart` already covers. |

## Implementation plan

### Prerequisites

Host configuration changes, independent of the code. Both are silent failures —
the daemon runs correctly and records nothing — so they must land before the
log is relied upon:

1. **Add `mpvScripts.mpris` to the mpv wrapper on both hosts.** Until this
   lands, mpv playback leaves no trace.
2. **Enable MPRIS2 in SMPlayer preferences.** Compiled in, off by default.

### Phase 1 — daemon core (1–2 days) — *done*

Layout, with the Cargo workspace rooted at the repo root:

```
flake.nix                      # devshell; single file by choice
Makefile                       # install/setup/run/test/build/ci/console (ADR-0002)
.github/workflows/ci.yml
Cargo.toml                     # workspace
src/media-logger/
  Cargo.toml
  src/
    main.rs                    # CLI dispatch, daemon wiring
    mpris.rs                   # player discovery + property streams
    audio.rs                   # pw-dump sampling -> sink snapshot
    event.rs                   # schema types + serialisation
    log.rs                     # resume, append, verify (+ chain tests)
    query.rs                   # window selection + state reconstruction
    render.rs                  # output formats behind one interface
  examples/demo.rs             # writes the synthetic fixture
docs/design.md
anchors/                       # pinned trust anchors
examples/                      # generated reports, checked in
README.md                      # Standard Readme
```

The crate is a library with a thin binary over it. That is what lets the
example generator write its fixture through the real `Log` rather than
imitating the format: the hash chain exists in one place, and nothing else
may grow a second copy of it.

This deviates from the house `nix-flake-organization` convention of a thin
`flake/` surface over `src/`: at one devshell and no packages yet, that split
would be structure without content. Revisit when phase 2 adds the package and
the NixOS module.

Each module owns a complete operation — `log.rs` exposes `resume`, `append`
and `verify`, with day rollover internal to `append` rather than a step callers
sequence. `resume` is the startup path and owns the whole of it: refuse on a
missing `<logs-root>`, derive the chain head from the most recent file,
truncate a partial trailing line, and *write* the `chain_restart` line when
there is no prior head. Writing it there rather than returning it for the
caller to write is what makes the guarantee hold — a restarted chain cannot
reach disk undeclared.

Deliverable: daemon writes a valid chained JSONL with heartbeats on `ce2`, and
survives a kill -9 mid-write followed by a restart with the chain intact.

### Phase 2 — packaging and deployment (half day) — *done*

Flake package via `rustPlatform.buildRustPackage`, plus a **NixOS module**
declaring `systemd.user.services.media-logger` with `Restart=always`, and log
root, heartbeat and host as module options.

It must be a *user* service: the daemon reads the session bus, which a
system-level service cannot reach. A NixOS module can still define user
services, so dropping home-manager costs nothing here. Verified under
`systemd-run --user`: the user manager's environment carries
`DBUS_SESSION_BUS_ADDRESS`, so players are found and the sink captured.

The packaged binary is wrapped with PipeWire on `PATH`. Unwrapped it installs
cleanly and then records no output device on any host without `pw-dump` — a
failure shaped like "the sink was never set" rather than a missing dependency,
which is the worst shape for a defect in a record meant to be evidence.

`logsRoot` is **not** created by the module, because the daemon creates it and
declares it as a `CreatedRoot` chain restart. Creating it from the module --
or by hand before first start -- pre-empts that record and leaves a tree that
began from nothing with nothing saying so.
`StartLimitIntervalSec=0` keeps the unit retrying instead of landing in
`failed`, since nobody notices a logger that is not running until they need
the log.

Two deviations from the plan above:

- **`buildRustPackage`, not crane.** Crane buys incremental caching this binary
  does not need, against a flake input and a lock entry. Not worth it given the
  preference for a single flake file.
- **No Ruhezeiten option yet.** Nothing reads it before the reporting layer in
  phase 4, and an option the binary ignores is a promise the system does not
  keep.

Also closes a phase 1 omission: `resume` now takes an exclusive `flock` per
(root, host) and holds it for the writer's lifetime. Two daemons appending to
one tree would interleave lines and break the chain in a way indistinguishable
from tampering — exactly the claim the chain exists to refute — and a service
plus one manual run is all it takes. The lock lives in `$XDG_STATE_HOME`, not
in the tree, which is replicated.

#### Deployment gap — *not done*

"Done" above means the module is written and verified under `systemd-run
--user`. It does **not** mean the daemon runs anywhere. As of 2026-10-22, on
`ce3`:

```
services.media-logger   — never referenced in any host config
systemd unit            — does not exist
logs root               — does not exist
log tree                — none; the only trees are examples/log fixtures
```

`media-logger.url` is declared as a flake input in both `/etc/nixos/flake.nix`
and `dotfiles/nix/os/flake.nix`, and nothing imports `nixosModules` or sets
`services.media-logger.enable` or `logsRoot`. The wiring stopped at the input
line.

So the project is complete through phase 6 and has recorded nothing. Every
"verified" claim in this document is a measurement taken from a dev checkout,
never from a running daemon — which is how a volume defect survived two
rounds of fixing. **The next step is deployment, not more features.** Nothing
in the log is evidence of anything until the daemon has been running for a
while, and the complaint window it is meant to cover is already passing
unrecorded.

### Phase 3 — integrity anchoring (half day) — *done*

Closed day files are timestamped by both authorities, tokens stored beside
them, and `verify` reports anchors alongside the chain. Shells out to
`openssl ts` and `curl` rather than taking an RFC 3161 crate, as planned:
proven, no dependency risk, and a third party verifies with the same command
we produce with.

**No queue exists.** A day file without a token beside it is pending by
definition, so the filesystem is the only state and nothing can disagree with
it — the same reasoning that removed the chain-head cache. The sweep runs at
startup and hourly rather than being triggered by rollover, which keeps the
log module and the anchoring independent and retries whatever the network
refused last time. Offline is the ordinary case, not an error: the day stays
pending and nothing is backdated. `queued` marks a stamp that landed later
than the morning after the day it covers.

Tokens are verified on arrival before being stored. A truncated or error
response that merely resembles a token is worse than none — it would sit in
the tree looking like evidence until someone tried to rely on it.

**Tokens cover the whole day file, not a separate head digest.** Simpler —
no auxiliary artefact to keep beside the log — and strictly stronger, since
the file includes the rollover line whose hash *is* the chain head.

#### The DFN root turned out to be weaker than assumed

The plan treated DFN as the primary anchor and DigiCert as redundancy. That is
backwards on the cryptography. The DFN-Verein Community Root CA 2022 is in no
system trust store and published at no URL we could find; the only copy
obtainable came from a token itself. Pinning it is therefore
**self-referential** — an adversary able to forge a token could forge the root
beside it.

So DigiCert, whose root came from its official distribution point, is the
independently rooted attestation and carries the cryptographic weight. DFN
remains first in presentation because a German reader recognises DFN-CERT
Services GmbH, and it is corroborated by DigiCert agreeing about the same
digest at the same moment. Both roots are pinned in `anchors/` with their
fingerprints recorded, so the DFN root can be checked against DFN-CERT if it
ever has to be relied on.

This is exactly the single-point-of-trust the second TSA was added to remove;
it just turned out to matter more than expected.

### Phase 4 — reporting (1 day) — *done*

`show <when>` with every renderer behind one interface, each emitting the
verification footer. Window forms, state reconstruction and span derivation
work as designed; an instant really is a zero-width window with no special
case.

**Two bugs that only rendering revealed.** Both compiled, ran, and produced
plausible output:

- The log writes RFC 3339 with an offset, which jiff parses as a `Timestamp`;
  its `Zoned` refuses the same string for want of an IANA annotation. Parsing
  as `Zoned` made every record unreadable and every report silently empty.
  Unreadable lines are now counted and surfaced rather than skipped quietly —
  a report that drops records without saying so is worse than one that admits
  it could not read them.
- `Span::get_seconds` returns the seconds *component* of a calendar span, so
  forty minutes read as zero: wrong durations, no quiet-hours overlap at all,
  and misplaced timeline bars.

Both are now regression-tested. The lesson generalises: the dangerous defects
here are the ones that produce a confident, wrong document.

Machine formats emit plain RFC 3339 rather than jiff's `Display`, which
appends `[Europe/Berlin]` — correct, but something spreadsheets and dataframe
libraries refuse to parse, which would defeat the point of `csv` and `xlsx`.

#### Ruhezeiten

Default **22:00–06:00** (Nachtruhe), the window with broad statutory backing.
Mittagsruhe and all-day Sunday rules are *not* default: they come from a
building's Hausordnung and vary, and shading a period as quiet that the house
rules do not impose would overstate the case in a document meant to withstand
being picked at. `--quiet-hours` is repeatable and accepts `22:00-06:00`,
`sun 00:00-24:00`, `mon-fri 13:00-15:00`. The NixOS module exports its own
setting as `MEDIA_LOGGER_QUIET_HOURS` so the shell agrees with the system
without retyping.

#### Gain: distribution, not a peak

A span records effective gain as **min / median / max** with a sample count,
not a single peak. A peak hides the shape: a track played quietly with one
loud moment and a track played loudly throughout share a maximum and are not
the same complaint. The demo bears this out — an evening session reads 0.420
min / 0.550 med / 0.680 max, which says it was *turned up over the evening*,
something no single figure conveys.

**Effective gain is the sink stage multiplied by the player's own.** The
design has said since the start that the two stages multiply and neither
alone reproduces the output; the first implementation nonetheless recorded
only the sink. It is still software gain, and still says nothing about
loudness in a room where a physical knob sets the level.

#### The timeline

Built to the dataviz method. Three categorical slots assigned per player in
fixed order and never cycled, validated against both the light and dark
surfaces. Quiet hours are a **recessive background annotation, not a series** —
they are context, not a measured quantity — and their key is shown even when a
single player needs no legend, since otherwise the shading cannot be decoded.
Gridlines sit only on the labelled hours; an hourly ladder competed with the
marks. The table beneath carries every row, which is also the relief for the
one slot below 3:1 contrast on the light surface.

**Gain is drawn inside the playback bars**, as a solid fill in the player's
own colour against a soft tint of it. The soft block is the span's extent;
the fill height within it is gain.

This was first built as a separate lane beneath each day, on the reasoning
that gain is a second measure and folding it in would be the dual-axis
mistake in another costume. That reasoning was wrong: the dual-axis fault is
*two scales on one axis with no way to tell which a mark belongs to*, and
here the bar's height was simply an unused channel with one gain scale shared
by every bar. The separate lane also had a real cost — it made the reader
eye-match x-positions across two tracks to connect a level with the track
that produced it.

Tints are computed in Rust and emitted as literal hex rather than written as
`color-mix()`, for the same reason the lane's `var()` fill had to go: the PDF
renderer resolves literal hex and little else. That lane printed solid black
while looking correct in a browser — neither `var()` nor `currentColor`
survives inside an SVG `fill`.

In the HTML the bar is a hover target reading out the gain at the cursor's
position along it. The marks themselves are a couple of pixels wide at day
scale, far too small to hover, so the bar is the target and position within
it selects the value. It is inert in the PDF, which is why the same figures
are in the table.

Rendering it and looking at it is what caught the legend gap, the ladder, and
a sink column showing `alsa_output.usb-Focusrite_...` where a person needs
"Scarlett 4i4 USB Pro". The stable node name stays in the data formats.

### Phase 5 — xlsx — *done*

`--format xlsx` writes two sheets: Playback, and Integrity carrying the chain
and anchor status. Separate sheets because an integrity note mixed into the
data rows would be sorted away the first time anyone sorted the table.

**`csv` and `xlsx` are one table in two encodings**, defined once as a column
list and a row builder that each renderer consumes. They were written twice
and drifted: the spreadsheet kept a `Peak volume` header after the data
beneath it had become min/median/max, leaving thirteen columns under ten
headings — mislabelled numbers in a document meant to be evidence.

Each format still decides how to *write* a value, which is the point of
holding values rather than strings: durations are raw seconds in csv, where a
script can use them, and `2h20m` in xlsx, where a person reads them; gains
stay numeric in the spreadsheet so they can be charted without anyone
retyping a column.

The document formats — text, html, pdf — are deliberately *not* this table.
They group and stack columns to fit a printed page, which is a presentation
decision rather than a different dataset.

Still not built, because nothing yet needs them: richer search, and additional
event types.

### Phase 6 — correctness, then navigation — *done*

Ordered by severity. The first two are defects: the tool currently records a
constant where it should record a measurement, and omits a machine without
saying so. Nothing below them matters until they are fixed.

#### 6a. Read the volume from where it actually lives — *done*

`audio.rs` reads `channelVolumes` from the **node**. On the `ce2` HDMI sink
that field sat at `1.0` whatever the volume was, while the real volume sat on
the device **Route**. Verified on 2026-10-22:

```
wpctl set-volume <hdmi> 0.5
  node.channelVolumes  = [1.0, 1.0]      <- what we read
  route.channelVolumes = [0.125, 0.125]  <- where the volume is
```

So on an ordinary machine — laptop speakers, HDMI, a USB DAC with a mixer —
**every volume this tool has recorded is the constant 1.0**, regardless of
what was done with the volume. Volume is half the evidence; this invalidates
it.

It stayed invisible because the only machine available to test on is the one
where the wrong answer is accidentally right: the Scarlett in pro-audio mode
genuinely has no software volume and no route, so `1.0` is the truth there.
A defect that is correct on the development machine and wrong everywhere else
is exactly the kind that survives to production.

The fix:

- record the node volumes **and** the route volumes when a route exists
- report `effective = route` where a route exists, else the node
- keep both in the record, so a reader can see where the attenuation sat

Logs written before this fix must be treated as having no usable volume on
mixer-equipped devices. They are not wrong about *what played* — only about
how loud. That loss is permanent: the route was never *recorded*, so there is
nothing to re-derive from.

##### Correction, 2026-10-22: node and route are one control, not two stages

The first attempt at this fix reported `effective = node × route`, reasoning
that the design's own "gain stages multiply" principle applied inside the sink
too. **It does not, and that rule was wrong.** Corrected the same day, against
the first machine that could falsify it.

The primary machine's Teufel ULTIMA 25 over A2DP, with the speaker's own
display as an independent readout:

```
node  channelVolumes = [0.00858, 0.00858]    softVolumes = [1.0, 1.0]
route channelVolumes = [0.00858, 0.00858]
BlueZ MediaTransport1.Volume = 26  (of 127 = 20.5%)
speaker's front panel        = 20%
```

`cbrt(0.00858) = 0.2047`, and `0.2047 × 127 = 26`. The three readings agree.
Node and route are **one control seen from two objects**, and they move
together: `wpctl set-volume 0.18` drove both to `0.005832` and AVRCP to 23.

So `node × route` gives `7.36e-5` — **-82.7 dB logged for a -41.3 dB
setting**, or 4% where the speaker says 20%. The error equals the gain itself
in dB, so it vanishes at unity and grows as the volume drops.

It is not Bluetooth-specific. The same machine's built-in analog sink mirrors
too — node = route = `0.216007`, `cbrt` = 0.60, matching `wpctl` — which the
product rule would have logged as 36%. The `ce2` HDMI reading above is the
*exception*, not the rule, and "pinned at `1.0` permanently on any ALSA sink
with a hardware mixer" was an over-generalisation from a single sink.

Where the two disagree the route is authoritative: a node reading `1.0` beside
a route carrying the volume means *no node-stage attenuation*, not unity times
something. Preferring the route is correct in all three sinks measured; the
product rule was correct only in the one it was derived from.

Two things let a wrong rule ship inside a fix for a wrong rule:

- the test asserted `node 1.0 × route 0.125`, the one combination where the
  product rule and the right answer coincide, and the demo fixtures were built
  in that same shape — so nothing in the suite ever presented a node and route
  both carrying a real value;
- the hedge "whichever field BlueZ populates, it is captured rather than
  guessed" treated an untested case as covered. The failure mode was never
  *which* field gets populated. Both do, with the same number.

The tests now assert the measured Bluetooth and analog values directly, and
the fixtures carry the mirrored shape alongside the `ce2` one.

Unlike the defect it corrects, this one cost no data. `effective()` is called
only from `query.rs` and `render.rs`; it is never serialised. Every record
carries raw `channel_volumes` and `route_volumes`, so a log written under the
product rule is **fully recoverable by re-running `show` with a fixed binary**
— no rewrite, no re-anchoring, no disturbance to the chain. The defect was in
the reporting layer, not the recording layer.

Moot in practice as of 2026-10-22: there is no log tree on either host yet
(see "Deployment gap" below), so nothing has been mis-reported at all.

Re-verified live against the corrected code, same day, speaker reconnected:
`audio.rs` sampled `node = route = [0.008, 0.008]` and reported an effective
`0.008` — 20.0% on the cubic scale, against AVRCP 25/127 and a front panel
reading 20%. The old rule would have reported 4.0%.

`softVolumes = [1.0, 1.0]` also confirms the premise of "a note on the two
machines" below: PipeWire applies no gain of its own here, it forwards the
setting over AVRCP and the speaker attenuates. The logged volume is a remote
control value, which is exactly why it is meaningful on this host.

#### 6b. Report every host, not just this one — *done*

`show` reports `<root>/<hostname>` and ignores any other host tree **without
saying so**:

```
hosts present: ce2 laptop
$ media-logger --root tree show 2026-10-19
media-logger — 2026-10-19 (host ce2)
```

A document headed with a date that silently omits a machine's playback is the
failure this project least affords — it would be handed over believing it
complete. The tree is host-partitioned precisely so machines can share one
replicated directory, and reporting never unified them.

All hosts become the default. Chains stay **per-host** — each host writes its
own and they must never be merged — so the integrity footer reports each
separately, and a break in one is attributed to that host.

Two hosts playing at once is a real state and must read as two concurrent
spans, not one.

#### 6c. Filters on the command line — *done*

`--host`, `--player`, `--device`, repeatable, and `--during quiet|other`,
narrowing what a *document* contains. The last is usually the one that
matters: "everything that touched Ruhezeiten" is the report a complaint
actually asks for. A session partly inside counts as inside — it touched the
protected hours, which is the question being asked. This is the half that matters: it is what makes "just the Sonos,
just that week" producible as a PDF someone else reads. Interactive filters
cannot do that.

#### 6d. A calendar to navigate by, with nothing hidden — *done*

A long window gains a calendar above the timeline. The calendar is a way to
*narrow*, not a replacement: clicking a day filters the view to it.

**The timeline is never suppressed.** An earlier draft hid it beyond some
number of days on the grounds that ninety rows is unreadable. That is the
wrong instinct for this document: a report that quietly stops showing data
once there is a lot of it is the same failure as one that omits a host. If a
long window is unwieldy, the reader narrows it deliberately — the tool does
not decide on their behalf what they are allowed to see.

#### 6e. Calendar view for long windows — *done*

One cell per day, coloured by how loud that day was.

**Summarised as a sustained maximum, not a peak.** The loudest level held
continuously for at least five minutes: take every five-minute window, find
the quietest moment within it, and report the loudest of those. A thirty
second spike while the volume is being turned back down cannot move it, by
construction. The peak remains available on hover.

This is the same reasoning that replaced peak with min/median/max for spans,
but the conclusion differs by context: the timeline can afford the whole
curve, a calendar cell has room for one number.

**A single-hue sequential ramp, light to dark.** Not green→yellow→red: that
is a rainbow for what is a magnitude, and red/green is the worst possible
pair for colour vision deficiency — around 8% of men, which may well include
the landlord or lawyer the document is meant to persuade. A single hue also
survives greyscale printing, which a traffic light does not.

**A gentle marker for quiet hours, not an alarm.** Sound during Ruhezeit is
not inherently disruptive if it was quiet, and loud sound at 20:00 can be —
which is why magnitude drives the colour. The marker answers *was any of this
in protected hours*, and deliberately does not answer *was it a problem*.

**The bounds come from the days on screen, not from a constant.** On `ce2`
the Scarlett is pinned at 1.0, so any fixed scale puts every day off the top;
on the Bluetooth machine ordinary listening sits far lower. One constant
cannot fit both machines, and the first fixed default — 0.05 to 0.35 —
duly drew four months of evenings as one uniform blue, which is a calendar
that says nothing. So `--gain-scale` defaults to `auto`: the quietest day
shown anchors one end, the loudest the other, and the ramp spends its whole
range on the differences that exist. It needs no knowledge of the device,
which is the point, since that knowledge does not exist yet.

The automatic bounds follow the filters, because the calendar follows the
filters: narrowing to the quiet laptop should spread the ramp over *its*
days rather than leave them all at one end, and the legend reprints the
numbers so the reader can see they moved. Two guards keep the honesty: a
span narrower than 0.1 is widened around its middle instead of stretched, so
a month of near-identical days reads as near-identical rather than as a
dramatic sweep, and `--gain-scale LOW:HIGH` pins the bounds for when two
reports have to be read against each other — a relative scale is the right
default for seeing, a fixed one for comparing.

**The scale is raw effective gain, not estimated dB.** Converting logged gain
to decibels through a two-point fit was considered and dropped. It would be
implied precision of exactly the kind this design keeps refusing: a fit from
one room and one position, presented in the units of a real measurement. The
log exists so that conditions can be *recreated and measured* when a specific
day is challenged — that is what makes the evidence worth anything, and a
guessed number standing in for the measurement would weaken it, not help.
This is the same reasoning that rejected continuous loudness sampling in the
first place.

#### 6f. Filters in the HTML report — *done*

Multi-select for host, device and player, and a calendar whose cells narrow
the view to a day.

**Clicking a name means "only this", and again means "everything".** That is
the question being asked nearly every time — *what did this one machine do* —
and making it the default click costs nothing, because the checkbox and
shift-click are still there for building a set by hand. Calendar days work
the same way: a plain click selects that day alone, shift extends. Cells are
focusable, so they answer Enter and Space too. Inert in the PDF, which is correct: these are for
*finding* the window, while the command line produces the document.

**The calendar follows the dropdowns, but not the day selection.** A cell
showing the unfiltered day beside a table showing a filtered one would be
two answers to the same question, and the calendar is the half a reader
trusts at a glance — so narrowing by host, player or device recolours it,
and a day whose only playback was filtered out goes uncoloured rather than
keeping a shade nothing on screen supports.

Picking a day is different in kind. It is navigation, not a statement about
what counts, and recolouring by it would blank every other cell and destroy
the overview being navigated with. The selection shows as an outline on the
chosen cell; the colours stay put.

#### What building it changed

- **The calendar is a table, not CSS grid.** Grid laid out correctly in a
  browser and silently spilled cells across columns in the PDF. Found the same
  way as the SVG `fill` and the `aspect-ratio` before it: by rendering the
  page and looking at it. Anything the printed report depends on has to be
  checked in the renderer that prints it, not the one that previews it.
- **Filters apply after reconstruction, never during.** State has to be
  replayed in full or the reconstruction is wrong; a filter is about what the
  reader is shown, not about what happened. Levels are narrowed alongside
  their spans, so a gain lane cannot outlive the bar it describes.
- **The gain scale is a flag, not a constant.** Until a real device is
  measured the default is a guess, and it is configurable because of that
  rather than in spite of it.
- **Sustained maximum is tested directly** against a spike, a held level, and
  a stretch too short to sustain — the three cases that decide whether the
  number means anything.

#### Follow-ups for an agent on the Bluetooth machine

These cannot be settled from `ce2`. They need the host with the Teufel
speaker, running the daemon, and they are written to be executed rather than
reasoned about.

**1. Where a BlueZ sink keeps its volume.** 6a reads node and route and
multiplies them, which is correct whichever BlueZ populates — but the
calendar thresholds cannot be chosen until the real numbers are known.

```sh
ID=$(wpctl status | grep -i bluez | head -1)      # find the sink id
wpctl set-volume <id> 0.5
pw-dump | jq '.[] | select(.id==<id>) | .info.params.Props[0].channelVolumes'
pw-dump | jq '.[] | select(.type|endswith("Device")) | .info.params.Route[]?
               | select(.direction=="Output") | {name, cv: .props.channelVolumes}'
```

Record which field moved, and whether `0.5` stored `0.125` (cubic, as on
`ce2`) or `0.5` (linear). **Report both, do not assume the `ce2` answer
carries over** — the AVRCP path is a different mapping from the ALSA one.

**2. What the Teufel's own display corresponds to.** Set a few known volumes,
read the number on the speaker's screen, and record the pairs alongside the
logged effective gain:

| wpctl | speaker display | logged effective gain |
| --- | --- | --- |

This answers whether the display follows the same cubic convention. It is
worth knowing; it is *not* for converting gain to decibels, which was
considered and rejected.

**3. What ordinary and loud actually read as.** Run the daemon, play at a
level you would call normal, then at one you would call loud, five minutes
each. Read the logged effective gain for both:

```sh
media-logger --root ~/logs show today --format json | jq '.spans[].gain_linear'
```

Those two numbers set the calendar's default thresholds. Until then the
defaults are a guess, and should be treated as one.

**4. Confirm player coverage on that host.** The table in this document was
measured on `ce2`. Check the same there, and in particular that
`mpvScripts.mpris` is in the mpv wrapper — without it mpv playback leaves no
trace at all, silently.

```sh
busctl --user list | grep mpris
```

**5. Confirm the daemon under systemd.** It must be a *user* service to reach
the session bus. Verified on `ce2`; worth re-confirming there, since a
service that starts and records nothing is the failure nobody notices.

```sh
systemctl --user status media-logger
media-logger --root ~/logs show today | head -20
```

**6. Sanity-check 6a against reality.** Change the volume mid-playback and
confirm the log follows it. Before 6a this would have recorded a flat `1.0`,
and that is precisely the defect to prove gone:

```sh
media-logger --root ~/logs show today --format json | jq '.spans[].gain_linear'
```

#### 6g. What the report can speak for — *done*

A report that cannot tell *nothing played* from *nothing was recorded* can be
read, in perfect good faith, as evidence of a quiet month that nobody
observed. The daemon was never running on `ce2` — enabled, but never started
— and the reports said only that no playback had been found.

**Coverage is derived from the beats, not declared.** Every record is proof
the daemon was alive at that moment, and a collapsed heartbeat is proof for
every beat it stands for. Merging those moments gives the stretches the
daemon was up; a gap longer than three of its own beats ends a stretch. The
beat interval is not assumed either — a collapsed run states it, so a host
beating every twenty seconds and one beating every minute are both judged by
their own clock.

**A day is only as recorded as its least covered host.** Playback on the
machine that was down leaves no trace, so a day where one of two hosts was
off is a day the report cannot fully speak for. Five minutes of slack keeps
a restart from marking a day incomplete — and five minutes is already the
window a level has to hold to count as sustained.

**It is stated above the data, not in the footer.** It changes how
everything below has to be read. The calendar hatches unrecorded days and
dashes the partial ones, achromatically: the fill means loudness, and this
is not a quantity but a statement about whether there is one. For an instant
— "what was playing at 23:40" — the note is about that moment, because the
answer "nothing" is worth nothing unless the logger was running at 23:40.

### Testing

Per `when-to-write-tests`, the default is no test and verification is running
the thing. Tests are warranted for the contracts below — all public, all of
which fail silently if wrong, which is the disqualifying property for a log
whose value is that it can be trusted without being watched:

- **Chain verification** — append a known sequence, verify; mutate one line,
  confirm verification fails at that line; confirm a day-boundary deletion is
  detected.
- **`resume` on a damaged tail** — truncated final line is dropped and the
  chain continues from the last intact line; a missing tree yields
  `chain_restart`, not a silent genesis; a missing `<logs-root>` refuses to
  start.
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

1. Ruhezeiten for this building — needs the actual *Hausordnung*, particularly
   whether Mittagsruhe and all-day Sunday rules apply.
2. Retention. Indefinite is the default; there is no reason to discard, but a
   policy should be stated rather than drifted into. At ~1.2 GB/year,
   compressing closed months is the first lever.
3. Whether to log the Scarlett monitor knob position manually on `ce2` when it
   changes, given software cannot see it.
4. Whether `ce2` has a log tree. It is almost certainly in the same state as
   `ce3` — the flake input is declared and the module never enabled — but that
   was checked from `ce3` only. If a tree does exist there, its volumes are
   recoverable by re-reporting, not lost.
5. Why the `ce2` HDMI sink reports a node of `1.0` beside a route carrying the
   volume, when both sinks on the primary machine mirror. Preferring the route
   is correct either way, so nothing is blocked on this — but it is the only
   sink measured that does not mirror, and the reason is not understood. Worth
   re-checking on `ce2` under this lens, since the last reading from that
   machine is what produced a wrong rule.
