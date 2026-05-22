//! mDNS discovery. The Android companion app registers
//! `_musicsync._tcp.local.` via NsdManager; this module browses for it from
//! the desktop and reports each resolved service back to the UI.
//!
//! Browse runs for the lifetime of the app — there is no auto-stop. The
//! frontend may switch into manual-entry mode at any time via a button,
//! but the daemon keeps running in the background so a phone coming online
//! later will still be discovered. mDNS is low-traffic, so this is cheap.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use musicsync_core::protocol::{DEFAULT_PORT, MDNS_SERVICE_TYPE};

#[derive(Serialize, Clone)]
pub struct DiscoveryFoundEvent {
    pub ws_url: String,
    /// Stable per-phone UUID. Empty string when the responder is an old
    /// companion that doesn't advertise `id` yet — frontend treats that
    /// as "match by name only" for backwards compat.
    #[serde(default)]
    pub device_id: String,
    pub device_name: String,
    pub host: String,
    pub port: u16,
}

const DISCOVERY_PORT: u16 = 7799;
const PROBE_PREAMBLE: &str = "MUSICSYNC_DISCOVER";
const REPLY_PREAMBLE: &str = "MUSICSYNC_HERE";

/// UDP-broadcast discovery. Sends one "MUSICSYNC_DISCOVER" packet to the
/// IPv4 broadcast address(es) on every local interface, listens for ~3
/// seconds, and emits `discovery_found` for each replier. Much faster
/// than per-IP TCP scanning and dodges the mDNS-multicast filtering some
/// routers do. The phone's [DiscoveryResponder] is on the other end.
pub fn start_lan_scan(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let local_v4s: Vec<Ipv4Addr> = match if_addrs::get_if_addrs() {
            Ok(addrs) => addrs
                .into_iter()
                .filter_map(|iface| {
                    if iface.is_loopback() { return None; }
                    if let IpAddr::V4(v4) = iface.ip() {
                        if v4.octets()[0] == 169 && v4.octets()[1] == 254 { return None; }
                        Some(v4)
                    } else { None }
                })
                .collect(),
            Err(e) => { tracing::warn!("if_addrs failed: {e}"); return; }
        };
        if local_v4s.is_empty() {
            tracing::warn!("no usable IPv4 interface; UDP discovery skipped");
            return;
        }

        let sock = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => { tracing::warn!("udp bind failed: {e}"); return; }
        };
        if let Err(e) = sock.set_broadcast(true) {
            tracing::warn!("set_broadcast failed: {e}");
            return;
        }

        // Three layers of probe targets, sent in parallel:
        //   1. 255.255.255.255  — global broadcast
        //   2. <subnet>.255     — directed /24 broadcast per interface
        //   3. every host in each /24 (unicast)
        // Some APs silently drop layer 1+2 (multicast-to-unicast
        // conversion, Wi-Fi broadcast filtering for power saving).
        // Layer 3 unicast UDP almost always survives — ~254 × ~25 bytes
        // per interface is negligible network cost.
        let probe = format!("{PROBE_PREAMBLE}\n");
        let probe_bytes = probe.as_bytes();

        let mut bcast_targets = std::collections::HashSet::<Ipv4Addr>::new();
        bcast_targets.insert(Ipv4Addr::BROADCAST);
        for v4 in &local_v4s {
            let o = v4.octets();
            bcast_targets.insert(Ipv4Addr::new(o[0], o[1], o[2], 255));
        }
        for tgt in &bcast_targets {
            let sa = SocketAddr::new(IpAddr::V4(*tgt), DISCOVERY_PORT);
            let _ = sock.send_to(probe_bytes, sa).await;
        }
        for v4 in &local_v4s {
            let o = v4.octets();
            for last in 1u8..=254 {
                let host = Ipv4Addr::new(o[0], o[1], o[2], last);
                if host == *v4 { continue; }
                let sa = SocketAddr::new(IpAddr::V4(host), DISCOVERY_PORT);
                let _ = sock.send_to(probe_bytes, sa).await;
            }
        }

        // Listen for replies for a few seconds. Phones on the LAN should
        // reply within milliseconds; we hold the window open longer to
        // catch sleeping devices that take a moment to respond.
        let mut buf = vec![0u8; 4096];
        let mut seen: std::collections::HashSet<SocketAddr> = std::collections::HashSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline { break; }
            let remaining = deadline - now;
            let r = tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await;
            let (n, from) = match r {
                Ok(Ok(v)) => v,
                _ => break,
            };
            if !seen.insert(from) { continue; }
            let payload = std::str::from_utf8(&buf[..n]).unwrap_or("");
            let trimmed = payload.trim();
            if !trimmed.starts_with(REPLY_PREAMBLE) { continue; }
            // The payload after the preamble is JSON; parse defensively.
            let json_start = trimmed.find('{').unwrap_or(trimmed.len());
            let json_str = &trimmed[json_start..];
            let parsed: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let device_name = parsed.get("name").and_then(|v| v.as_str())
                .unwrap_or("(unknown)").to_string();
            let device_id = parsed.get("id").and_then(|v| v.as_str())
                .unwrap_or("").to_string();
            let port = parsed.get("port").and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_PORT as u64) as u16;
            let host = from.ip().to_string();
            let ws_url = format!("ws://{host}:{port}");
            let _ = app.emit("discovery_found", DiscoveryFoundEvent {
                ws_url, device_id, device_name, host, port,
            });
        }
        tracing::info!("UDP discovery done");
    });
}

/// Spawn a background task that browses for the MusicSync service forever
/// (until the process exits). Emits `discovery_found` on each unique
/// service resolved. There is no timeout — the user's "Enter manually"
/// button on the frontend swaps the UI state without affecting this task.
pub fn start_browse(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("mdns daemon failed to start: {e}");
                return;
            }
        };
        let recv = match daemon.browse(MDNS_SERVICE_TYPE) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("mdns browse failed: {e}");
                return;
            }
        };

        let mut seen = std::collections::HashSet::new();
        // Loop until the receiver errors (daemon shutdown), then exit.
        // No deadline — the daemon keeps the network browse alive.
        while let Ok(event) = recv.recv_async().await {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let host = info
                        .get_addresses()
                        .iter()
                        .find(|a| a.is_ipv4())
                        .map(|a| a.to_string())
                        .or_else(|| info.get_addresses().iter().next().map(|a| a.to_string()));
                    let Some(host) = host else { continue };
                    let port = info.get_port();
                    let fp = info.get_fullname().to_string();
                    if !seen.insert(fp.clone()) { continue; }
                    let device_name = info
                        .get_property_val_str("name")
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            info.get_hostname().trim_end_matches('.').to_string()
                        });
                    let device_id = info
                        .get_property_val_str("id")
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let ws_url = format!("ws://{host}:{port}");
                    let _ = app.emit(
                        "discovery_found",
                        DiscoveryFoundEvent { ws_url, device_id, device_name, host, port },
                    );
                }
                _ => continue, // other event types — ignore
            }
        }
    });
}
