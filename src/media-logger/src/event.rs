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

fn linear() -> String {
    "linear".to_owned()
}

fn one() -> u32 {
    1
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
    /// Node-stage gain, linear. On an ALSA sink with a hardware mixer this
    /// is pinned at 1.0 no matter what the volume is — the mixer's volume
    /// lives on the device route below.
    pub channel_volumes: Vec<f64>,
    /// Device-route gain, linear, where the sink has a route. This is where
    /// an ALSA mixer actually keeps its volume. Absent on sinks with no
    /// route, such as a pro-audio interface whose level is set by a knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_volumes: Option<Vec<f64>>,
    /// Names the scale of `channel_volumes` in the record itself, so a reader
    /// years later cannot mistake it for the percentage a slider displayed.
    #[serde(default = "linear")]
    pub volume_scale: String,
    pub mute: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bluetooth: Option<Bluetooth>,
}

impl Sink {
    /// Gain actually applied, per channel: the node stage multiplied by the
    /// route stage.
    ///
    /// Reading only the node recorded a constant 1.0 on every ALSA sink with
    /// a hardware mixer. The two stages multiply, exactly as the player and
    /// sink stages do — the principle simply also applies inside the sink.
    pub fn effective(&self) -> Vec<f64> {
        match &self.route_volumes {
            Some(route) => self
                .channel_volumes
                .iter()
                .zip(
                    route
                        .iter()
                        .chain(std::iter::repeat(route.last().unwrap_or(&1.0))),
                )
                .map(|(n, r)| n * r)
                .collect(),
            None => self.channel_volumes.clone(),
        }
    }

    /// Loudest channel of the effective gain.
    pub fn peak(&self) -> Option<f64> {
        self.effective()
            .into_iter()
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            })
    }
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
    Heartbeat(Box<FullBeat>),
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playing {
    #[serde(flatten)]
    pub state: PlayerState,
    /// Carried on every playback event, not only when it changed — the
    /// volume in force when a track started is exactly what reproduction
    /// needs, and `sink_changed` alone would not record it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink: Option<Sink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkChange {
    pub sink: Sink,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Sink>,
}

/// One heartbeat, standing for every sample it covers.
///
/// State is sampled far more often than it is written: a sample that differs
/// is recorded at once, and a run of identical samples collapses into a
/// single line naming how many there were and when the run began. Fifteen
/// identical lines said no more than one line saying "fifteen samples, no
/// change", and cost fifteen times as much to keep.
///
/// The run is bounded in time, not only by change, so the log never depends
/// on a run ending: what is on disk is never more than one write interval
/// behind, which is also all a power cut can take.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullBeat {
    /// Empty when nothing is playing.
    pub players: Vec<PlayerState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink: Option<Sink>,
    /// Fingerprint of this state.
    pub state: String,
    /// Samples this line stands for, including the one that wrote it.
    ///
    /// Defaulted for logs written before runs were collapsed, where every
    /// line stood for exactly one sample.
    #[serde(default = "one")]
    pub beats: u32,
    /// When the run of identical samples began. Equals this line's own `ts`
    /// when the state had just changed, and is empty in logs written before
    /// runs were collapsed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub since: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(node: Vec<f64>, route: Option<Vec<f64>>) -> Sink {
        Sink {
            name: "s".to_owned(),
            description: None,
            device_api: None,
            channel_volumes: node,
            route_volumes: route,
            volume_scale: "linear".to_owned(),
            mute: false,
            bluetooth: None,
        }
    }

    /// The defect 6a fixes: an ALSA sink with a hardware mixer pins the node
    /// at 1.0 and keeps the real volume on the route, so reading the node
    /// alone recorded a constant no matter what the volume was.
    #[test]
    fn effective_gain_multiplies_both_sink_stages() {
        let mixer = sink(vec![1.0, 1.0], Some(vec![0.125, 0.125]));
        assert_eq!(mixer.effective(), vec![0.125, 0.125]);
        assert_eq!(mixer.peak(), Some(0.125));

        // A sink with no route — pro audio, level set by a knob — keeps the
        // node's own value.
        let knob = sink(vec![0.4, 0.4], None);
        assert_eq!(knob.effective(), vec![0.4, 0.4]);

        // Attenuation at both stages compounds.
        let both = sink(vec![0.5, 0.5], Some(vec![0.5, 0.5]));
        assert_eq!(both.peak(), Some(0.25));
    }

    /// A route reporting fewer channels than the node must not truncate the
    /// result or panic.
    #[test]
    fn a_shorter_route_extends_to_every_channel() {
        let four = sink(vec![1.0, 1.0, 1.0, 1.0], Some(vec![0.5]));
        assert_eq!(four.effective(), vec![0.5, 0.5, 0.5, 0.5]);
    }

    /// Logs written before route volumes existed must still read.
    #[test]
    fn older_records_without_a_route_still_parse() {
        let json = r#"{"name":"s","channel_volumes":[0.6],"volume_scale":"linear","mute":false}"#;
        let parsed: Sink = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.route_volumes, None);
        assert_eq!(parsed.peak(), Some(0.6));
    }
}
