//! media-logger — a tamper-evident log of media playback, output device and
//! volume.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use jiff::Zoned;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zbus::Connection;

use media_logger::event::{Event, Playing, Sink, SinkChange};
use media_logger::log::Log;
use media_logger::{anchor, audio, log, mpris, query, quiet, render};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often closed days are checked for a missing timestamp. Rollover makes
/// a day pending; this picks it up, and retries whatever the network refused
/// last time without any queue to keep.
const ANCHOR_EVERY: Duration = Duration::from_secs(3600);

/// Longest a sink sample is reused. Bounds `pw-dump` spawns when a player
/// emits property changes rapidly, without making the record meaningfully
/// staler than the event it accompanies.
const SINK_CACHE: Duration = Duration::from_secs(1);

#[derive(Parser)]
#[command(name = "media-logger", version, about)]
struct Cli {
    /// Root of the log tree. Each host writes only to <root>/<hostname>/.
    #[arg(long, env = "MEDIA_LOGGER_ROOT")]
    root: PathBuf,

    /// Host name to log under. Defaults to this machine's hostname.
    #[arg(long, env = "MEDIA_LOGGER_HOST")]
    host: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the logging daemon.
    Run {
        /// Seconds between heartbeats.
        ///
        /// A run of unchanged beats collapses into one line that is rewritten
        /// in place, so a short interval costs resolution, not storage.
        #[arg(long, default_value_t = 20)]
        heartbeat: u64,
    },
    /// Check the hash chain and the anchors over this host's tree.
    Verify,
    /// Report on a moment, a day, or a range.
    ///
    /// <WHEN> accepts an instant (2026-10-22T23:14), a day (2026-10-22), a
    /// range (2026-10-01..2026-10-31), or today / yesterday / last-night.
    Show {
        when: String,

        #[arg(long, value_enum, default_value_t = render::Format::Text)]
        format: render::Format,

        /// Write to a file instead of stdout. Required for pdf and xlsx.
        #[arg(long, short)]
        out: Option<PathBuf>,

        /// Quiet hours, repeatable. Defaults to German Nachtruhe.
        ///
        /// Accepts "22:00-06:00", "sun 00:00-24:00", "mon-fri 13:00-15:00".
        /// Mittagsruhe and Sunday rules come from a building's Hausordnung
        /// and are not applied unless given; common additions are
        /// 13:00-15:00 (Mittagsruhe) and sun 00:00-24:00 (Sundays).
        ///
        /// Also read from MEDIA_LOGGER_QUIET_HOURS as a ";"-separated list,
        /// which the NixOS module sets from its own option.
        #[arg(long = "quiet-hours", value_name = "SPEC")]
        quiet_hours: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let host = cli
        .host
        .clone()
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());

    match cli.command {
        Command::Verify => verify(&cli.root, &host),
        Command::Show {
            when,
            format,
            out,
            quiet_hours,
        } => show(
            &cli.root,
            &host,
            &when,
            format,
            out.as_deref(),
            &quiet_hours,
        ),
        Command::Run { heartbeat } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(cli.root, host, Duration::from_secs(heartbeat))),
    }
}

