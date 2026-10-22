//! Output device and volume, sampled from PipeWire.
//!
//! Sampled on demand rather than streamed: only point-in-time sink state is
//! needed, so `pw-dump` and a JSON parse do the job and PipeWire bindings
//! would be cost without benefit.

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::event::{Bluetooth, Sink};

/// Reads the current default sink. `Ok(None)` means PipeWire reported no
/// default audio sink, which is a real state, not a failure.
pub fn sample() -> Result<Option<Sink>> {
    let output = Command::new("pw-dump")
        .output()
        .context("running pw-dump (is PipeWire installed and running?)")?;
    if !output.status.success() {
        anyhow::bail!("pw-dump exited with {}", output.status);
    }
    let objects: Vec<Value> = serde_json::from_slice(&output.stdout).context("parsing pw-dump")?;
    Ok(default_sink(&objects))
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
