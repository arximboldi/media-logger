//! media-logger — a tamper-evident log of media playback, output device and
//! volume.

mod audio;
mod event;
mod log;
mod mpris;

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

use crate::event::{Event, Sink};
use crate::log::Log;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        #[arg(long, default_value_t = 60)]
        heartbeat: u64,
    },
    /// Check the hash chain over this host's tree.
    Verify,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let host = cli
        .host
        .clone()
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());

    match cli.command {
        Command::Verify => verify(&cli.root, &host),
        Command::Run { heartbeat } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(cli.root, host, Duration::from_secs(heartbeat))),
    }
}

fn verify(root: &std::path::Path, host: &str) -> Result<()> {
    let report = log::verify(&root.join(host))?;
    println!("{} files, {} lines", report.files, report.lines);
    for restart in &report.restarts {
        println!("  chain restart: {restart}");
    }
    if report.ok() {
        println!("chain intact");
        return Ok(());
    }
    for problem in &report.problems {
        println!("  PROBLEM: {problem}");
    }
    anyhow::bail!("{} problem(s) found", report.problems.len())
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
                &Event::SinkChanged {
                    sink: current.clone(),
                    previous,
                },
                &now,
            )?;
        }
    }
    log.append(
        &Event::Playback {
            state: Box::new(state),
            sink,
        },
        &now,
    )?;
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
                &Event::SinkChanged {
                    sink: current.clone(),
                    previous,
                },
                &now,
            )?;
        }
    }
    log.append(
        &Event::Heartbeat {
            players: states,
            sink,
        },
        &now,
    )
    .context("appending heartbeat")?;
    Ok(())
}
