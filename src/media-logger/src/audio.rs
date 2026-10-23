//! Output device and volume, sampled from PipeWire.
//!
//! Sampled on demand rather than streamed: only point-in-time sink state is
//! needed, so `pw-dump` and a JSON parse do the job and PipeWire bindings
//! would be cost without benefit.

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::event::{Bluetooth, Sink, Stream};

/// One look at the audio graph: where sound is going, and what is sending
/// it. Both come from the same `pw-dump`, which was already being read in
/// full and thrown away but for the sink.
pub struct Sampled {
    /// `None` means PipeWire reported no default audio sink, which is a
    /// real state, not a failure.
    pub sink: Option<Sink>,
    pub streams: Vec<Stream>,
}

pub fn sample() -> Result<Sampled> {
    let output = Command::new("pw-dump")
        .output()
        .context("running pw-dump (is PipeWire installed and running?)")?;
    if !output.status.success() {
        anyhow::bail!("pw-dump exited with {}", output.status);
    }
    let objects: Vec<Value> = serde_json::from_slice(&output.stdout).context("parsing pw-dump")?;
    Ok(Sampled {
        sink: default_sink(&objects),
        streams: streams(&objects),
    })
}

/// Every program sending audio right now.
///
/// Running ones only. A node stays in the graph while its program is open
/// and silent, so recording an idle one would say Mixxx was playing
/// whenever Mixxx happened to be running -- the mirror of the omission
/// this exists to fix.
fn streams(objects: &[Value]) -> Vec<Stream> {
    objects
        .iter()
        .filter_map(|o| parse_stream(o, objects))
        .collect()
}

fn parse_stream(object: &Value, objects: &[Value]) -> Option<Stream> {
    let info = object.get("info")?;
    let props = info.get("props")?;
    if !props
        .get("media.class")?
        .as_str()?
        .starts_with("Stream/Output/Audio")
    {
        return None;
    }
    if info.get("state")?.as_str()? != "running" {
        return None;
    }
    // The node names the stream; the client behind it names the process.
    // Only the client carries a pid, and the pid is what tells a stream
    // already covered by a player from one nothing on the bus accounts for.
    let client = props
        .get("client.id")
        .and_then(Value::as_u64)
        .and_then(|id| {
            objects
                .iter()
                .find(|c| c.get("id").and_then(Value::as_u64) == Some(id))
        })
        .and_then(|c| c.get("info")?.get("props").cloned());
    let client_prop =
        |key: &str| -> Option<String> { Some(client.as_ref()?.get(key)?.as_str()?.to_owned()) };
    let app = client_prop("application.name")
        .or_else(|| props.get("application.name")?.as_str().map(str::to_owned))
        .or_else(|| props.get("node.name")?.as_str().map(str::to_owned))?;
    Some(Stream {
        app,
        binary: client_prop("application.process.binary"),
        pid: client
            .as_ref()
            .and_then(|c| c.get("application.process.id"))
            .and_then(Value::as_i64),
        media: props
            .get("media.name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        volume: info
            .get("params")
            .and_then(|p| p.get("Props"))
            .and_then(|p| p.get(0))
            .and_then(|p| p.get("channelVolumes"))
            .and_then(Value::as_array)
            .and_then(|v| v.iter().filter_map(Value::as_f64).max_by(f64::total_cmp)),
    })
}

fn default_sink(objects: &[Value]) -> Option<Sink> {
    let name = default_sink_name(objects)?;
    objects.iter().find_map(|o| parse_sink(o, &name, objects))
}

/// The output route's gain for a sink's device.
///
/// This is where the volume actually lives: an ALSA mixer's setting, or on
/// Bluetooth the AVRCP level the speaker applies. The node usually mirrors
/// it, but not always — some sinks leave the node at 1.0 — so the route is
/// read rather than inferred from the node.
fn route_volumes(objects: &[Value], device_id: Option<u64>) -> Option<Vec<f64>> {
    let device_id = device_id?;
    for object in objects {
        if object.get("id").and_then(Value::as_u64) != Some(device_id) {
            continue;
        }
        let routes = object
            .get("info")?
            .get("params")?
            .get("Route")?
            .as_array()?;
        for route in routes {
            if route.get("direction").and_then(Value::as_str) != Some("Output") {
                continue;
            }
            if let Some(v) = route
                .get("props")
                .and_then(|p| p.get("channelVolumes"))
                .and_then(Value::as_array)
            {
                return Some(v.iter().filter_map(Value::as_f64).collect());
            }
        }
    }
    None
}

/// PipeWire names the default sink indirectly, through a Metadata object
/// rather than a flag on the node.
fn default_sink_name(objects: &[Value]) -> Option<String> {
    for object in objects {
        if object.get("type")?.as_str()? != "PipeWire:Interface:Metadata" {
            continue;
        }
        for entry in object.get("metadata")?.as_array()? {
            if entry.get("key").and_then(Value::as_str) != Some("default.audio.sink") {
                continue;
            }
            // The value arrives as {"name": "..."} but older versions encode
            // the same object as a JSON string.
            let value = entry.get("value")?;
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                return Some(name.to_owned());
            }
            if let Some(raw) = value.as_str() {
                if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                    if let Some(name) = parsed.get("name").and_then(Value::as_str) {
                        return Some(name.to_owned());
                    }
                }
            }
        }
    }
    None
}

