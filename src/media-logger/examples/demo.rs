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

use media_logger::event::{Event, PlayerState, Playing, Sink, Track};
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

/// A small library, so a season does not read as one evening repeated.
const ALBUMS: &[(&str, &str, &str)] = &[
    (
        "Music for 18 Musicians",
        "Steve Reich",
        "sync/reich/01.flac",
    ),
    (
        "II (Continuous Mix)",
        "Voices from the Lake",
        "sync/vftl/11.flac",
    ),
    (
        "Selected Ambient Works Volume II",
        "Aphex Twin",
        "sync/afx/saw2-03.flac",
    ),
    (
        "Symphony No. 3, Op. 36: II. Lento",
        "Henryk Górecki",
        "sync/gorecki/02.flac",
    ),
    ("Spiegel im Spiegel", "Arvo Pärt", "sync/part/spiegel.flac"),
    (
        "A Rainbow in Curved Air",
        "Terry Riley",
        "sync/riley/rainbow.flac",
    ),
    ("Discreet Music", "Brian Eno", "sync/eno/discreet.flac"),
    (
        "The Disintegration Loops I",
        "William Basinski",
        "sync/basinski/dlp1.flac",
    ),
    ("Pulses", "Steve Reich", "sync/reich/pulses.flac"),
    (
        "Harmony of Difference",
        "Kamasi Washington",
        "sync/kamasi/truth.flac",
    ),
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
            "|{:?}{:?}{}{}{:.4}{:.4}",
            s.from,
            s.to,
            s.player.player,
            s.player.track.title.as_deref().unwrap_or(""),
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
    let album = |r: &mut Rand| {
        let a = ALBUMS[r.below(ALBUMS.len())];
        (a.0, a.1, a.2)
    };

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
            vec![session(from, to, mpd(a.0, a.1, a.2), scarlett, move |x| {
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
                session(
                    after,
                    (after + r.between(55, 120)).min(latest),
                    mpd(a.0, a.1, a.2),
                    scarlett,
                    move |x| level + 0.03 * (x * 6.0).sin(),
                ),
            ]
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
                    session(
                        from,
                        (from + len).min(latest),
                        mpd(a.0, a.1, a.2),
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
            vec![session(
                from,
                (from + r.between(50, 150)).min(latest),
                mpd(a.0, a.1, a.2),
                scarlett,
                move |x| level + 0.04 * (x * 5.0).sin(),
            )]
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
            let a = ALBUMS[r.below(ALBUMS.len())];
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
                session(
                    later,
                    (later + r.between(50, 120)).min(latest),
                    mpd(a.0, a.1, a.2),
                    headphones,
                    |x| 0.25 + 0.02 * x,
                ),
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
            let a = ALBUMS[r.below(ALBUMS.len())];
            let from = 20 * 60 + 30 + r.between(0, 16) * 10;
            let level = 0.26 + r.between(0, 8) as f64 * 0.01;
            vec![session(
                from,
                (from + r.between(60, 150)).min(latest),
                mpd(a.0, a.1, a.2),
                headphones,
                move |x| level + 0.04 * (x * 5.0).sin(),
            )]
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
    }
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
    while minute < beat_until {
        let when = moment(minute)?;
        match sessions.iter().find(|s| minute >= s.from && minute <= s.to) {
            Some(s) => {
                let span = (s.to - s.from).max(1) as f64;
                let sink = (s.sink)(round((s.gain)((minute - s.from) as f64 / span)));
                // Only where the session really starts: one carried in from
                // yesterday appeared yesterday.
                if minute == s.from {
                    log.append(
                        &Event::PlayerAppeared {
                            player: s.player.player.clone(),
                            identity: s.player.identity.clone(),
                        },
                        &when,
                    )?;
                    log.append(
                        &Event::Playback(Box::new(Playing {
                            state: s.player.clone(),
                            sink: Some(sink.clone()),
                        })),
                        &when,
                    )?;
                }
                let players = vec![s.player.clone()];
                let state = fingerprint(&players, &Some(sink.clone()));
                log.beat(players, Some(sink.clone()), state, &when)?;
                // The volume is left where the listener left it.
                *rest = sink;
            }
            None => {
                // The beat that says the daemon was there and nothing played.
                if let Some(done) = sessions.iter().find(|s| minute == s.to + 5) {
                    log.append(
                        &Event::PlayerGone {
                            player: done.player.player.clone(),
                        },
                        &when,
                    )?;
                }
                let state = fingerprint(&[], &Some(rest.clone()));
                log.beat(Vec::new(), Some(rest.clone()), state, &when)?;
            }
        }
        minute += 5;
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

fn mpd(title: &str, artist: &str, url: &str) -> PlayerState {
    player(
        "mpd",
        "Music Player Daemon (mpdris2-rs)",
        title,
        artist,
        url,
    )
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
