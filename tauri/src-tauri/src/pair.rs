//! Desktop side of the bluetooth-style pairing handshake. The high-level
//! orchestration lives in `main.rs` (Tauri commands); this module just owns
//! the wire dance: connect → PAIR_REQUEST → receive PAIR_CHALLENGE → wait
//! for user confirm → PAIR_CONFIRM → receive PAIR_OK.
//!
//! The "wait for user confirm" step uses a oneshot channel whose sender is
//! stored in app state and triggered by the `pair_confirm`/`pair_cancel`
//! Tauri commands.

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use musicsync_core::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::Message;

pub struct PairOutcome {
    pub token: String,
    pub device_name: String,
    pub music_root: String,
}

/// Run the handshake to completion. The `notify_challenge` callback fires
/// once the phone has sent its PAIR_CHALLENGE — that's the cue for the UI
/// to show the comparison code modal. The future returned by `await_user`
/// resolves with `true` if the user confirmed locally, `false` if cancelled.
pub async fn run_pairing<F>(
    ws_url: &str,
    notify_challenge: impl FnOnce(&str, &str),
    await_user: F,
) -> Result<PairOutcome>
where
    F: std::future::Future<Output = bool>,
{
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .with_context(|| format!("connecting to {ws_url}"))?;
    let (mut sink, mut stream) = ws_stream.split();

    // PAIR_REQUEST → server expected to respond with PAIR_CHALLENGE.
    // Include OS user + LAN IP so the phone's confirm dialog can show
    // "from andrew@192.168.0.42" alongside the 6-digit code.
    let (desktop_user, desktop_host) = desktop_identity();
    send_text(
        &mut sink,
        &ClientMessage::PairRequest {
            protocol_version: PROTOCOL_VERSION,
            desktop_user,
            desktop_host,
        },
    )
    .await?;
    let challenge = recv_text(&mut stream).await?;
    let (code, _challenge_device) = match challenge {
        ServerMessage::PairChallenge { code, device_name } => (code, device_name),
        ServerMessage::Error { message } => return Err(anyhow!("phone error: {message}")),
        other => return Err(anyhow!("unexpected response to PAIR_REQUEST: {other:?}")),
    };
    notify_challenge(&code, &_challenge_device);

    // Wait for the user to press Confirm or Cancel on the desktop UI.
    let confirmed = await_user.await;
    send_text(
        &mut sink,
        &if confirmed { ClientMessage::PairConfirm } else { ClientMessage::PairCancel },
    )
    .await?;
    if !confirmed {
        return Err(anyhow!("cancelled by user on desktop"));
    }

    // Wait for the phone's verdict. The phone only sends PAIR_OK once *its*
    // user has also tapped Confirm; until then we are blocked here (with the
    // server-side 60s timeout as a safety net).
    let final_msg = recv_text(&mut stream).await?;
    match final_msg {
        ServerMessage::PairOk { token, device_name, music_root } => {
            Ok(PairOutcome { token, device_name, music_root })
        }
        ServerMessage::PairCancelled { reason } => {
            Err(anyhow!("phone cancelled: {reason}"))
        }
        ServerMessage::Error { message } => Err(anyhow!("phone error: {message}")),
        other => Err(anyhow!("unexpected response after PAIR_CONFIRM: {other:?}")),
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn send_text(sink: &mut WsSink, msg: &ClientMessage) -> Result<()> {
    let s = serde_json::to_string(msg)?;
    sink.send(Message::Text(s.into())).await?;
    Ok(())
}

/// `(user, host)` strings used as this desktop's self-identification in
/// PAIR_REQUEST and HELLO. Empty strings on platforms that don't expose
/// the value — the phone treats those as "unknown".
pub fn desktop_identity() -> (String, String) {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let host = best_local_ip().unwrap_or_default();
    (user, host)
}

/// Best-effort first non-loopback IPv4 string on this host. Used purely
/// for the desktop's self-label in pair requests.
pub(crate) fn best_local_ip() -> Option<String> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    for a in addrs {
        if a.is_loopback() { continue; }
        if let std::net::IpAddr::V4(v4) = a.ip() {
            if v4.octets()[0] == 169 && v4.octets()[1] == 254 { continue; }
            return Some(v4.to_string());
        }
    }
    None
}

async fn recv_text(stream: &mut WsStream) -> Result<ServerMessage> {
    loop {
        let frame = stream
            .next()
            .await
            .ok_or_else(|| anyhow!("connection closed"))??;
        match frame {
            Message::Text(t) => return Ok(serde_json::from_str(&t)?),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(_) => return Err(anyhow!("unexpected binary frame during pair")),
            Message::Close(_) => return Err(anyhow!("connection closed by server")),
        }
    }
}
