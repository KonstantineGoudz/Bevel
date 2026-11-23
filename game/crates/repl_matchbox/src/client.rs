//! Client half of the matchbox replicon backend: connect the socket, run the
//! postcard handshake, drive `ClientState`, pump channels and publish session
//! state for the game layer.

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use postcard::{from_bytes, to_stdvec};

use crate::transport::{channel_configs, ByteWindow, ClientLinkState, ClientTransport, MatchboxClient};
use crate::{
    BackendIceServer, BackendState, BackendStatus, BackendTraffic, ClientBackendConfig, Handshake,
    ReconnectRequest, SessionDenied, SessionInfo, SessionStarted, CONTROL_CHANNEL,
};

/// Where the client is in the backend handshake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ClientPhase {
    /// Waiting for the WebRTC link (or a fresh link after a drop).
    #[default]
    Idle,
    /// `Hello` sent, waiting for `Welcome`/`Deny`.
    Awaiting,
    /// Authenticated, replicon traffic flows.
    Connected,
    /// Server denied us; do not retry with the same credentials.
    Denied,
}

/// How long a link may sit in "connecting"/"handshake" without reaching the
/// host before the backend throws it away and starts a fresh signaling/WebRTC
/// attempt. A healthy LAN join takes well under a second, but mobile data +
/// TURN can take tens of seconds (ICE through a relay, several candidate
/// pairs), so this must be generous — cutting early restarts the negotiation
/// and can loop forever without ever succeeding. A permanently stuck
/// half-handshake (or a socket that died while the tab was suspended) still
/// recovers, just after a full attempt window.
const LINK_WATCHDOG_SECS: f64 = 40.0;
/// Retry backoff: 1 s, 2 s, 4 s, … capped at 30 s.
const RETRY_BASE_SECS: f64 = 1.0;
const RETRY_MAX_SECS: f64 = 30.0;

fn retry_delay(attempts: u32) -> f64 {
    let exp = attempts.saturating_sub(1).min(16) as i32;
    (RETRY_BASE_SECS * 2f64.powi(exp)).min(RETRY_MAX_SECS)
}

/// How often the client probes the control channel for an RTT sample.
const PING_INTERVAL_SECS: f64 = 1.0;

/// Latency probe state (matchbox/SCTP has no RTT API).
#[derive(Default)]
struct PingState {
    /// When the next probe is due (app time); `None` until authenticated.
    next_at: Option<f64>,
    nonce: u64,
    /// `(nonce, sent_at)` of the outstanding probe.
    pending: Option<(u64, f64)>,
    /// Last measured round trip in seconds.
    rtt: f64,
}

/// Byte counters + ping state feeding replicon's `ClientStats`.
#[derive(Resource, Default)]
struct ClientMetrics {
    bytes: ByteWindow,
    ping: PingState,
}

#[derive(Resource, Default)]
struct ClientHandshake {
    phase: ClientPhase,
    /// The config the current socket was created from (`None` = no socket has
    /// been created yet). Used to detect a real config change, as opposed to
    /// the first insert or a `Res::is_changed` flag.
    last_config: Option<ClientBackendConfig>,
    /// When the current socket/phase started (`elapsed_secs_f64`, 0 = unset).
    since: f64,
    /// Consecutive failed/stalled link attempts, for the retry backoff.
    attempts: u32,
    /// A retry is scheduled at this time; no socket is created before it.
    retry_at: Option<f64>,
}

pub(crate) fn add(app: &mut App) {
    app.init_resource::<ClientHandshake>()
        .init_resource::<BackendStatus>()
        .init_resource::<ClientMetrics>()
        .init_resource::<BackendTraffic>()
        .add_message::<SessionStarted>()
        .add_message::<SessionDenied>()
        .add_message::<ReconnectRequest>()
        .add_systems(
            PreUpdate,
            (
                reset_on_reconnect,
                init_transport
                    .run_if(|transport: Option<Res<ClientTransport>>| transport.is_none()),
                pump,
                flush_client_stats,
            )
                .chain()
                .in_set(ClientSystems::ReceivePackets),
        )
        .add_systems(
            PostUpdate,
            send_packets.in_set(ClientSystems::SendPackets),
        );
}

