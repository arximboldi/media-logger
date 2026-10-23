//! Writes the synthetic tree the checked-in example reports are built from.
//!
//! It goes through the real `Log`, so the fixture is a genuine log — correctly
//! chained, in the current schema — rather than an imitation that drifts the
//! first time either changes. The hash chain exists in exactly one place and
//! this does not add a second.
//!
//!     cargo run --example demo -- examples/log
//!
//! Three days on one host, chosen to exercise what the reports have to show:
//! a quiet evening, an afternoon film and an evening session that climbs past
//! the start of Nachtruhe, and a short Bluetooth listen. Deterministic, so
//! regenerating changes nothing but the reports' own "generated" line.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use jiff::civil::date;
use jiff::tz::TimeZone;
use jiff::Span as JiffSpan;
use jiff::Zoned;

use media_logger::event::{Event, PlayerState, Playing, Sink, Stream, Track};
use media_logger::log::Log;
use media_logger::query::fingerprint;

/// Two machines sharing one replicated tree, which is what the layout
/// exists for — and what a report has to cover without being asked.
const HOSTS: &[&str] = &["ce2", "ce3"];
/// Fixed so the fixture is reproducible; Berlin because Ruhezeiten are local.
const TZ: &str = "Europe/Berlin";

fn main() -> Result<()> {
    let root: PathBuf = std::env::args()
        .nth(1)
        .context("usage: demo <logs-root>")?
        .into();

    // Refuse to add to an existing tree: this writes fabricated playback, and
    // it must never be possible to mix that into a real log.
    for host in HOSTS {
        if root.join(host).exists() {
            bail!(
                "{} already exists — remove it first; this writes synthetic data \
                 and must never be mixed into a real log",
                root.join(host).display()
            );
        }
    }

    let tz = TimeZone::get(TZ)?;

    // Four months, so the calendar has months to lay out and a season's
    // shape to show. Deterministic: the pattern is a function of the host
    // and the date, never a random draw, so regenerating changes nothing.
    // No two days on a host may look the same. The first version drew from
    // small moduli and produced twenty-nine identical pairs, which made four
    // months read as a handful of evenings on a loop.
    let mut seen: std::collections::HashMap<String, (i8, i8)> = std::collections::HashMap::new();
    let mut empty = 0;

    let mut off = 0;

    for host in HOSTS {
        let (mut log, _) = Log::resume(&root, host, &at(&tz, 2026, 8, 1, 0, 0))?;
        // The daemon runs continuously, as it does on a machine that is on:
        // it stops only where the machine itself is away, and those days are
        // the ones a report must mark as unrecorded rather than silent.
        let mut running = false;
        let mut carry: Vec<Session> = Vec::new();
        let mut rest = idle_sink(host);

        for (year, month, last) in [(2026, 8, 31), (2026, 9, 30), (2026, 10, 31), (2026, 11, 15)] {
            for day_of in 1..=last {
                let midnight = at(&tz, year, month, day_of, 0, 0);
                if away(month, day_of) {
                    if running {
                        // The machine is shut down the night before, so the
                        // stop belongs to the last day there is a record of
                        // -- not to the first day there is none.
                        log.append(
                            &Event::DaemonStop {
                                reason: "sigterm".to_owned(),
                            },
                            &midnight.checked_sub(JiffSpan::new().minutes(1))?,
                        )?;
                        running = false;
                    }
                    // Nothing runs into an outage: the machine is off.
                    carry.clear();
                    off += 1;
                    continue;
                }
                if !running {
                    log.append(
                        &Event::DaemonStart {
                            version: "0.1.0".to_owned(),
                        },
                        &midnight,
                    )?;
                    running = true;
                }

                let sessions = plan(host, month, day_of);
                if sessions.is_empty() {
                    empty += 1;
                } else if let Some(before) =
                    seen.insert(signature(host, &sessions), (month, day_of))
                {
                    bail!(
                        "{host} {month:02}-{day_of:02} is identical to {:02}-{:02}; \
                         the fixture must not repeat a day",
                        before.0,
                        before.1
                    );
                }

                carry.extend(sessions);
                let until = ((month, day_of) == (LEAVES.0, LEAVES.1)).then_some(LEAVES.2);
                carry = day(&mut log, &midnight, carry, &mut rest, until)?;
                if let Some(minute) = until {
                    log.append(
                        &Event::DaemonStop {
                            reason: "sigterm".to_owned(),
                        },
                        &at(
                            &tz,
                            year,
                            month,
                            day_of,
                            (minute / 60) as i8,
                            (minute % 60) as i8,
                        ),
                    )?;
                    running = false;
                    carry.clear();
                    off += 1;
                }
            }
        }
        if running {
            log.append(
                &Event::DaemonStop {
                    reason: "sigterm".to_owned(),
                },
                &at(&tz, 2026, 11, 16, 0, 0),
            )?;
        }
    }
    println!("{empty} silent host-days, {off} unrecorded host-days");

    println!("wrote {}", root.display());
    Ok(())
}