fn verify(root: &std::path::Path, host: &str) -> Result<()> {
    let host_root = root.join(host);
    let report = log::verify(&host_root)?;
    let mut o = String::new();
    o.push_str(&format!("{} files, {} lines\n", report.files, report.lines));
    for restart in &report.restarts {
        o.push_str(&format!("  chain restart: {restart}\n"));
    }
    o.push_str(if report.ok() {
        "chain intact\n"
    } else {
        "chain BROKEN\n"
    });
    for problem in &report.problems {
        o.push_str(&format!("  PROBLEM: {problem}\n"));
    }

    // A chain nobody anchored proves only internal consistency, so the
    // anchors are reported alongside it rather than behind a second command.
    o.push_str("\nanchors:\n");
    let today = Zoned::now().date().to_string();
    let mut unstamped = 0;
    let mut invalid = 0;
    for day in log::day_files(&host_root)? {
        let name = day
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let open = name == today;
        let states: Vec<String> = anchor::check(&day)
            .into_iter()
            .map(|(tsa, state)| match state {
                anchor::TokenState::Valid { at } => format!("{} OK {at}", tsa.name),
                anchor::TokenState::Missing => {
                    if !open {
                        unstamped += 1;
                    }
                    format!("{} —", tsa.name)
                }
                anchor::TokenState::Invalid(why) => {
                    invalid += 1;
                    format!("{} INVALID ({why})", tsa.name)
                }
            })
            .collect();
        let note = if open { "  (today, still open)" } else { "" };
        o.push_str(&format!("  {name}  {}{note}\n", states.join(" | ")));
    }
    if unstamped > 0 {
        o.push_str(&format!(
            "\n{unstamped} closed day(s) still awaiting a token\n"
        ));
    }
    render::to_stdout(o.trim_end())?;

    if invalid > 0 {
        anyhow::bail!("{invalid} invalid token(s)");
    }
    if !report.ok() {
        anyhow::bail!("{} chain problem(s)", report.problems.len());
    }
    Ok(())
}

fn show(
    root: &std::path::Path,
    host: &str,
    when: &str,
    format: render::Format,
    out: Option<&std::path::Path>,
    quiet_hours: &[String],
) -> Result<()> {
    let host_root = root.join(host);
    let specs: Vec<String> = if !quiet_hours.is_empty() {
        quiet_hours.to_vec()
    } else {
        match std::env::var("MEDIA_LOGGER_QUIET_HOURS") {
            Ok(v) if !v.trim().is_empty() => v
                .split(';')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => vec![quiet::DEFAULT.to_owned()],
        }
    };
    let quiet = quiet::QuietHours::parse(&specs)?;

    let window = query::parse_when(when, &Zoned::now())?;
    let integrity = query::integrity(&host_root, &window)?;
    let report = query::build(&host_root, host, window)?;
    render::render(&report, &integrity, &quiet, format, out)
}

/// Stamps any closed day that lacks a token.
///
/// The work runs on a blocking thread: each request can take seconds, and
/// stalling the main loop would tear a hole in the heartbeat sequence —
/// which is exactly the gap the heartbeat exists to rule out.
async fn anchor_pending(log: &mut Log, host_root: PathBuf) -> Result<()> {
    let today = Zoned::now().date().to_string();
    let stamped = tokio::task::spawn_blocking(move || {
        let mut done = Vec::new();
        for (day, tsas) in anchor::pending(&host_root, &today)? {
            for tsa in tsas {
                match anchor::stamp(&day, tsa) {
                    Ok(s) => done.push((day.clone(), tsa.name, s.at)),
                    // Offline is the ordinary case, not an error: the day
                    // stays pending and the next sweep retries it. Never
                    // backdate, and never record a stamp we did not get.
                    Err(e) => eprintln!(
                        "media-logger: stamping {} with {}: {e:#}",
                        day.display(),
                        tsa.name
                    ),
                }
            }
        }
        anyhow::Ok(done)
    })
    .await??;

    for (day, tsa, at) in stamped {
        let date = day
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        log.append(
            &Event::Anchor {
                queued: is_late(&date, &at),
                date,
                tsa: tsa.to_owned(),
                stamped_at: at,
            },
            &Zoned::now(),
        )?;
    }
    Ok(())
}

/// A day is normally stamped the morning after it closes. Later than that
/// means the sweep could not reach a TSA when it should have.
fn is_late(date: &str, stamped_at: &str) -> bool {
    let (Ok(day), Ok(stamp)) = (
        date.parse::<jiff::civil::Date>(),
        stamped_at
            .get(..10)
            .unwrap_or("")
            .parse::<jiff::civil::Date>(),
    ) else {
        return false;
    };
    day.tomorrow().is_ok_and(|next| stamp > next)
}

/// What the watcher tasks report back to the single writer.
enum Signal {
    Appeared(String),
    Gone(String),
    Changed(String),
}