/// Drop the socket on request, or automatically when the game replaced the
/// [`ClientBackendConfig`] (new token / URL). The next tick's `init_transport`
/// re-creates it from the current config and re-runs the handshake.
#[allow(clippy::too_many_arguments)]
fn reset_on_reconnect(
    mut commands: Commands,
    config: Option<Res<ClientBackendConfig>>,
    transport: Option<ResMut<ClientTransport>>,
    mut requests: MessageReader<ReconnectRequest>,
    mut handshake: ResMut<ClientHandshake>,
    mut metrics: ResMut<ClientMetrics>,
    mut stats: ResMut<ClientStats>,
    mut traffic: ResMut<BackendTraffic>,
    mut status: ResMut<BackendStatus>,
) {
    let requested = requests.read().count() > 0;
    let current = config.as_deref().cloned();
    // Compare with the config the socket was built from: the first insert is
    // not a "change" (init_transport picks it up right after this system), and
    // `Res::is_changed` cannot tell the difference.
    let config_changed = match (&handshake.last_config, &current) {
        (Some(previous), Some(current)) => previous != current,
        (Some(_), None) => true,
        // First config: applied by `init_transport` below.
        (None, _) => false,
    };
    if !requested && !config_changed {
        if handshake.last_config.is_none() {
            handshake.last_config = current;
        }
        return;
    }
    handshake.last_config = current;
    metrics.bytes.reset();
    metrics.ping = PingState::default();
    *stats = ClientStats::default();
    *traffic = BackendTraffic::default();
    let Some(mut transport) = transport else {
        return;
    };
    log::info!(
        "repl backend: reconnecting (config changed: {config_changed}, requested: {requested})"
    );
    transport.close();
    commands.remove_resource::<ClientTransport>();
    handshake.phase = ClientPhase::Idle;
    handshake.since = 0.0;
    handshake.attempts = 0;
    handshake.retry_at = None;
    *status = BackendStatus::default();
}

/// Discard a dead/stalled link and schedule a fresh socket after the backoff.
fn schedule_retry(
    commands: &mut Commands,
    handshake: &mut ClientHandshake,
    status: &mut BackendStatus,
    now: Option<f64>,
    link: &ClientLinkState,
) {
    let Some(now) = now else {
        return;
    };
    handshake.attempts = handshake.attempts.saturating_add(1);
    let delay = retry_delay(handshake.attempts);
    commands.remove_resource::<ClientTransport>();
    handshake.phase = ClientPhase::Idle;
    handshake.since = 0.0;
    handshake.retry_at = Some(now + delay);
    status.state = BackendState::WaitingForHost;
    status.detail = match link {
        ClientLinkState::Closed(detail) => format!(
            "link closed ({detail}); reconnecting in {delay:.0}s (attempt {})",
            handshake.attempts
        ),
        _ => format!(
            "link watchdog: retrying in {delay:.0}s (attempt {})",
            handshake.attempts
        ),
    };
    log::warn!("repl backend: {}", status.detail);
}

/// Create the matchbox client socket once a config exists (the game inserts it
/// after login) and the game registered the replication schema, so
/// `RepliconChannels` is complete.
fn init_transport(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    config: Option<Res<ClientBackendConfig>>,
    ice: Option<Res<BackendIceServer>>,
    mut handshake: ResMut<ClientHandshake>,
    time: Option<Res<Time>>,
) {
    let Some(config) = config.as_deref().cloned() else {
        return; // login has not produced a token yet
    };
    let now = time.as_deref().map(|time| time.elapsed_secs_f64());
    if let (Some(retry_at), Some(now)) = (handshake.retry_at, now) {
        if now < retry_at {
            // Backoff between attempts: keep the old transport gone.
            return;
        }
        handshake.retry_at = None;
    }
    if let Some(now) = now {
        handshake.since = now;
    }
    handshake.phase = ClientPhase::Idle;
    let configs = channel_configs(&channels);
    let client = MatchboxClient::new(
        &config.signaling_url,
        configs,
        ice.as_ref().map(|ice| ice.0.clone()),
    );
    log::info!(
        "repl backend: connecting matchbox room {}",
        config.signaling_url
    );
    commands.insert_resource(ClientTransport::Matchbox(client));
}