/// A record as it plays: its tracks in order, with their lengths in minutes.
///
/// One span per track is what a run of songs is made of, and the timeline
/// has to draw a side of four-minute songs differently from a single
/// hour-long piece. The library holds both, so the examples exercise both.
struct Album {
    title: &'static str,
    artist: &'static str,
    dir: &'static str,
    tracks: &'static [(&'static str, i32)],
}

/// A small library, so a season does not read as one evening repeated.
const ALBUMS: &[Album] = &[
    // Short sections one after another: at day scale these fall below the
    // width at which a mark can be told from the hairline beside it, and
    // the hour has to read as one block.
    Album {
        title: "Music for 18 Musicians",
        artist: "Steve Reich",
        dir: "sync/reich",
        tracks: &[
            ("Pulses", 6),
            ("Section I", 5),
            ("Section II", 5),
            ("Section IIIA", 4),
            ("Section IIIB", 4),
            ("Section IV", 7),
            ("Section V", 6),
            ("Section VI", 5),
            ("Section VII", 5),
            ("Section VIII", 5),
            ("Section IX", 6),
            ("Section X", 2),
            ("Section XI", 6),
        ],
    },
    Album {
        title: "Selected Ambient Works Volume II",
        artist: "Aphex Twin",
        dir: "sync/afx",
        tracks: &[
            ("Cliffs", 5),
            ("Radiator", 5),
            ("Rhubarb", 7),
            ("Hankie", 4),
            ("Grass", 5),
            ("Mould", 4),
            ("Curtains", 4),
            ("Lichen", 4),
            ("Spots", 3),
            ("Tassels", 5),
        ],
    },
    // Two sides, each long enough to carry its own hairline.
    Album {
        title: "A Rainbow in Curved Air",
        artist: "Terry Riley",
        dir: "sync/riley",
        tracks: &[
            ("A Rainbow in Curved Air", 19),
            ("Poppy Nogood and the Phantom Band", 22),
        ],
    },
    // One piece, start to finish: a single mark, which is what the drawing
    // has to leave alone.
    Album {
        title: "The Disintegration Loops I",
        artist: "William Basinski",
        dir: "sync/basinski",
        tracks: &[("dlp 1.1", 63)],
    },
    Album {
        title: "II (Continuous Mix)",
        artist: "Voices from the Lake",
        dir: "sync/vftl",
        tracks: &[("II (Continuous Mix)", 65)],
    },
    // Short pieces and one long one on the same record.
    Album {
        title: "Harmony of Difference",
        artist: "Kamasi Washington",
        dir: "sync/kamasi",
        tracks: &[
            ("Desire", 4),
            ("Humility", 4),
            ("Knowledge", 4),
            ("Perspective", 4),
            ("Integrity", 5),
            ("Truth", 13),
        ],
    },
    Album {
        title: "Discreet Music",
        artist: "Brian Eno",
        dir: "sync/eno",
        tracks: &[
            ("Discreet Music", 31),
            ("Fullness of Wind", 9),
            ("French Catalogues", 5),
            ("Brutal Ardour", 7),
        ],
    },
    Album {
        title: "Spiegel im Spiegel",
        artist: "Arvo Pärt",
        dir: "sync/part",
        tracks: &[
            ("Spiegel im Spiegel", 9),
            ("Für Alina", 2),
            ("Fratres", 11),
            ("Cantus in Memoriam Benjamin Britten", 6),
        ],
    },
    Album {
        title: "Symphony No. 3, Op. 36",
        artist: "Henryk Górecki",
        dir: "sync/gorecki",
        tracks: &[
            ("I. Lento — Sostenuto tranquillo ma cantabile", 27),
            ("II. Lento e Largo — Tranquillissimo", 10),
            ("III. Lento — Cantabile semplice", 18),
        ],
    },
];

