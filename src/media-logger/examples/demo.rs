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
    for host in HOSTS {
        let (mut log, _) = Log::resume(&root, host, &at(&tz, 2026, 8, 1, 0, 0))?;
        for (year, month, last) in [(2026, 8, 31), (2026, 9, 30), (2026, 10, 31), (2026, 11, 15)] {
            for day_of in 1..=last {
                let sessions = plan(host, month, day_of);
                if sessions.is_empty() {
                    continue;
                }
                day(&mut log, &tz, year, month, day_of, &sessions)?;
            }
        }
    }

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

/// What was played on a given day, as a pure function of host and date.
///
/// Varied on every axis a report can show — what, how many, for how long,
/// starting when, at what level, through which device, on which machine —
/// so the calendar and the filters have something real to distinguish.
/// Nothing here is random: host and date alone decide.
fn plan(host: &str, month: i8, day_of: i8) -> Vec<Session> {
    if host == "ce3" {
        return ce3(month, day_of);
    }

    let n = month as i32 * 31 + day_of as i32;
    let pick = |len: usize, salt: i32| ((n * 7 + salt * 13) as usize) % len;
    let album = ALBUMS[pick(ALBUMS.len(), 1)];
    let other = ALBUMS[pick(ALBUMS.len(), 5)];
    let third = ALBUMS[pick(ALBUMS.len(), 9)];
    let film = FILMS[pick(FILMS.len(), 2)];
    let stream = STREAMS[pick(STREAMS.len(), 3)];

    let start_h = 19 + (n % 3) as i8;
    let start_m = ((n * 17) % 4) as i8 * 15;
    let run = 50 + (n * 23) % 95;
    let latest = 23 * 60 + 50;
    let end = (start_h as i32 * 60 + start_m as i32 + run).min(latest);
    let hm = |m: i32| ((m / 60) as i8, (m % 60) as i8);
    let level = 0.38 + ((n % 7) as f64) * 0.015;

    match n % 13 {
        // Silent.
        0 | 7 => Vec::new(),
        // A loud evening running well into Nachtruhe.
        3 => vec![session(
            (21, ((n * 11) % 4) as i8 * 10),
            (23, 20 + ((n * 5) % 3) as i8 * 10),
            mpd(album.0, album.1, album.2),
            scarlett,
            move |x| 0.42 + 0.26 * x,
        )],
        // An afternoon film, then something quiet after dinner.
        5 => vec![
            session(
                (14 + (n % 3) as i8, 0),
                (16 + (n % 3) as i8, 40),
                mpv(film.0, film.1, film.2),
                scarlett,
                |x| 0.33 + 0.02 * (x * 4.0).sin(),
            ),
            session(
                (20, 30),
                (21, 50),
                mpd(other.0, other.1, other.2),
                scarlett,
                move |x| 0.44 + 0.03 * (x * 6.0).sin(),
            ),
        ],
        // Something short on the Bluetooth speaker.
        8 => vec![session(
            (start_h, start_m),
            hm(end),
            firefox(stream.0, stream.1, stream.2),
            sonos,
            move |x| 0.34 + 0.06 * x,
        )],
        // A long day: three records, afternoon into evening.
        11 => {
            let a = 15 * 60 + ((n * 7) % 4) * 15;
            let b = a + 95;
            let c = b + 70;
            vec![
                session(
                    hm(a),
                    hm(a + 80),
                    mpd(album.0, album.1, album.2),
                    scarlett,
                    move |x| level - 0.04 + 0.02 * (x * 3.0).sin(),
                ),
                session(
                    hm(b),
                    hm(b + 60),
                    mpd(other.0, other.1, other.2),
                    scarlett,
                    move |x| level + 0.02 * x,
                ),
                session(
                    hm(c),
                    hm((c + 85).min(latest)),
                    mpd(third.0, third.1, third.2),
                    scarlett,
                    move |x| level + 0.06 * x,
                ),
            ]
        }
        // Two records back to back.
        9 => {
            let first_end = end.min(20 * 60 + 30);
            let second = (first_end + 40).min(latest);
            vec![
                session(
                    (start_h.min(18), start_m),
                    hm(first_end),
                    mpd(album.0, album.1, album.2),
                    scarlett,
                    move |x| level + 0.03 * (x * 5.0).sin(),
                ),
                session(
                    hm(second),
                    hm((second + 90).min(latest)),
                    mpd(other.0, other.1, other.2),
                    scarlett,
                    move |x| level + 0.05 * x,
                ),
            ]
        }
        // An ordinary evening.
        _ => vec![session(
            (start_h, start_m),
            hm(end),
            mpd(album.0, album.1, album.2),
            scarlett,
            move |x| level + 0.04 * (x * 5.0).sin(),
        )],
    }
}