/// Set [`NextState`] only when the value actually changes.
///
/// Bevy 0.20 supports *identity* state transitions: setting `NextState` to the
/// current state still runs `OnExit`/`OnEnter`. Replicon resets its client
/// state on `OnExit(ClientState::Connected)` (clearing queued client messages
/// and the message cursors), so re-setting `Connected` every frame would drop
/// every outgoing client message.
fn set_client_state(
    current: Option<&State<ClientState>>,
    next: &mut NextState<ClientState>,
    target: ClientState,
) {
    if current.is_none_or(|state| *state.get() != target) {
        next.set(target);
    }
}

/// One tick of client transport work: link state, handshake, channel pump and
/// the [`ClientState`] transitions replicon gates its systems on.
#[allow(clippy::too_many_arguments)]
fn pump(
    transport: Option<ResMut<ClientTransport>>,
    mut handshake: ResMut<ClientHandshake>,
    config: Option<Res<ClientBackendConfig>>,
    protocol: Res<ProtocolHash>,
    time: Option<Res<Time>>,
    mut metrics: ResMut<ClientMetrics>,
    mut messages: ResMut<ClientMessages>,
    mut status: ResMut<BackendStatus>,
    current_state: Option<Res<State<ClientState>>>,
    mut state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
    mut started: MessageWriter<SessionStarted>,
    mut denied: MessageWriter<SessionDenied>,
) {
    let now = time.as_deref().map(|time| time.elapsed_secs_f64());

    // A scheduled retry still in its backoff window: keep the old socket gone
    // and report progress so the HUD does not look stuck forever.
    if let (Some(retry_at), Some(now)) = (handshake.retry_at, now) {
        if now < retry_at && handshake.phase != ClientPhase::Denied {
            status.state = BackendState::WaitingForHost;
            status.detail = format!(
                "reconnecting in {:.0}s (attempt {})",
                (retry_at - now).ceil(),
                handshake.attempts + 1
            );
            set_client_state(current_state.as_deref(), &mut state, ClientState::Connecting);
            return;
        }
    }

    let Some(mut transport) = transport else {
        // No socket yet: waiting for login config, or a retry whose backoff
        // has just elapsed (`init_transport` re-creates it this same frame).
        status.state = BackendState::Idle;
        status.detail.clear();
        set_client_state(current_state.as_deref(), &mut state, ClientState::Disconnected);
        return;
    };
    let link = transport.poll();
    status.relayed = transport.relayed().unwrap_or(false);
    if let Some(now) = now {
        if handshake.since == 0.0 {
            handshake.since = now;
        }
    }

    // Watchdog: a link that never reaches `Connected`, or one that closed
    // unexpectedly, is discarded and retried with backoff. This recovers
    // mobile clients after a suspended tab, a network switch, or a half-dead
    // WebRTC handshake instead of leaving the page waiting forever.
    let stalled = handshake.phase != ClientPhase::Denied
        && handshake.phase != ClientPhase::Connected
        && now.is_some_and(|now| now - handshake.since > LINK_WATCHDOG_SECS);
    let dead_link = matches!(link, ClientLinkState::Closed(_))
        && handshake.phase != ClientPhase::Denied;
    if stalled || dead_link {
        schedule_retry(&mut commands, &mut handshake, &mut status, now, &link);
        return;
    }

    match &link {
        ClientLinkState::Off => {
            status.state = BackendState::Idle;
            set_client_state(current_state.as_deref(), &mut state, ClientState::Disconnected);
        }
        ClientLinkState::Connecting => {
            if handshake.phase != ClientPhase::Connected {
                status.state = BackendState::WaitingForHost;
                let elapsed = now.map(|now| (now - handshake.since).max(0.0).ceil() as u64);
                status.detail = match elapsed {
                    Some(secs) => format!(
                        "waiting for the host peer ({}s, attempt {})",
                        secs,
                        handshake.attempts + 1
                    ),
                    None => "waiting for the host peer".to_string(),
                };
                set_client_state(current_state.as_deref(), &mut state, ClientState::Connecting);
            }
        }
        ClientLinkState::Up => match handshake.phase {
            ClientPhase::Idle => {
                // New link: greet the host once.
                let token = config
                    .as_deref()
                    .map(|config| config.token.clone())
                    .unwrap_or_default();
                let hello = Handshake::Hello {
                    token,
                    protocol: *protocol,
                };
                match to_stdvec(&hello) {
                    Ok(bytes) => {
                        metrics.bytes.add_sent(bytes.len() as u64);
                        transport.send(CONTROL_CHANNEL, bytes);
                        handshake.phase = ClientPhase::Awaiting;
                        status.state = BackendState::Handshaking;
                        status.detail = "handshake sent".to_string();
                        set_client_state(current_state.as_deref(), &mut state, ClientState::Connecting);
                    }
                    Err(err) => {
                        status.state = BackendState::Failed;
                        status.detail = format!("hello encode failed: {err}");
                    }
                }
            }
            ClientPhase::Awaiting => {
                status.state = BackendState::Handshaking;
                let elapsed = now.map(|now| (now - handshake.since).max(0.0).ceil() as u64);
                status.detail = match elapsed {
                    Some(secs) => format!("handshake sent ({}s)", secs),
                    None => "handshake sent".to_string(),
                };
                set_client_state(current_state.as_deref(), &mut state, ClientState::Connecting);
            }
            ClientPhase::Connected => {
                handshake.attempts = 0;
                handshake.retry_at = None;
                status.state = BackendState::Connected;
                set_client_state(current_state.as_deref(), &mut state, ClientState::Connected);
            }
            ClientPhase::Denied => {}
        },
        ClientLinkState::Closed(detail) => {
            if handshake.phase != ClientPhase::Denied {
                // A new link (matchbox may reconnect) re-runs the handshake.
                status.state = BackendState::Failed;
                status.detail = detail.clone();
                handshake.phase = ClientPhase::Idle;
            }
            // A deny is terminal: keep its reason and never auto-retry.
            set_client_state(current_state.as_deref(), &mut state, ClientState::Disconnected);
        }
    }

    // Latency probe: one small postcard Ping per second on the reliable control
    // channel; the server answers with Pong (see server.rs).
    match handshake.phase {
        ClientPhase::Connected => {
            if let Some(now) = now {
                match metrics.ping.next_at {
                    None => metrics.ping.next_at = Some(now + PING_INTERVAL_SECS),
                    Some(at) if now >= at => {
                        let nonce = metrics.ping.nonce;
                        metrics.ping.nonce = nonce.wrapping_add(1);
                        metrics.ping.pending = Some((nonce, now));
                        metrics.ping.next_at = Some(now + PING_INTERVAL_SECS);
                        if let Ok(bytes) = to_stdvec(&Handshake::Ping { nonce }) {
                            metrics.bytes.add_sent(bytes.len() as u64);
                            transport.send(CONTROL_CHANNEL, bytes);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {
            metrics.ping.next_at = None;
            metrics.ping.pending = None;
        }
    }

    let mut authenticated = handshake.phase == ClientPhase::Connected;
    for (channel, bytes) in transport.recv() {
        metrics.bytes.add_received(bytes.len() as u64);
        if channel == CONTROL_CHANNEL {
            match from_bytes::<Handshake>(&bytes) {
                Ok(Handshake::Welcome { sub, name, tick }) => {
                    log::info!("repl backend: session {sub} ({name}) connected at tick {tick}");
                    handshake.phase = ClientPhase::Connected;
                    authenticated = true;
                    status.state = BackendState::Connected;
                    status.detail.clear();
                    commands.insert_resource(SessionInfo {
                        sub: sub.clone(),
                        name: name.clone(),
                        tick,
                    });
                    started.write(SessionStarted { sub, name, tick });
                    set_client_state(current_state.as_deref(), &mut state, ClientState::Connected);
                }
                Ok(Handshake::Deny { reason }) => {
                    log::warn!("repl backend: handshake denied: {reason}");
                    handshake.phase = ClientPhase::Denied;
                    status.state = BackendState::Failed;
                    status.detail = format!("denied: {reason}");
                    denied.write(SessionDenied {
                        reason: reason.clone(),
                    });
                    transport.close();
                    set_client_state(current_state.as_deref(), &mut state, ClientState::Disconnected);
                }
                Ok(Handshake::Pong { nonce }) => {
                    if let (Some((pending, sent_at)), Some(now)) = (metrics.ping.pending, now)
                        && nonce == pending
                    {
                        metrics.ping.rtt = (now - sent_at).max(0.0);
                        metrics.ping.pending = None;
                    }
                }
                Ok(Handshake::Hello { .. }) | Ok(Handshake::Ping { .. }) | Err(_) => {
                    log::debug!("repl backend: ignoring unexpected control message");
                }
            }
            continue;
        }
        if authenticated && channel >= 1 {
            messages.insert_received(channel - 1, bytes);
        }
    }
}

/// Forward replicon's outbound client messages onto the matching matchbox
/// channels (only while authenticated).
fn send_packets(
    transport: Option<ResMut<ClientTransport>>,
    mut metrics: ResMut<ClientMetrics>,
    mut messages: ResMut<ClientMessages>,
    handshake: Res<ClientHandshake>,
) {
    let Some(mut transport) = transport else {
        // Nothing to send before the socket exists; replicon only writes
        // messages while it considers the client connected.
        messages.drain_sent().for_each(drop);
        return;
    };
    for (channel, bytes) in messages.drain_sent() {
        if handshake.phase == ClientPhase::Connected {
            metrics.bytes.add_sent(bytes.len() as u64);
            transport.send(channel + 1, bytes.to_vec());
        }
    }
}

/// Publish the byte counters and the measured RTT onto replicon's `ClientStats`
/// (the game logs this resource; `packet_loss` stays 0 — matchbox/SCTP does not
/// expose losses).
fn flush_client_stats(
    time: Option<Res<Time>>,
    mut metrics: ResMut<ClientMetrics>,
    mut stats: ResMut<ClientStats>,
    mut traffic: ResMut<BackendTraffic>,
) {
    let Some(time) = time else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if metrics.bytes.flush(now) {
        stats.sent_bps = metrics.bytes.sent_bps();
        stats.received_bps = metrics.bytes.received_bps();
        traffic.sent_bps = metrics.bytes.sent_bps();
        traffic.received_bps = metrics.bytes.received_bps();
    }
    // Cumulative totals: exact bytes since the socket was created, immune to
    // windowing (an idle second cannot hide them).
    traffic.sent_bytes = metrics.bytes.sent_total();
    traffic.received_bytes = metrics.bytes.received_total();
    stats.rtt = metrics.ping.rtt;
}

#[cfg(test)]
mod retry_tests {
    use super::retry_delay;

    /// Backoff doubles and caps, so a flapping mobile link settles into a
    /// 30 s retry instead of hammering signaling.
    #[test]
    fn retry_backoff_doubles_and_caps() {
        assert_eq!(retry_delay(0), 1.0);
        assert_eq!(retry_delay(1), 1.0);
        assert_eq!(retry_delay(2), 2.0);
        assert_eq!(retry_delay(3), 4.0);
        assert_eq!(retry_delay(5), 16.0);
        assert_eq!(retry_delay(6), 30.0);
        assert_eq!(retry_delay(100), 30.0);
    }
}