const FILMS: &[(&str, &str, &str)] = &[
    (
        "Stalker (1979)",
        "Andrei Tarkovsky",
        "/media/films/stalker.mkv",
    ),
    (
        "Sans Soleil (1983)",
        "Chris Marker",
        "/media/films/sans-soleil.mkv",
    ),
    ("Ran (1985)", "Akira Kurosawa", "/media/films/ran.mkv"),
    (
        "Jeanne Dielman (1975)",
        "Chantal Akerman",
        "/media/films/jeanne-dielman.mkv",
    ),
];

const STREAMS: &[(&str, &str, &str)] = &[
    (
        "Nils Frahm — Says (Live)",
        "Nils Frahm",
        "https://youtube.com/watch?v=dIww",
    ),
    (
        "Hania Rani — Live from Studio",
        "Hania Rani",
        "https://youtube.com/watch?v=Hr4n",
    ),
    (
        "Floating Points — Essential Mix",
        "Floating Points",
        "https://youtube.com/watch?v=Fp93",
    ),
];

/// What a day looks like, for the no-two-days-alike check.
fn signature(host: &str, sessions: &[Session]) -> String {
    let mut out = host.to_owned();
    for s in sessions {
        out.push_str(&format!(
            "|{:?}{:?}{}{}{}{:.4}{:.4}",
            s.from,
            s.to,
            s.player.player,
            s.player.track.title.as_deref().unwrap_or(""),
            // Which track the needle landed on varies too, and two evenings
            // of the same record from different points are not the same day.
            s.cuts.len(),
            (s.gain)(0.0),
            (s.gain)(1.0),
        ));
    }
    out
}

/// Deterministic noise, seeded by host and date.
///
/// The first version drew everything from small moduli — `n % 3`, `n % 7` —
/// and twenty-nine pairs of days came out identical because those cycles
/// realign long before four months are up. A well-distributed stream makes
/// a repeat vanishingly unlikely, and `main` asserts there are none.
struct Rand(u64);

