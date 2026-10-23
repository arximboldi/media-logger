//! MPRIS player discovery and state.
//!
//! Watches the session bus directly rather than going through `playerctld`,
//! which exposes only the active player and would silently drop a second one
//! playing at the same time.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zbus::zvariant::{OwnedVariant, Variant};
use zbus::Connection;

use crate::event::{PlayerState, Track};

pub const PREFIX: &str = "org.mpris.MediaPlayer2.";
pub const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";

/// `org.mpris.MediaPlayer2.mpd` → `mpd`. The prefix is fixed, so the short
/// name is lossless and far easier to read in a log.
pub fn short_name(bus_name: &str) -> &str {
    bus_name.strip_prefix(PREFIX).unwrap_or(bus_name)
}

/// The process behind a bus name.
///
/// This is what makes "already accounted for on the bus" an identity test
/// rather than a guess at matching `org.mpris.MediaPlayer2.firefox
/// .instance_1_181` against an application PipeWire calls "Firefox".
/// `None` when the bus will not say, in which case the caller must decide
/// what to do with a stream it cannot place.
pub async fn pid_of(conn: &Connection, bus_name: &str) -> Option<u32> {
    let dbus = DBusProxy::new(conn).await.ok()?;
    let name = zbus::names::BusName::try_from(bus_name).ok()?;
    dbus.get_connection_unix_process_id(name).await.ok()
}

pub async fn connect() -> Result<Connection> {
    Connection::session()
        .await
        .context("connecting to the session bus")
}

/// Players already on the bus. Startup must enumerate as well as subscribe —
/// a player running before the daemon started emits no signal.
pub async fn list_players(conn: &Connection) -> Result<Vec<String>> {
    let dbus = DBusProxy::new(conn).await?;
    let names = dbus.list_names().await?;
    Ok(names
        .into_iter()
        .map(|n| n.as_str().to_owned())
        .filter(|n| n.starts_with(PREFIX))
        .collect())
}

async fn properties(conn: &Connection, bus_name: &str) -> Result<PropertiesProxy<'static>> {
    PropertiesProxy::builder(conn)
        .destination(bus_name.to_owned())
        .path(PATH)
        .build()
        .await
        .with_context(|| format!("building properties proxy for {bus_name}"))
}

pub async fn identity(conn: &Connection, bus_name: &str) -> Result<Option<String>> {
    let proxy = properties(conn, bus_name).await?;
    let iface = InterfaceName::try_from(ROOT_IFACE)?;
    let all = proxy.get_all(iface).await?;
    Ok(all.get("Identity").and_then(as_string))
}

/// Reads the player's full state in one call.
///
/// Reading everything on every change, rather than applying the delta carried
/// by the signal, means each logged line is a complete picture and no partial
/// update can desynchronise the record. At human event rates the extra round
/// trip costs nothing.
pub async fn snapshot(
    conn: &Connection,
    bus_name: &str,
    identity: Option<String>,
) -> Result<PlayerState> {
    let proxy = properties(conn, bus_name).await?;
    let iface = InterfaceName::try_from(PLAYER_IFACE)?;
    let all = proxy
        .get_all(iface)
        .await
        .with_context(|| format!("reading player state from {bus_name}"))?;

    Ok(PlayerState {
        player: short_name(bus_name).to_owned(),
        identity,
        status: all
            .get("PlaybackStatus")
            .and_then(as_string)
            .unwrap_or_else(|| "Unknown".to_owned()),
        player_volume: all.get("Volume").and_then(as_f64),
        // Position is excluded from PropertiesChanged by the specification,
        // so it only ever arrives through an explicit read like this one.
        position_us: all.get("Position").and_then(as_i64),
        track: track_from(&all),
    })
}

/// Subscribes to property changes for one player.
pub async fn changes(
    conn: &Connection,
    bus_name: &str,
) -> Result<zbus::fdo::PropertiesChangedStream> {
    let proxy = properties(conn, bus_name).await?;
    proxy
        .receive_properties_changed()
        .await
        .with_context(|| format!("subscribing to {bus_name}"))
}

pub async fn name_changes(conn: &Connection) -> Result<zbus::fdo::NameOwnerChangedStream> {
    let dbus = DBusProxy::new(conn).await?;
    dbus.receive_name_owner_changed()
        .await
        .context("subscribing to NameOwnerChanged")
}

fn track_from(all: &HashMap<String, OwnedVariant>) -> Track {
    let Some(metadata) = all.get("Metadata") else {
        return Track::default();
    };
    let Value::Object(map) = to_json(metadata) else {
        return Track::default();
    };

    let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_owned);

    // Keep everything the player exposed, so metadata that becomes
    // interesting later is already in the record. artUrl is dropped: a local
    // cache path, meaningless off this machine and noise on every line.
    let mut retained: Map<String, Value> = map.clone();
    retained.remove("mpris:artUrl");

    Track {
        title: text("xesam:title"),
        artist: map
            .get("xesam:artist")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        album: text("xesam:album"),
        url: text("xesam:url"),
        trackid: text("mpris:trackid"),
        length_us: map.get("mpris:length").and_then(Value::as_i64),
        metadata: (!retained.is_empty()).then_some(Value::Object(retained)),
    }
}

fn as_string(value: &OwnedVariant) -> Option<String> {
    match to_json(value) {
        Value::String(s) => Some(s),
        _ => None,
    }
}

fn as_f64(value: &OwnedVariant) -> Option<f64> {
    to_json(value).as_f64()
}

fn as_i64(value: &OwnedVariant) -> Option<i64> {
    to_json(value).as_i64()
}

/// Converts a D-Bus variant to JSON.
///
/// MPRIS metadata is `a{sv}` holding a mix of strings, object paths, integer
/// widths and string arrays, so the conversion has to be dynamic.
fn to_json(value: &Variant<'_>) -> Value {
    match value {
        Variant::Bool(b) => json!(b),
        Variant::U8(n) => json!(n),
        Variant::I16(n) => json!(n),
        Variant::U16(n) => json!(n),
        Variant::I32(n) => json!(n),
        Variant::U32(n) => json!(n),
        Variant::I64(n) => json!(n),
        Variant::U64(n) => json!(n),
        // serde_json cannot represent NaN or infinity; null is honest.
        Variant::F64(n) => {
            if n.is_finite() {
                json!(n)
            } else {
                Value::Null
            }
        }
        Variant::Str(s) => json!(s.as_str()),
        Variant::ObjectPath(p) => json!(p.as_str()),
        Variant::Signature(s) => json!(s.to_string()),
        Variant::Variant(inner) => to_json(inner),
        Variant::Array(array) => Value::Array(array.inner().iter().map(to_json).collect()),
        Variant::Dict(dict) => Value::Object(
            dict.iter()
                .map(|(k, v)| {
                    let key = match to_json(k) {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (key, to_json(v))
                })
                .collect(),
        ),
        Variant::Structure(s) => Value::Array(s.fields().iter().map(to_json).collect()),
        other => json!(format!("{other:?}")),
    }
}
