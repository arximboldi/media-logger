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
    /// Node gain, linear. Usually mirrors `route_volumes` where the sink has
    /// a route; on some sinks it instead sits at 1.0 while the route carries
    /// the volume. Kept raw and separate so a reader can see which.
    pub channel_volumes: Vec<f64>,
    /// Device-route gain, linear, where the sink has a route. This is where
    /// the volume actually lives — for an ALSA mixer, and for Bluetooth,
    /// where it is the AVRCP setting the speaker applies. Absent on sinks
    /// with no route, such as a pro-audio interface whose level is set by a
    /// knob.
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
    /// Gain actually applied, per channel: the route where the sink has one,
    /// otherwise the node's own volume.
    ///
    /// Node and route are one control seen from two objects, not two stages
    /// in series. Where both carry a value they carry the *same* value, so
    /// multiplying them squares the gain — on a Bluetooth speaker sitting at
    /// 0.00858 that logged -82.7 dB for a -41.3 dB setting. Where they
    /// disagree the route is authoritative: it is where a mixer-equipped
    /// device keeps its volume, and a node reading 1.0 beside it means no
    /// node-stage attenuation, not unity times something.
    ///
    /// This is unlike the player and sink stages, which really are in series
    /// and really do multiply.
    pub fn effective(&self) -> Vec<f64> {
        let Some(route) = self.route_volumes.as_deref().filter(|r| !r.is_empty()) else {
            return self.channel_volumes.clone();
        };
        // pw-dump can omit the node's Props entirely; the route still holds
        // the volume, and dropping it would record no volume at all.
        if self.channel_volumes.is_empty() {
            return route.to_vec();
        }
        // A route may report fewer channels than the node; its last value
        // governs the rest.
        let last = route[route.len() - 1];
        route
            .iter()
            .copied()
            .chain(std::iter::repeat(last))
            .take(self.channel_volumes.len())
            .collect()
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

/// A program sending audio that no MPRIS player accounts for.
///
/// PipeWire sees every stream; MPRIS sees only the programs that choose to
/// announce themselves. Mixxx, games, and anything using the audio API
/// directly are invisible on the bus, so a log that recorded only the bus
/// would call those evenings silent -- which is the one inference this log
/// exists to stop a reader from drawing.
///
/// Weaker evidence than a player, and kept apart from one for that reason:
/// it says a binary was emitting audio, not what it was playing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    /// `application.name`, as PipeWire reports it.
    pub app: String,
    /// The binary behind it, which survives a window title changing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// The process, which is what makes "already covered by MPRIS" an
    /// identity test rather than a guess at matching names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    /// `media.name`: often a file, a tab title, or the program again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// The stream's own gain, linear, as the application set it. Recorded
    /// and never folded into the effective gain: multiplying two stages is
    /// the mistake the sink's own volume already cost this log once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
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
    /// Programs seen emitting audio that no player on the bus accounts
    /// for. Apart from `players` on purpose: the two come from different
    /// sources and say different amounts, and a reader must be able to
    /// tell which one spoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<Stream>,
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

    /// Where a sink has a route, the route holds the volume — whether or not
    /// the node mirrors it.
    #[test]
    fn effective_gain_takes_the_route_over_the_node() {
        // A node left at 1.0 beside a route carrying the volume.
        let mixer = sink(vec![1.0, 1.0], Some(vec![0.125, 0.125]));
        assert_eq!(mixer.effective(), vec![0.125, 0.125]);
        assert_eq!(mixer.peak(), Some(0.125));

        // A sink with no route — pro audio, level set by a knob — keeps the
        // node's own value.
        let knob = sink(vec![0.4, 0.4], None);
        assert_eq!(knob.effective(), vec![0.4, 0.4]);
    }

    /// Node and route are one control seen twice, so a sink reporting the
    /// same figure on both is at that figure, not its square. Measured on a
    /// Bluetooth speaker whose own display read back the 20% that
    /// `cbrt(0.00858)` predicts; multiplying would have logged 4%.
    #[test]
    fn a_mirrored_node_and_route_do_not_compound() {
        let bluetooth = sink(vec![0.00858, 0.00858], Some(vec![0.00858, 0.00858]));
        assert_eq!(bluetooth.effective(), vec![0.00858, 0.00858]);
        assert_eq!(bluetooth.peak(), Some(0.00858));

        // The same on a built-in ALSA sink, which mirrors it too.
        let analog = sink(vec![0.216007, 0.216007], Some(vec![0.216007, 0.216007]));
        assert_eq!(analog.peak(), Some(0.216007));
    }

    /// A route reporting fewer channels than the node must not truncate the
    /// result or panic.
    #[test]
    fn a_shorter_route_extends_to_every_channel() {
        let four = sink(vec![1.0, 1.0, 1.0, 1.0], Some(vec![0.5]));
        assert_eq!(four.effective(), vec![0.5, 0.5, 0.5, 0.5]);
    }

    /// A route with more channels than the node is reported at the node's
    /// width, as the node is what carries the stream.
    #[test]
    fn a_longer_route_is_cut_to_the_nodes_channels() {
        let two = sink(vec![1.0, 1.0], Some(vec![0.5, 0.5, 0.5, 0.5]));
        assert_eq!(two.effective(), vec![0.5, 0.5]);
    }

    /// pw-dump can omit the node's Props entirely. The route still holds the
    /// volume, and dropping it would record no volume at all.
    #[test]
    fn a_route_survives_a_node_that_reported_no_channels() {
        let headless = sink(vec![], Some(vec![0.3, 0.3]));
        assert_eq!(headless.effective(), vec![0.3, 0.3]);
        assert_eq!(headless.peak(), Some(0.3));
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