fn parse_sink(object: &Value, want: &str, objects: &[Value]) -> Option<Sink> {
    let info = object.get("info")?;
    let props = info.get("props")?;
    if props.get("media.class")?.as_str()? != "Audio/Sink" {
        return None;
    }
    if props.get("node.name")?.as_str()? != want {
        return None;
    }

    let params = info
        .get("params")
        .and_then(|p| p.get("Props"))
        .and_then(Value::as_array);
    let first = params.and_then(|p| p.first());

    // Linear gain; every UI shows its cube root, so the two agree only at
    // unity. Store the raw value and name the scale.
    let channel_volumes = first
        .and_then(|p| p.get("channelVolumes"))
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let mute = first
        .and_then(|p| p.get("mute"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let text = |key: &str| props.get(key).and_then(Value::as_str).map(str::to_owned);
    let device_api = text("device.api");

    // On Bluetooth the same volume figure means something quite different per
    // profile, so reproduction needs the profile as much as the level.
    let bluetooth = (device_api.as_deref() == Some("bluez5")).then(|| Bluetooth {
        address: text("api.bluez5.address"),
        profile: text("api.bluez5.profile").or_else(|| text("device.profile.name")),
        codec: text("api.bluez5.codec"),
    });

    let route = route_volumes(objects, props.get("device.id").and_then(Value::as_u64));

    Some(Sink {
        name: want.to_owned(),
        description: text("node.description"),
        device_api,
        channel_volumes,
        route_volumes: route,
        volume_scale: "linear".to_owned(),
        mute,
        bluetooth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped on a real `pw-dump`: the node names the stream, and only the
    /// client behind it carries the process.
    fn graph() -> Vec<Value> {
        serde_json::from_str(
            r#"[
          {"id": 78,
           "info": {"props": {"application.name": "Mixxx",
                              "application.process.binary": "mixxx",
                              "application.process.id": 132637}}},
          {"id": 90,
           "info": {"state": "running",
                    "props": {"media.class": "Stream/Output/Audio",
                              "node.name": "Mixxx",
                              "media.name": "main out",
                              "client.id": 78},
                    "params": {"Props": [{"channelVolumes": [0.5, 0.857375]}]}}},
          {"id": 91,
           "info": {"state": "idle",
                    "props": {"media.class": "Stream/Output/Audio",
                              "node.name": "open but quiet",
                              "client.id": 78}}},
          {"id": 92,
           "info": {"state": "running",
                    "props": {"media.class": "Stream/Input/Audio",
                              "node.name": "a microphone",
                              "client.id": 78}}}
        ]"#,
        )
        .expect("the fixture is valid json")
    }

    /// The stream record exists so an evening of Mixxx is not read as
    /// silence, and it has to name the process: that is what tells a
    /// stream already accounted for on the bus from one that is not.
    #[test]
    fn a_running_stream_is_named_by_the_client_behind_it() {
        let found = streams(&graph());
        assert_eq!(found.len(), 1, "{found:?}");
        let s = &found[0];
        assert_eq!(s.app, "Mixxx");
        assert_eq!(s.binary.as_deref(), Some("mixxx"));
        assert_eq!(s.pid, Some(132637));
        assert_eq!(s.media.as_deref(), Some("main out"));
        // The loudest channel, as the sink's own reading is taken.
        assert_eq!(s.volume, Some(0.857375));
    }

    /// A node stays in the graph while its program is open and silent.
    /// Recording one would say Mixxx was playing whenever Mixxx was
    /// running, which is the mirror of the omission this exists to fix.
    #[test]
    fn an_idle_stream_is_not_playback() {
        assert!(!streams(&graph()).iter().any(|s| s.app == "open but quiet"));
    }

    /// Only what leaves the machine. A microphone is a stream too.
    #[test]
    fn an_input_stream_is_not_output() {
        assert!(!streams(&graph()).iter().any(|s| s.app == "a microphone"));
    }
}