struct Player {
    identity: Option<String>,
    watcher: JoinHandle<()>,
}

/// Rate-limited view of the current output device.
struct SinkCache {
    sampled: Option<Instant>,
    sink: Option<Sink>,
}

impl SinkCache {
    fn new() -> Self {
        SinkCache {
            sampled: None,
            sink: None,
        }
    }

    /// Returns the current sink, and the previous one when it changed.
    fn refresh(&mut self) -> (Option<Sink>, Option<Sink>) {
        if self.sampled.is_some_and(|at| at.elapsed() < SINK_CACHE) {
            return (self.sink.clone(), None);
        }
        self.sampled = Some(Instant::now());
        let sampled = match audio::sample() {
            Ok(sink) => sink,
            Err(e) => {
                eprintln!("media-logger: sampling sink failed: {e:#}");
                return (self.sink.clone(), None);
            }
        };
        let changed = sampled != self.sink;
        let previous = changed.then(|| self.sink.clone()).flatten();
        self.sink = sampled.clone();
        (sampled, changed.then_some(previous).flatten())
    }
}

async fn run(root: PathBuf, host: String, heartbeat: Duration) -> Result<()> {
    let now = Zoned::now();
    // resume declares any discontinuity in-band itself, so verification
    // reports a restart rather than corruption.
    let (mut log, restart) = Log::resume(&root, &host, &now)?;
    if let Some(restart) = &restart {
        eprintln!("media-logger: chain restarted ({:?})", restart.reason);
    }
    log.append(
        &Event::DaemonStart {
            version: VERSION.to_owned(),
        },
        &now,
    )?;

    let conn = mpris::connect().await?;
    let (tx, mut rx) = mpsc::channel::<Signal>(64);
    let mut players: HashMap<String, Player> = HashMap::new();
    let mut sinks = SinkCache::new();

    spawn_name_watcher(&conn, tx.clone()).await?;
    for bus_name in mpris::list_players(&conn).await? {
        tx.send(Signal::Appeared(bus_name)).await.ok();
    }

    let host_root = root.join(&host);
    if let Err(e) = anchor_pending(&mut log, host_root.clone()).await {
        eprintln!("media-logger: anchoring failed: {e:#}");
    }

    let mut anchors = tokio::time::interval(ANCHOR_EVERY);
    anchors.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    anchors.reset(); // the startup sweep already ran

    let mut ticks = tokio::time::interval(heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    let reason = loop {
        tokio::select! {
            Some(signal) = rx.recv() => {
                if let Err(e) =
                    handle(&conn, &mut log, &mut players, &mut sinks, &tx, signal).await
                {
                    // A player can vanish between its signal and our read.
                    // That is ordinary, and must not take the daemon down.
                    eprintln!("media-logger: {e:#}");
                }
            }
            _ = ticks.tick() => {
                if let Err(e) = beat(&conn, &mut log, &players, &mut sinks).await {
                    eprintln!("media-logger: heartbeat failed: {e:#}");
                }
            }
            _ = anchors.tick() => {
                if let Err(e) = anchor_pending(&mut log, host_root.clone()).await {
                    eprintln!("media-logger: anchoring failed: {e:#}");
                }
            }
            _ = sigint.recv() => break "sigint",
            _ = sigterm.recv() => break "sigterm",
        }
    };

    for player in players.values() {
        player.watcher.abort();
    }
    log.append(
        &Event::DaemonStop {
            reason: reason.to_owned(),
        },
        &Zoned::now(),
    )?;
    Ok(())
}

async fn spawn_name_watcher(conn: &Connection, tx: mpsc::Sender<Signal>) -> Result<()> {
    let mut stream = mpris::name_changes(conn).await?;
    tokio::spawn(async move {
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else { continue };
            let name = args.name().as_str().to_owned();
            if !name.starts_with(mpris::PREFIX) {
                continue;
            }
            let signal = if args.new_owner().is_some() {
                Signal::Appeared(name)
            } else {
                Signal::Gone(name)
            };
            if tx.send(signal).await.is_err() {
                break;
            }
        }
    });
    Ok(())
}

