//! Event schema.
//!
//! Records are serialised with a fixed field order so the log reads well by
//! eye. The hash chain covers the literal bytes written, so nothing here needs
//! to be canonicalised — new fields and new event types never affect
//! verification of existing lines.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SCHEMA_VERSION: u32 = 1;

/// `prev` of the first line of a chain.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One line of the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub v: u32,
    /// Local time with offset. Ruhezeiten are wall-clock boundaries, so this
    /// is the field reports work from.
    pub ts: String,
    /// The same instant in UTC, which disambiguates the hour repeated at the
    /// autumn DST transition.
    pub ts_utc: String,
    pub host: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
    pub prev: String,
}

/// Why a chain begins without a predecessor.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartReason {
    /// The logs root itself was missing and had to be created. Ordinarily a
    /// first install; worth recording because it is also what a replicated
    /// tree gone astray would look like.
    CreatedRoot,
    /// No tree for this host under an existing logs root: a genuine first run.
    NewHost,
    /// The host tree exists but holds no readable log.
    EmptyTree,
    /// Every line of the most recent file failed to parse.
    UnreadableTail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sink {
    /// Stable `node.name`, never the node index, which changes across reboots.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `alsa`, `bluez5`, ... The same volume figure means different things
    /// across device APIs, so reproduction needs this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_api: Option<String>,
    /// Raw PipeWire volumes. These are *linear gain*, while every UI shows a
    /// cubic scale; see `volume_scale`.
    pub channel_volumes: Vec<f64>,
    /// Names the scale of `channel_volumes` in the record itself, so a reader
    /// years later cannot mistake it for the percentage a slider displayed.
    pub volume_scale: &'static str,
    pub mute: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bluetooth: Option<Bluetooth>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bluetooth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// `a2dp-sink` against a headset profile is a large level difference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// Identity that survives: tags are ambiguous between masters of the same
    /// work, which matters when the log is used to reproduce a level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trackid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_us: Option<i64>,
    /// Everything the player exposed, so metadata that becomes interesting
    /// later is already captured. `mpris:artUrl` is dropped: it is a local
    /// cache path, useless off this machine and noisy in every line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub player: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    pub status: String,
    /// The player's own gain stage. It multiplies with the sink's, so neither
    /// alone reproduces the output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_volume: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_us: Option<i64>,
    pub track: Track,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    DaemonStart {
        version: String,
    },
    DaemonStop {
        reason: String,
    },
    ChainRestart {
        reason: RestartReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Rollover {
        /// Hash of the last content line of the day. Redundant with this
        /// line's own `prev`, kept so the record is self-describing when a
        /// single day is extracted and read on its own.
        chain_head: String,
        line_count: u64,
        date: String,
    },
    PlayerAppeared {
        player: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<String>,
    },
    PlayerGone {
        player: String,
    },
    Playback(Box<Playing>),
    Heartbeat(Beat),
    /// A day file was timestamped by a third party.
    ///
    /// Written into the *current* day's log about a *closed* day, so it never
    /// disturbs the file it describes.
    Anchor {
        date: String,
        tsa: String,
        /// The time the authority asserts, not our observation of it.
        stamped_at: String,
        /// True when the stamp landed later than the day it covers. Recorded
        /// rather than hidden: a log that silently implies same-day stamping
        /// it did not get is worse than one with an honest gap.
        queued: bool,
    },
    /// Note the semantics: sink state is *sampled*, at playback events and
    /// heartbeats, not subscribed to. This event's timestamp is therefore
    /// when the change was **detected**, bounded by the heartbeat interval,
    /// not when it occurred. A change that happens and reverts between two
    /// samples leaves no trace at all.
    SinkChanged(Box<SinkChange>),
}

/// The payloads large enough to be worth boxing, so moving an `Event` stays
/// cheap and the enum does not carry its biggest variant everywhere.
#[derive(Debug, Clone, Serialize)]
pub struct Playing {
    #[serde(flatten)]
    pub state: PlayerState,
    /// Carried on every playback event, not only when it changed — the
    /// volume in force when a track started is exactly what reproduction
    /// needs, and `sink_changed` alone would not record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink: Option<Sink>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SinkChange {
    pub sink: Sink,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Sink>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FullBeat {
    /// Empty when nothing is playing.
    pub players: Vec<PlayerState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink: Option<Sink>,
    /// Fingerprint of this state, which abbreviated beats refer back to.
    pub state: String,
}

/// The two shapes a heartbeat takes.
///
/// Both forms are heartbeats and both prove the daemon was alive at that
/// instant — which is the whole point, and why an unchanged beat is
/// abbreviated rather than skipped. Omitting it would make a gap ambiguous
/// again, and "nothing was playing at 23:40" is the most valuable claim this
/// log can support.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Beat {
    /// The full picture, written whenever the state changed and periodically
    /// regardless, so the log keeps self-contained anchors.
    Full(Box<FullBeat>),
    /// State identical to the last `Full` beat, named by its fingerprint so
    /// the line is meaningful without trusting position in the file.
    ///
    /// Track position is deliberately excluded from the fingerprint: it
    /// advances every second, and including it would defeat abbreviation
    /// exactly while a track plays, which is when lines are largest. Position
    /// is refreshed at the next full beat.
    Repeat { unchanged: String },
}

impl Event {
    /// Splits the event into the `type` and `data` halves of a [`Record`].
    pub fn split(&self) -> Result<(String, Value)> {
        let mut value = serde_json::to_value(self)?;
        let kind = value
            .get_mut("type")
            .map(Value::take)
            .and_then(|k| k.as_str().map(str::to_owned))
            .unwrap_or_default();
        // Unit variants serialise without a `data` key; give them an empty
        // object so every line has the same shape.
        let data = value
            .get_mut("data")
            .map(Value::take)
            .unwrap_or_else(|| json!({}));
        Ok((kind, data))
    }
}