/// The laptop: daytime and browser-heavy, shorter, quieter, often on
/// headphones. Overlaps `ce2` on some evenings, which is what makes two
/// hosts worth reporting together — that is two sources of sound at once.
fn ce3(month: i8, day_of: i8) -> Vec<Session> {
    let n = month as i32 * 29 + day_of as i32 * 3;
    let pick = |len: usize, salt: i32| ((n * 5 + salt * 11) as usize) % len;
    let stream = STREAMS[pick(STREAMS.len(), 1)];
    let album = ALBUMS[pick(ALBUMS.len(), 4)];
    let film = FILMS[pick(FILMS.len(), 6)];
    let hm = |m: i32| ((m / 60) as i8, (m % 60) as i8);
    let latest = 23 * 60 + 40;

    match n % 7 {
        // Nothing.
        0 | 4 => Vec::new(),
        // Background listening across the working day.
        1 => {
            let a = 9 * 60 + (n % 3) * 20;
            vec![
                session(
                    hm(a),
                    hm(a + 110),
                    firefox(stream.0, stream.1, stream.2),
                    headphones,
                    |x| 0.22 + 0.03 * (x * 4.0).sin(),
                ),
                session(
                    hm(a + 240),
                    hm(a + 330),
                    mpd(album.0, album.1, album.2),
                    headphones,
                    |x| 0.25 + 0.02 * x,
                ),
            ]
        }
        // A film in the evening on the laptop's own speakers.
        2 => vec![session(
            (20, ((n * 3) % 4) as i8 * 10),
            (22, 10 + ((n * 7) % 3) as i8 * 15),
            mpv(film.0, film.1, film.2),
            laptop,
            |x| 0.46 + 0.05 * x,
        )],
        // A single short thing at lunch.
        3 => {
            let a = 12 * 60 + (n % 4) * 15;
            vec![session(
                hm(a),
                hm(a + 35),
                firefox(stream.0, stream.1, stream.2),
                laptop,
                |x| 0.30 + 0.04 * x,
            )]
        }
        // Four short stretches scattered through the day.
        5 => (0..4)
            .map(|i| {
                let a = 8 * 60 + i * 180 + (n % 5) * 10;
                session(
                    hm(a),
                    hm((a + 45).min(latest)),
                    firefox(stream.0, stream.1, stream.2),
                    headphones,
                    move |x| 0.20 + 0.02 * i as f64 + 0.03 * x,
                )
            })
            .collect(),
        // A late evening overlapping the other machine.
        _ => vec![session(
            (21, 30),
            (23, 0),
            mpd(album.0, album.1, album.2),
            headphones,
            |x| 0.28 + 0.04 * (x * 5.0).sin(),
        )],
    }
}

struct Session {
    from: (i8, i8),
    to: (i8, i8),
    player: PlayerState,
    sink: fn(f64) -> Sink,
    /// Boxed rather than a plain `fn`: the curves vary with the date, so
    /// they close over it.
    gain: Box<dyn Fn(f64) -> f64>,
}

fn session(
    from: (i8, i8),
    to: (i8, i8),
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

/// Writes one day: a start, each session sampled every five minutes, and a stop.
fn day(
    log: &mut Log,
    tz: &TimeZone,
    year: i16,
    month: i8,
    day_of: i8,
    sessions: &[Session],
) -> Result<()> {
    let first = sessions
        .iter()
        .map(|s| s.from.0)
        .min()
        .unwrap_or(19)
        .saturating_sub(1);
    let last = sessions.iter().map(|s| s.to.0).max().unwrap_or(22).min(23);
    log.append(
        &Event::DaemonStart {
            version: "0.1.0".to_owned(),
        },
        &at(tz, year, month, day_of, first, 0),
    )?;

    for s in sessions {
        let (begin, end) = (
            s.from.0 as i32 * 60 + s.from.1 as i32,
            s.to.0 as i32 * 60 + s.to.1 as i32,
        );
        let span = (end - begin).max(1) as f64;

        let mut minute = begin;
        while minute <= end {
            let progress = (minute - begin) as f64 / span;
            let sink = (s.sink)(round((s.gain)(progress)));
            let when = at(
                tz,
                year,
                month,
                day_of,
                (minute / 60) as i8,
                (minute % 60) as i8,
            );

            if minute == begin {
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
            log.beat(players, Some(sink), state, &when)?;
            minute += 5;
        }

        // Idle again, so the span closes where it should.
        let after = at(
            tz,
            year,
            month,
            day_of,
            ((end + 5) / 60) as i8,
            ((end + 5) % 60) as i8,
        );
        log.append(
            &Event::PlayerGone {
                player: s.player.player.clone(),
            },
            &after,
        )?;
        let idle = (s.sink)(round((s.gain)(1.0)));
        let state = fingerprint(&[], &Some(idle.clone()));
        log.beat(Vec::new(), Some(idle), state, &after)?;
    }

    log.append(
        &Event::DaemonStop {
            reason: "sigterm".to_owned(),
        },
        &at(tz, year, month, day_of, last, 58),
    )
    .map(|_| ())
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

/// The laptop's own speakers: mixer-backed, like most ALSA sinks.
fn laptop(gain: f64) -> Sink {
    Sink {
        name: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_owned(),
        description: Some("Built-in Audio".to_owned()),
        device_api: Some("alsa".to_owned()),
        channel_volumes: vec![1.0, 1.0],
        route_volumes: Some(vec![gain, gain]),
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: None,
    }
}

/// Headphones on the same card, a different route.
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

/// A mixer-backed device: the node sits at 1.0 and the route carries the
/// gain. This is the shape that recorded a constant before 6a, so the
/// fixture exercises it deliberately.
fn sonos(gain: f64) -> Sink {
    Sink {
        name: "bluez_output.F8_4E_17_02_9A_C1.1".to_owned(),
        description: Some("Sonos Roam".to_owned()),
        device_api: Some("bluez5".to_owned()),
        channel_volumes: vec![1.0, 1.0],
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
