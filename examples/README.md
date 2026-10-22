# Example reports

**Synthetic. Nothing here happened.** `log/` is fabricated playback written to
demonstrate the report formats, and the reports beside it are generated from
it. None of it is a record of anything, and it must never be mixed into a real
log tree — the generator refuses to write into one that already exists.

Regenerate with:

```sh
make examples
```

## What is here

| File | What it shows |
| --- | --- |
| `report.pdf` | The three-day report as it prints — the one you would hand to someone |
| `report.html` | The same, with hover readouts of gain at a point |
| `report.txt` | Terminal output |
| `report.csv`, `report.xlsx` | The same table in two encodings, from one definition |
| `calendar.pdf`, `calendar.html` | Three and a half months: four month grids above the timeline. 16 pages, because the timeline is never suppressed |
| `one-day.pdf` | A single day |
| `instant.txt` | `show 2026-10-19T22:30` — what was playing at one moment |
| `log/` | The chained JSONL the reports are built from |

The calendar example uses `--gain-scale 0.3:0.7`, chosen so the fixture's
levels spread across the ramp. The default scale is a guess until a real
device is measured; see the phase 6 follow-ups in
[docs/design.md](../docs/design.md).

## The fixture

August to mid-November 2026 on **two hosts**, `ce2` and `ce3`, sharing one
tree — which is what the host-partitioned layout exists for, and what a
report has to cover without being asked. Each writes its own chain and they
are verified separately. The pattern is a pure function of host and date,
never a random draw, so regenerating changes nothing:

**`ce2`** — the desk machine, evenings, mostly through the Scarlett:

- Roughly four evenings in five have something on, drawn from a library of
  ten records, four films and three streams — so a season does not read as
  one evening repeated.
- Start times, durations and levels all drift with the date rather than
  repeating: evenings run from about 50 minutes to two and a half hours.
- Every eleventh day or so is a loud evening running past 23:00, climbing
  from 0.42 to 0.68 — the case the reports exist to make legible.
- Nine nights are a film started before midnight and finished after it —
  one session, two day files. One of them is 24–25 October, which is also
  the night the clocks go back.
- Some days are an afternoon film followed by something quiet after dinner,
  some are two or three records back to back, some a short stream on the
  Bluetooth speaker. About one in five is silent.

**`ce3`** — a laptop: daytime, browser-heavy, shorter and quieter, usually on
headphones. Some days are four scattered stretches, some a single thing at
lunch, some a film on its own speakers in the evening. It overlaps `ce2` on
sixteen occasions, which is two sources of sound at once and reads as two
concurrent spans.

Both go quiet together for a week in September and a few long weekends —
nobody was home — which is what makes a day empty in a report covering two
hosts, and the case the timeline's "days with no playback are not shown"
line exists for.

Together: 229 spans over 107 days, 1 to 5 sessions on an active host-day,
18 days silent on both machines, three players and three output devices.

Every day on a host is different from every other. The generator asserts
it: an earlier version drew from small moduli and produced twenty-nine
identical pairs, so four months read as a handful of evenings on a loop.

The three-day reports cover 18–20 October, which has one of each.

It includes **25 October**, the day the clocks go back. That day is 25 hours
long, and the timeline places marks by elapsed time, so its quiet band sits
at 28% and 92% rather than 25% and 91.67%. A row that assumed 24 hours drew
everything after the transition in the wrong place.

It is written through the real `Log`, so it is a genuine chained log in the
current schema rather than an imitation that drifts the first time either
moves. The hash chain lives in exactly one place and the fixture does not add
a second:

```sh
media-logger --root examples/log verify
```

The days carry no timestamp tokens. Anchoring reaches a real authority, and
fabricating a token — or stamping made-up playback as though it were
evidence — is not something this repository should contain.

## Why the diff moves

Every report carries a "generated" line, so regenerating changes that even
when nothing else has. `log/` itself is deterministic and will not change
unless the schema does.