impl Rand {
    fn seed(host: &str, month: i8, day_of: i8) -> Self {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for b in host.bytes().chain([month as u8, day_of as u8, 0x5a]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        Rand(h)
    }

    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// Inclusive of `lo`, exclusive of `hi`.
    fn between(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo).max(1) as u64) as i32
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// The afternoon both machines are shut down for a trip.
///
/// A day with half a record is its own case: it is neither quiet nor
/// unrecorded, and a report has to be able to say so.
const LEAVES: (i8, i8, i32) = (10, 9, 15 * 60);

/// Days nobody was home and both machines are off, so there is no record of
/// them at all — which is not the same as a day when nothing played, and is
/// the distinction the calendar's hatching exists to draw.
fn away(month: i8, day_of: i8) -> bool {
    matches!(
        (month, day_of),
        // A week away in September, a long weekend in each of the others.
        (9, 11..=18) | (8, 14..=16) | (10, 30..=31) | (11, 1) | (10, 10..=11)
    )
}

/// What was played on a given day, as a pure function of host and date.
///
/// Varied on every axis a report can show — what, how many, for how long,
/// starting when, at what level, through which device, on which machine.
/// Nothing is random: host and date seed the stream, so regenerating
/// produces exactly the same tree.
fn plan(host: &str, month: i8, day_of: i8) -> Vec<Session> {
    if away(month, day_of) {
        return Vec::new();
    }
    let mut r = Rand::seed(host, month, day_of);
    if host == "ce3" {
        return ce3(&mut r);
    }

    // A quiet day of its own, on top of the shared ones.
    if r.chance(14) {
        return Vec::new();
    }

    let latest = 23 * 60 + 50;
    let album = |r: &mut Rand| &ALBUMS[r.below(ALBUMS.len())];

    match r.below(11) {
        // A film started before midnight and finishing after it. One
        // session, two day files, and the only case where a day opens with
        // playback already under way.
        10 => {
            let f = FILMS[r.below(FILMS.len())];
            let from = 22 * 60 + 40 + r.between(0, 8) * 5;
            let level = 0.40 + r.between(0, 8) as f64 * 0.01;
            vec![session(
                from,
                from + r.between(150, 215),
                mpv(f.0, f.1, f.2),
                scarlett,
                move |x| level + 0.04 * (x * 3.0).sin(),
            )]
        }
        // A loud evening running well into Nachtruhe.
        0 | 1 => {
            let a = album(&mut r);
            let from = 20 * 60 + 40 + r.between(0, 14) * 5;
            let to = (from + r.between(110, 190)).min(latest);
            let top = 0.60 + r.between(0, 12) as f64 * 0.01;
            vec![record(&mut r, from, to, a, scarlett, move |x| {
                0.42 + (top - 0.42) * x
            })]
        }
        // An afternoon film, then something after dinner.
        2 => {
            let f = FILMS[r.below(FILMS.len())];
            let a = album(&mut r);
            let from = 13 * 60 + r.between(0, 20) * 10;
            let after = 19 * 60 + r.between(0, 12) * 10;
            let level = 0.40 + r.between(0, 10) as f64 * 0.01;
            vec![
                session(
                    from,
                    from + r.between(95, 165),
                    mpv(f.0, f.1, f.2),
                    scarlett,
                    |x| 0.33 + 0.02 * (x * 4.0).sin(),
                ),
                {
                    let to = (after + r.between(55, 120)).min(latest);
                    record(&mut r, after, to, a, scarlett, move |x| {
                        level + 0.03 * (x * 6.0).sin()
                    })
                },
            ]
        }
        // A DJ set in Mixxx, which puts audio through PipeWire and says
        // nothing on the bus at all. The case the stream record exists
        // for: without it this evening reads as silence.
        9 => {
            let from = 20 * 60 + r.between(0, 16) * 10;
            let level = 0.52 + r.between(0, 14) as f64 * 0.01;
            vec![direct(
                from,
                (from + r.between(90, 220)).min(latest),
                "Mixxx",
                "mixxx",
                scarlett,
                move |x| level + 0.05 * (x * 2.0).sin(),
            )]
        }
        // Something on the Bluetooth speaker.
        3 => {
            let s = STREAMS[r.below(STREAMS.len())];
            let from = 17 * 60 + r.between(0, 40) * 10;
            let base = 0.32 + r.between(0, 10) as f64 * 0.008;
            vec![session(
                from,
                (from + r.between(30, 90)).min(latest),
                firefox(s.0, s.1, s.2),
                sonos,
                move |x| base + 0.06 * x,
            )]
        }
        // Two or three records back to back.
        4 | 5 => {
            let count = r.between(2, 4);
            let mut at = 18 * 60 + r.between(0, 18) * 10;
            let level = 0.38 + r.between(0, 12) as f64 * 0.01;
            (0..count)
                .map(|i| {
                    let a = album(&mut r);
                    let len = r.between(45, 105);
                    let from = at;
                    at = (at + len + r.between(5, 30)).min(latest);
                    record(
                        &mut r,
                        from,
                        (from + len).min(latest),
                        a,
                        scarlett,
                        move |x| level + 0.02 * i as f64 + 0.03 * (x * 5.0).sin(),
                    )
                })
                .collect()
        }
        // An ordinary evening.
        _ => {
            let a = album(&mut r);
            let from = 19 * 60 + r.between(0, 24) * 10;
            let level = 0.36 + r.between(0, 14) as f64 * 0.01;
            let to = (from + r.between(50, 150)).min(latest);
            vec![record(&mut r, from, to, a, scarlett, move |x| {
                level + 0.04 * (x * 5.0).sin()
            })]
        }
    }
}

/// The laptop: daytime and browser-heavy, shorter, quieter, usually on
/// headphones. Overlaps `ce2` on some evenings, which is what makes two
/// hosts worth reporting together — that is two sources of sound at once.
fn ce3(r: &mut Rand) -> Vec<Session> {
    if r.chance(28) {
        return Vec::new();
    }
    let latest = 23 * 60 + 40;

    match r.below(8) {
        // Background listening across the working day.
        0 | 1 => {
            let s = STREAMS[r.below(STREAMS.len())];
            let a = &ALBUMS[r.below(ALBUMS.len())];
            let from = 8 * 60 + r.between(0, 18) * 10;
            let later = from + r.between(180, 330);
            vec![
                session(
                    from,
                    from + r.between(60, 140),
                    firefox(s.0, s.1, s.2),
                    headphones,
                    |x| 0.22 + 0.03 * (x * 4.0).sin(),
                ),
                {
                    let to = (later + r.between(50, 120)).min(latest);
                    record(r, later, to, a, headphones, |x| 0.25 + 0.02 * x)
                },
            ]
        }
        // A film in the evening on the laptop's own speakers.
        2 => {
            let f = FILMS[r.below(FILMS.len())];
            let from = 19 * 60 + r.between(0, 20) * 10;
            let level = 0.44 + r.between(0, 8) as f64 * 0.01;
            vec![session(
                from,
                (from + r.between(100, 170)).min(latest),
                mpv(f.0, f.1, f.2),
                laptop,
                move |x| level + 0.05 * x,
            )]
        }
        // A game, which like Mixxx is invisible to the bus.
        5 => {
            let from = 21 * 60 + r.between(0, 14) * 10;
            let level = 0.30 + r.between(0, 10) as f64 * 0.01;
            vec![direct(
                from,
                (from + r.between(40, 130)).min(latest),
                "0 A.D.",
                "pyrogenesis",
                laptop,
                move |x| level + 0.04 * x,
            )]
        }
        // A single short thing around lunch.
        3 => {
            let s = STREAMS[r.below(STREAMS.len())];
            let from = 11 * 60 + 30 + r.between(0, 14) * 10;
            vec![session(
                from,
                from + r.between(20, 55),
                firefox(s.0, s.1, s.2),
                laptop,
                |x| 0.30 + 0.04 * x,
            )]
        }
        // Several short stretches scattered through the day.
        4 => {
            let count = r.between(3, 6);
            let mut at = 8 * 60 + r.between(0, 12) * 10;
            (0..count)
                .map(|i| {
                    let s = STREAMS[r.below(STREAMS.len())];
                    let len = r.between(25, 60);
                    let from = at;
                    at = (at + len + r.between(60, 180)).min(latest);
                    session(
                        from,
                        (from + len).min(latest),
                        firefox(s.0, s.1, s.2),
                        headphones,
                        move |x| 0.20 + 0.02 * i as f64 + 0.03 * x,
                    )
                })
                .collect()
        }
        // A late evening overlapping the other machine.
        _ => {
            let a = &ALBUMS[r.below(ALBUMS.len())];
            let from = 20 * 60 + 30 + r.between(0, 16) * 10;
            let level = 0.26 + r.between(0, 8) as f64 * 0.01;
            let to = (from + r.between(60, 150)).min(latest);
            vec![record(r, from, to, a, headphones, move |x| {
                level + 0.04 * (x * 5.0).sin()
            })]
        }
    }
}

struct Session {
    /// Minutes from that day's midnight. May exceed 1440: a film started
    /// before midnight and finishing after it is one session, and the log
    /// has to carry it across the day boundary.
    from: i32,
    to: i32,
    player: PlayerState,
    sink: fn(f64) -> Sink,
    /// Boxed rather than a plain `fn`: the curves vary with the date, so
    /// they close over it.
    gain: Box<dyn Fn(f64) -> f64>,
    /// Each track of the record, and the minutes from the session's own
    /// start that it sounds for. Empty where the session plays one thing
    /// straight through -- a film, a stream.
    cuts: Vec<(PlayerState, i32, i32)>,
    /// Set where the program never announces itself on the bus. PipeWire
    /// sees the stream and MPRIS sees nothing, which is the case the
    /// stream record exists for; `player` is then only bookkeeping for
    /// the no-two-days-alike check and never reaches the log.
    app: Option<(&'static str, &'static str)>,
}

/// What a session is putting through the sink at some minute.
enum Sounding {
    /// Announced on the bus, with a track behind it.
    Bus(PlayerState),
    /// Seen only in the audio graph.
    Direct(Stream),
}

impl Session {
    /// What is sounding at a given minute, if anything. Between two tracks
    /// of a record there is a moment where nothing is, and the log says so.
    fn at(&self, minute: i32) -> Option<Sounding> {
        if minute < self.from || minute > self.to {
            return None;
        }
        if let Some((app, binary)) = self.app {
            return Some(Sounding::Direct(Stream {
                app: app.to_owned(),
                binary: Some(binary.to_owned()),
                pid: Some(4000 + (self.from.unsigned_abs() % 900) as i64),
                media: Some(app.to_owned()),
                volume: Some(1.0),
            }));
        }
        if self.cuts.is_empty() {
            return Some(Sounding::Bus(self.player.clone()));
        }
        self.cuts
            .iter()
            .find(|(_, a, b)| minute >= self.from + a && minute < self.from + b)
            .map(|(p, _, _)| Sounding::Bus(p.clone()))
    }
}

fn session(
    from: i32,
    to: i32,
    player: PlayerState,
    sink: fn(f64) -> Sink,
    gain: impl Fn(f64) -> f64 + 'static,
) -> Session {
    Session {
        from,
        to,
        player,
        sink,
        gain: Box::new(gain),
        cuts: Vec::new(),
        app: None,
    }
}

/// A session played by a program that is not on the bus.
fn direct(
    from: i32,
    to: i32,
    app: &'static str,
    binary: &'static str,
    sink: fn(f64) -> Sink,
    gain: impl Fn(f64) -> f64 + 'static,
) -> Session {
    let mut s = session(from, to, player(binary, app, app, app, binary), sink, gain);
    s.app = Some((app, binary));
    s
}

/// A session that plays a record, track by track.
fn record(
    r: &mut Rand,
    from: i32,
    to: i32,
    a: &'static Album,
    sink: fn(f64) -> Sink,
    gain: impl Fn(f64) -> f64 + 'static,
) -> Session {
    let mut s = session(from, to, mpd(a), sink, gain);
    s.cuts = cuts(r, a, to - from);
    s
}

/// Lays a record across a session: its tracks in order, starting wherever
/// the listener dropped the needle and going round again if they stayed
/// longer than the record lasts, with the occasional minute or two between
/// tracks for whoever got up to change it.
///
/// Those small gaps are the point. A report has to show a run of songs as a
/// run -- not as one undivided block, and not as forty marks fighting each
/// other for the same inch of page.
fn cuts(r: &mut Rand, a: &'static Album, minutes: i32) -> Vec<(PlayerState, i32, i32)> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut i = r.below(a.tracks.len());
    while at < minutes {
        let end = (at + a.tracks[i % a.tracks.len()].1).min(minutes);
        out.push((mpd_track(a, i % a.tracks.len()), at, end));
        at = end + if r.chance(18) { r.between(1, 5) } else { 0 };
        i += 1;
    }
    out
}

/// Where along a session's gain curve a given minute falls, read on the
/// five-minute grid so that the beats inside one step are identical. The
/// curve is only ever drawn in five-minute buckets, so nothing is lost.
fn grid(minute: i32, from: i32, span: f64) -> f64 {
    ((minute - minute.rem_euclid(5) - from) as f64 / span).clamp(0.0, 1.0)
}

/// Writes one day of beats, from its midnight to the next.
///
/// Every five minutes, all day, whether anything is playing or not -- which
/// is what a running daemon does, and what makes a silent day in a report
/// mean *nothing played* rather than *nobody was recording*. Idle stretches
/// collapse to one line per run, so a quiet day costs almost nothing.
///
/// Takes the sessions that start today plus any that ran in from yesterday,
/// and returns whatever runs on into tomorrow.
fn day(
    log: &mut Log,
    midnight: &Zoned,
    sessions: Vec<Session>,
    rest: &mut Sink,
    // The minute the machine is shut down, if it is shut down today.
    until: Option<i32>,
) -> Result<Vec<Session>> {
    // The day's own length: 25 October is twenty-five hours long, and a loop
    // that assumed 1440 minutes would stop an hour early on it.
    let tomorrow = midnight.checked_add(JiffSpan::new().days(1))?;
    let length = (&tomorrow - midnight).total(jiff::Unit::Minute)? as i32;
    let beat_until = until.unwrap_or(length).min(length);
    let moment = |minutes: i32| -> Result<Zoned> {
        Ok(midnight.checked_add(JiffSpan::new().minutes(minutes as i64))?)
    };

    let mut minute = 0;
    // A beat every minute while a session is under way, every five while
    // nothing is. The five-minute grid alone could not carry a record: a
    // span ends at the last beat that saw the track, so a four-minute song
    // between two beats came back as a span of no length at all. A minute
    // is also nearer what the daemon really does.
    //
    // It costs almost nothing. The gain is still read on the five-minute
    // grid, so the beats inside one step are identical and collapse to the
    // single rewritten line they would have been anyway.
    let mut written = -1;
    while minute < beat_until {
        let when = moment(minute)?;
        for s in &sessions {
            // Only where the session really starts: one carried in from
            // yesterday appeared yesterday. A program that is not on the
            // bus announces nothing at all -- no appearance, no playback,
            // no departure; the heartbeat's stream list is the only trace
            // it leaves, which is the whole point of the case.
            if s.app.is_some() {
                continue;
            }
            if minute == s.from {
                log.append(
                    &Event::PlayerAppeared {
                        player: s.player.player.clone(),
                        identity: s.player.identity.clone(),
                    },
                    &when,
                )?;
            }
            if minute == s.to + 1 {
                log.append(
                    &Event::PlayerGone {
                        player: s.player.player.clone(),
                    },
                    &when,
                )?;
            }
            let span = (s.to - s.from).max(1) as f64;
            // A track starts on its own minute rather than on the next
            // beat, which is what keeps the short ones in the record.
            for (track, from, _) in &s.cuts {
                let at = s.from + from;
                if at <= written || at > minute {
                    continue;
                }
                let sink = (s.sink)(round((s.gain)(grid(at, s.from, span))));
                log.append(
                    &Event::Playback(Box::new(Playing {
                        state: track.clone(),
                        sink: Some(sink),
                    })),
                    &moment(at)?,
                )?;
            }
            // A session that plays one thing straight through -- a film, a
            // stream -- says so once, where it starts.
            if s.cuts.is_empty() && minute == s.from {
                let sink = (s.sink)(round((s.gain)(0.0)));
                log.append(
                    &Event::Playback(Box::new(Playing {
                        state: s.player.clone(),
                        sink: Some(sink),
                    })),
                    &when,
                )?;
            }
        }
        written = minute;

        let playing = sessions.iter().find_map(|s| s.at(minute).map(|p| (s, p)));
        // Inside a session but between two of its tracks the daemon is still
        // there and nothing is sounding, which is a beat worth having.
        let inside = sessions.iter().any(|s| minute >= s.from && minute <= s.to);
        if playing.is_some() || inside || minute % 5 == 0 {
            match playing {
                Some((s, sounding)) => {
                    let span = (s.to - s.from).max(1) as f64;
                    let sink = (s.sink)(round((s.gain)(grid(minute, s.from, span))));
                    let (players, streams) = match sounding {
                        Sounding::Bus(p) => (vec![p], Vec::new()),
                        Sounding::Direct(st) => (Vec::new(), vec![st]),
                    };
                    let state = fingerprint(&players, &streams, &Some(sink.clone()));
                    log.beat(players, streams, Some(sink.clone()), state, &when)?;
                    // The volume is left where the listener left it.
                    *rest = sink;
                }
                None => {
                    // The beat that says the daemon was there and nothing
                    // played.
                    let state = fingerprint(&[], &[], &Some(rest.clone()));
                    log.beat(Vec::new(), Vec::new(), Some(rest.clone()), state, &when)?;
                }
            }
        }
        minute += 1;
    }

    Ok(sessions
        .into_iter()
        .filter(|s| s.to >= length)
        .map(|s| Session {
            from: s.from - length,
            to: s.to - length,
            ..s
        })
        .collect())
}

/// Where each machine's volume sits when nothing is playing.
fn idle_sink(host: &str) -> Sink {
    if host == "ce3" {
        laptop(0.35)
    } else {
        scarlett(0.45)
    }
}

fn at(tz: &TimeZone, year: i16, month: i8, day_of: i8, hour: i8, minute: i8) -> Zoned {
    date(year, month, day_of)
        .at(hour, minute, 0, 0)
        .to_zoned(tz.clone())
        .expect("fixture times are valid")
}

/// Three decimals, matching what PipeWire reports in practice.
fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn player(name: &str, identity: &str, title: &str, artist: &str, url: &str) -> PlayerState {
    PlayerState {
        player: name.to_owned(),
        identity: Some(identity.to_owned()),
        status: "Playing".to_owned(),
        player_volume: Some(1.0),
        position_us: Some(0),
        track: Track {
            title: Some(title.to_owned()),
            artist: vec![artist.to_owned()],
            url: Some(url.to_owned()),
            trackid: Some("/t/1".to_owned()),
            length_us: Some(1_800_000_000),
            ..Track::default()
        },
    }
}

/// The record itself, which is what a session is identified by. Which of
/// its tracks is sounding comes from `mpd_track`.
fn mpd(a: &Album) -> PlayerState {
    player(
        "mpd",
        "Music Player Daemon (mpdris2-rs)",
        a.title,
        a.artist,
        &format!("{}/01.flac", a.dir),
    )
}

fn mpd_track(a: &Album, i: usize) -> PlayerState {
    let mut p = player(
        "mpd",
        "Music Player Daemon (mpdris2-rs)",
        a.tracks[i].0,
        a.artist,
        &format!("{}/{:02}.flac", a.dir, i + 1),
    );
    p.track.album = Some(a.title.to_owned());
    p.track.trackid = Some(format!("/t/{}", i + 1));
    p.track.length_us = Some(a.tracks[i].1 as i64 * 60_000_000);
    p
}

fn mpv(title: &str, artist: &str, url: &str) -> PlayerState {
    player("mpv", "mpv", title, artist, url)
}

fn firefox(title: &str, artist: &str, url: &str) -> PlayerState {
    player("firefox", "Firefox", title, artist, url)
}

/// Pro-audio: no device route, so the node carries the gain — the shape
/// `ce2` actually has.
fn scarlett(gain: f64) -> Sink {
    Sink {
        name: "alsa_output.usb-Focusrite_Scarlett_4i4_USB-00.pro-output-0".to_owned(),
        description: Some("Scarlett 4i4 USB Pro".to_owned()),
        device_api: Some("alsa".to_owned()),
        channel_volumes: vec![gain, gain],
        route_volumes: None,
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: None,
    }
}

/// The laptop's own speakers: mixer-backed, like most ALSA sinks, with the
/// node mirroring the route as a built-in analog sink was measured doing.
fn laptop(gain: f64) -> Sink {
    Sink {
        name: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_owned(),
        description: Some("Built-in Audio".to_owned()),
        device_api: Some("alsa".to_owned()),
        channel_volumes: vec![gain, gain],
        route_volumes: Some(vec![gain, gain]),
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: None,
    }
}

/// Headphones on the same card, a different route — and the other shape a
/// routed sink takes, with the node left at 1.0 while the route carries the
/// gain. Both shapes mean the same level.
fn headphones(gain: f64) -> Sink {
    Sink {
        name: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_owned(),
        description: Some("Built-in Audio — Headphones".to_owned()),
        device_api: Some("alsa".to_owned()),
        channel_volumes: vec![1.0, 1.0],
        route_volumes: Some(vec![gain, gain]),
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: None,
    }
}

/// Bluetooth: node and route both carry the gain, the mirrored shape a real
/// speaker was measured in. Multiplying the two would square it, so the
/// fixture exercises that deliberately.
fn sonos(gain: f64) -> Sink {
    Sink {
        name: "bluez_output.F8_4E_17_02_9A_C1.1".to_owned(),
        description: Some("Sonos Roam".to_owned()),
        device_api: Some("bluez5".to_owned()),
        channel_volumes: vec![gain, gain],
        route_volumes: Some(vec![gain, gain]),
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: Some(media_logger::event::Bluetooth {
            address: Some("F8:4E:17:02:9A:C1".to_owned()),
            profile: Some("a2dp-sink".to_owned()),
            codec: Some("aac".to_owned()),
        }),
    }
}