async fn handle(
    conn: &Connection,
    log: &mut Log,
    players: &mut HashMap<String, Player>,
    sinks: &mut SinkCache,
    tx: &mpsc::Sender<Signal>,
    signal: Signal,
) -> Result<()> {
    match signal {
        Signal::Appeared(bus_name) => {
            if players.contains_key(&bus_name) {
                return Ok(());
            }
            let identity = mpris::identity(conn, &bus_name).await.unwrap_or(None);
            let watcher = spawn_player_watcher(conn, &bus_name, tx.clone()).await?;
            players.insert(
                bus_name.clone(),
                Player {
                    identity: identity.clone(),
                    watcher,
                },
            );

            log.append(
                &Event::PlayerAppeared {
                    player: mpris::short_name(&bus_name).to_owned(),
                    identity,
                },
                &Zoned::now(),
            )?;
            // Record what it is already doing: a player that was playing
            // before we attached emits no change of its own.
            record_playback(conn, log, players, sinks, &bus_name).await
        }
        Signal::Gone(bus_name) => {
            if let Some(player) = players.remove(&bus_name) {
                player.watcher.abort();
                log.append(
                    &Event::PlayerGone {
                        player: mpris::short_name(&bus_name).to_owned(),
                    },
                    &Zoned::now(),
                )?;
            }
            Ok(())
        }
        Signal::Changed(bus_name) => record_playback(conn, log, players, sinks, &bus_name).await,
    }
}

async fn spawn_player_watcher(
    conn: &Connection,
    bus_name: &str,
    tx: mpsc::Sender<Signal>,
) -> Result<JoinHandle<()>> {
    let mut stream = mpris::changes(conn, bus_name).await?;
    let bus_name = bus_name.to_owned();
    Ok(tokio::spawn(async move {
        while stream.next().await.is_some() {
            if tx.send(Signal::Changed(bus_name.clone())).await.is_err() {
                break;
            }
        }
    }))
}

async fn record_playback(
    conn: &Connection,
    log: &mut Log,
    players: &HashMap<String, Player>,
    sinks: &mut SinkCache,
    bus_name: &str,
) -> Result<()> {
    let identity = players.get(bus_name).and_then(|p| p.identity.clone());
    let state = mpris::snapshot(conn, bus_name, identity).await?;
    let (sink, previous) = sinks.refresh();
    let now = Zoned::now();

    if let Some(current) = &sink {
        if previous.is_some() {
            log.append(
                &Event::SinkChanged(Box::new(SinkChange {
                    sink: current.clone(),
                    previous,
                })),
                &now,
            )?;
        }
    }
    log.append(&Event::Playback(Box::new(Playing { state, sink })), &now)?;
    Ok(())
}

/// Writes the periodic record of what was happening, including when nothing
/// was. The empty case is the one that lets the log prove a negative.
async fn beat(
    conn: &Connection,
    log: &mut Log,
    players: &HashMap<String, Player>,
    sinks: &mut SinkCache,
) -> Result<()> {
    let mut states = Vec::new();
    for (bus_name, player) in players {
        match mpris::snapshot(conn, bus_name, player.identity.clone()).await {
            Ok(state) => states.push(state),
            Err(e) => eprintln!("media-logger: reading {bus_name} failed: {e:#}"),
        }
    }
    states.sort_by(|a, b| a.player.cmp(&b.player));

    let (sink, previous) = sinks.refresh();
    let now = Zoned::now();
    if let Some(current) = &sink {
        if previous.is_some() {
            log.append(
                &Event::SinkChanged(Box::new(SinkChange {
                    sink: current.clone(),
                    previous,
                })),
                &now,
            )?;
        }
    }
    let state = query::fingerprint(&states, &sink);
    log.beat(states, sink, state, &now)
        .context("recording heartbeat")?;
    Ok(())
}
