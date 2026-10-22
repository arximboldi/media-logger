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
    Playback {
        #[serde(flatten)]
        state: Box<PlayerState>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sink: Option<Sink>,
    },
    Heartbeat {
        /// Empty when nothing is playing. The empty case is the point: it is
        /// what lets the log prove that no media was playing at a given time.
        players: Vec<PlayerState>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sink: Option<Sink>,
    },
    SinkChanged {
        sink: Sink,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous: Option<Sink>,
    },
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
