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
| `calendar.pdf`, `calendar.html` | A month-long window, where the calendar appears above the timeline |
| `one-day.pdf` | A single day |
| `instant.txt` | `show 2026-10-19T22:30` — what was playing at one moment |
| `log/` | The chained JSONL the reports are built from |

The calendar example uses `--gain-scale 0.3:0.7`, chosen so the fixture's
levels spread across the ramp. The default scale is a guess until a real
device is measured; see the phase 6 follow-ups in
[docs/design.md](../docs/design.md).

## The fixture

Three days on one host, chosen to exercise what the reports have to show:

- **18th** — a quiet evening on the Scarlett, ending well before Nachtruhe.
- **19th** — an afternoon film, then an evening session whose gain climbs from
  0.42 to 0.68 and runs to 23:40. That is the case the reports exist to make
  legible: 1h40m inside quiet hours, at a level that rose over the evening.
- **20th** — a short listen on a Bluetooth speaker, low and steady.

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
