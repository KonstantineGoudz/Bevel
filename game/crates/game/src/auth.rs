//! In-game signup + login against the self-hosted Rauthy IdP.
//!
//! No browser redirect: login uses the OAuth2 password grant directly against
//! `…/oidc/token`, signup goes through the sim's provisioning endpoint (the
//! sim holds a Rauthy admin API key and creates the user server-side).
//! Rauthy has no SMTP configured, so the self-service registration page
//! cannot deliver passwords — hence the sim-mediated signup.
//!
//! One persistent character per auth subject: the sim keys players by `sub`
//! (UNIQUE `rid` in SurrealDB) and stores the display name claimed here on
//! first join.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::touch::Touches;
use bevy::input_focus::tab_navigation::TabIndex;
#[cfg(not(target_os = "android"))]
use bevy::input_focus::AutoFocus;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, GlobalZIndex, UiGlobalTransform};
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};
#[cfg(target_os = "android")]
use bevy::window::PrimaryWindow;

// ---------------------------------------------------------------------------
// Config (environment-driven, conman.biz defaults)
// ---------------------------------------------------------------------------

/// Where Rauthy serves OIDC. `DUNGEON_RAUTHY_URL` matches the server crate's
/// convention (`https://auth.conman.biz` in prod).
pub fn rauthy_base() -> String {
    std::env::var("DUNGEON_RAUTHY_URL")
        .unwrap_or_else(|_| "https://auth.conman.biz".to_string())
}

/// OIDC client id created in the Rauthy admin UI. Must be a *public* client
/// (confidential OFF) with `openid` (+ `offline_access` for refresh).
pub fn rauthy_client_id() -> String {
    std::env::var("DUNGEON_RAUTHY_CLIENT_ID")
        .unwrap_or_else(|_| "party-dungeon".to_string())
}

/// Sim auth API (login / signup / refresh proxy), same origin as the game so
/// WASM needs no CORS. The sim holds the OIDC client secret, so a
/// **confidential** Rauthy client works without shipping the secret in the
/// binary. Override for local dev, e.g. `http://127.0.0.1:6565`.
pub fn auth_api_url() -> String {
    // The page sets this so the same build works on play.conman.biz (auth is
    // same-origin, proxied to the sim's :6565) and on localhost (the sim's
    // port directly): on web, `std::env` is empty, so without the override a
    // local page would post to production and die on CORS.
    #[cfg(target_arch = "wasm32")]
    if let Some(url) = crate::web_global("DUNGEON_AUTH_API_URL") {
        return url;
    }
    std::env::var("DUNGEON_AUTH_API_URL")
        .unwrap_or_else(|_| "https://play.conman.biz".to_string())
}

pub fn signup_url() -> String {
    format!("{}/signup", auth_api_url())
}

fn login_url() -> String {
    format!("{}/login", auth_api_url())
}

fn refresh_url() -> String {
    format!("{}/refresh", auth_api_url())
}

/// Dev/test escape hatch: `DUNGEON_AUTH_REQUIRED=0` skips the login gate and
/// uses the legacy env token, like before this module existed.
///
/// Headless runs (`--headless`) have no keyboard, so they can never type
/// credentials into the overlay: the gate defaults to off there, and an
/// explicit `DUNGEON_AUTH_REQUIRED` in the environment still wins.
pub fn auth_required() -> bool {
    let headless = crate::headless::is_active();
    std::env::var("DUNGEON_AUTH_REQUIRED")
        .map(|v| v != "0")
        .unwrap_or(!headless)
}

// ---------------------------------------------------------------------------
// Session + state
// ---------------------------------------------------------------------------

/// Gate for the whole online experience. While not `SignedIn`, input sampling
/// and Hello are suppressed and the auth overlay owns the screen.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AuthState {
    #[default]
    SignedOut,
    Working,
    SignedIn,
}

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AuthMode {
    #[default]
    Login,
    Register,
}

/// Live session. Memory-only for now (log in again after each launch);
/// the net client reads the token from here, falling back to the legacy
/// `DUNGEON_AUTH_TOKEN` env chain when no session exists.
#[derive(Resource, Clone, Debug)]
pub struct AuthSession {
    pub id_token: String,
    pub refresh_token: Option<String>,
    pub display_name: String,
    /// Name to claim on first Hello (signup flow). Taken once, then `None`.
    pub pending_name: Option<String>,
    obtained_at: f64,
    expires_in: u64,
}

impl AuthSession {
    fn seconds_left(&self, now: f64) -> f64 {
        (self.obtained_at + self.expires_in as f64) - now
    }
}

#[derive(Resource, Default)]
pub struct AuthForm {
    pub mode: AuthMode,
    pub error: Option<String>,
    pub busy_label: Option<String>,
}

#[derive(Debug)]
enum AuthEvent {
    LoggedIn {
        id_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
        display_name: String,
    },
    Failed(String),
}

#[derive(Resource)]
struct AuthInbox {
    rx: Mutex<Receiver<AuthEvent>>,
}

#[derive(Resource, Clone)]
struct AuthOutbox {
    tx: Sender<AuthEvent>,
}

// ---------------------------------------------------------------------------
// Mobile soft keyboard (wasm + touch devices)
// ---------------------------------------------------------------------------
//
// A Bevy UI field is pixels on a canvas, and winit's web backend attaches its
// `keydown` handlers to that canvas. A canvas is not an editable element, so
// tapping a field can never summon the OS keyboard: iOS/Android have nothing
// to open a keyboard for. Keystrokes typed into a real `<input>` do not reach
// the canvas either (the events never bubble through it), so the editor's
// `WasmTextBridge` mirrors a native input instead of relying on winit.
//
// The fix is the same idea: on a touch device we lay one transparent native
// `<input>` over each field row, positioned every frame from the Bevy layout
// rects. A tap is then a native tap on a real editable element — the gesture
// browsers require — the OS keyboard opens, and `input`/`keydown`/`focus`
// events are mirrored into [`AuthFieldValue`] / [`InputFocus`]. Desktop and
// native builds keep the Bevy-only keyboard path: `enabled` stays false and no
// native element is ever focused there.
//
// The DOM side talks to Bevy through a mutex-guarded [`DomEvents`] mailbox
// (the same pattern as the editor bridge): the JS closures deposit one frame's
// events, the Bevy system drains and applies them. The mailbox also keeps the
// text-entry rules (`AuthFieldKind::next_visible`, the 128-char cap, Enter =
// submit) in one place that a native test can exercise.

/// One frame's worth of events from the native overlay inputs.
#[derive(Debug, Default)]
struct DomEvents {
    /// A row's native input gained focus.
    focused: Option<AuthFieldKind>,
    /// A row's native input lost focus.
    blurred: Option<AuthFieldKind>,
    /// Text after a row's native input changed. While the native input holds
    /// focus it is authoritative; the Bevy [`AuthFieldValue`] is the mirror.
    edited: Option<(AuthFieldKind, String)>,
    /// Enter / "Go" pressed in a row.
    entered: Option<AuthFieldKind>,
    /// Tab pressed in a row.
    tabbed: Option<AuthFieldKind>,
    /// Escape pressed in a row: drop the inline error.
    escaped: bool,
}

impl DomEvents {
    fn is_empty(&self) -> bool {
        self.focused.is_none()
            && self.blurred.is_none()
            && self.edited.is_none()
            && self.entered.is_none()
            && self.tabbed.is_none()
            && !self.escaped
    }
}

/// What the DOM events asked the form to do once applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum DomFocus {
    /// No focus change this frame.
    #[default]
    Keep,
    /// Focus was lost (a native input blurred and nothing took over).
    Clear,
    /// The row that should own focus.
    Row(AuthFieldKind),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct DomOutcome {
    focus: DomFocus,
    /// `focus` came from Enter/Tab, so the native overlay must follow it (a
    /// DOM-originated focus is already where it belongs).
    move_dom_focus: bool,
    /// Enter was pressed on the last visible row: submit the form.
    submit: bool,
}

/// State of the native-overlay bridge. `enabled` is decided once on a touch
/// device (or on the first touch) and never goes back: a hybrid laptop must
/// keep working whichever way the player last interacted with it.
#[derive(Resource)]
struct KeyboardBridge {
    /// One frame's DOM events; the JS closures and the Bevy system share it.
    events: Arc<Mutex<DomEvents>>,
    /// Touch device: overlays may appear.
    enabled: bool,
    /// Last published rect per [`AuthFieldKind::slot`], in CSS pixels with a
    /// top-left origin. `None` means the overlay is hidden.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    shown: [Option<[f32; 4]>; 4],
}

impl Default for KeyboardBridge {
    fn default() -> Self {
        Self {
            events: Arc::new(Mutex::new(DomEvents::default())),
            enabled: false,
            shown: [None; 4],
        }
    }
}

/// Whether a row is on screen in this mode (login hides name and invite).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn field_visible(kind: AuthFieldKind, mode: AuthMode) -> bool {
    match kind {
        AuthFieldKind::Name | AuthFieldKind::Invite => mode == AuthMode::Register,
        AuthFieldKind::Email | AuthFieldKind::Password => true,
    }
}

/// CSS-pixel rect (left, top, width, height) of one field row.
///
/// [`ComputedNode`] and [`UiGlobalTransform`] carry physical pixels with the
/// transform at the node's *centre*; the DOM overlay wants CSS (logical)
/// pixels from a top-left origin. Kept out of the wasm block so the arithmetic
/// is unit tested on the native target.
fn field_rect(node: &ComputedNode, transform: &UiGlobalTransform) -> [f32; 4] {
    let inverse_scale = node.inverse_scale_factor();
    let centre = transform.translation * inverse_scale;
    let size = node.size * inverse_scale;
    [
        centre.x - size.x * 0.5,
        centre.y - size.y * 0.5,
        size.x,
        size.y,
    ]
}

/// Apply one frame of DOM events to a values snapshot.
///
/// Deliberately ECS-free so the mobile-only behaviour is still covered by the
/// native test suite: `cargo test` never runs the DOM half of the bridge.
fn apply_dom_events(
    events: DomEvents,
    mode: AuthMode,
    values: &mut [String; 4],
    form: &mut AuthForm,
) -> DomOutcome {
    let mut outcome = DomOutcome::default();
    if events.escaped {
        form.error = None;
    }
    if let Some((kind, text)) = events.edited {
        // Same cap as the desktop keyboard handler, so both input paths accept
        // exactly the same text.
        values[kind.slot()] = text.chars().take(128).collect();
        outcome.focus = DomFocus::Row(kind);
    }
    if let Some(kind) = events.focused {
        outcome.focus = DomFocus::Row(kind);
    } else if events.blurred.is_some() && outcome.focus == DomFocus::Keep {
        // Moving between rows fires blur(old) then focus(new); the focus above
        // wins, a lone blur means the keyboard went away.
        outcome.focus = DomFocus::Clear;
    }
    if let Some(kind) = events.tabbed {
        outcome.focus = DomFocus::Row(kind.next_visible(mode));
        outcome.move_dom_focus = true;
    }
    if let Some(kind) = events.entered {
        // Enter on the last row submits; on any earlier row it advances, which
        // is what a phone's "next" keyboard button should do. (The password
        // row is always the last visible one.)
        if kind == AuthFieldKind::Password {
            outcome.submit = true;
        } else {
            outcome.focus = DomFocus::Row(kind.next_visible(mode));
            outcome.move_dom_focus = true;
        }
    }
    outcome
}

// ---------------------------------------------------------------------------
// UI markers
// ---------------------------------------------------------------------------

/// Which auth-driven `Node` a container is. Visibility is synced from a single
/// tagged query rather than one `Query<&mut Node, With<…>>` per section: Bevy
/// rejects two queries that both take `&mut Node` unless every pair is
/// *provably* disjoint (`With<A>` vs `With<B>` is not — an entity may have
/// both), and hand-maintained `Without` lists are easy to get wrong. That
/// mismatch is error B0001 at runtime, not at compile time.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum AuthSection {
    /// The login/signup overlay (visible while signed out).
    Root,
    /// The signed-in identity bar.
    Bar,
    /// Display-name row (signup only).
    NameRow,
    /// Invite-code row (signup only).
    InviteRow,
}

/// Every mutable `Text` in the auth UI, tagged so one query can update them all
/// (same B0001 reasoning as [`AuthSection`]). Field labels are tagged too: their
/// text mirrors [`AuthFieldValue`], masked for the password.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum AuthTextKind {
    Error,
    Status,
    Submit,
    BarLabel,
    /// The label rendered inside one of the four input rows.
    Field(AuthFieldKind),
}

/// Which credential a row edits.
///
/// All four rows use this crate's own keyboard handling instead of Bevy's
/// `TextInput`/`EditableText` widget. That widget is focusable only through
/// [`TabIndex`]: `PointerFocusPlugin`'s resolver focuses entities that carry
/// one and otherwise bubbles the click up to the window, which *clears* focus —
/// so a click on a plain `TextInput` field could never give it focus, and every
/// keystroke went nowhere. One code path for all four fields also means the
/// password never needed to be special-cased.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AuthFieldKind {
    Email,
    Name,
    Invite,
    Password,
}

impl AuthFieldKind {
    /// Tab order: signup reads top to bottom, password last.
    const TAB_ORDER: [Self; 4] = [Self::Email, Self::Name, Self::Invite, Self::Password];

    fn slot(self) -> usize {
        match self {
            Self::Email => 0,
            Self::Name => 1,
            Self::Invite => 2,
            Self::Password => 3,
        }
    }

    fn next(self) -> Self {
        let i = Self::TAB_ORDER
            .iter()
            .position(|k| *k == self)
            .unwrap_or(0);
        Self::TAB_ORDER[(i + 1) % Self::TAB_ORDER.len()]
    }

    /// Tab must skip rows that the current mode hides: login only shows the
    /// email and password rows, so tabbing through invisible fields is a trap.
    fn next_visible(self, mode: AuthMode) -> Self {
        match mode {
            AuthMode::Login => match self {
                Self::Password => Self::Email,
                _ => Self::Password,
            },
            AuthMode::Register => self.next(),
        }
    }

    /// Only the password is masked on screen.
    fn masked(self) -> bool {
        matches!(self, Self::Password)
    }
}

/// Current text of one field (plain text; masking happens at display time).
#[derive(Component, Default)]
struct AuthFieldValue(String);

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct AuthPlugin;

impl Plugin for AuthPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuthState>()
            .init_resource::<AuthForm>()
            .init_resource::<KeyboardBridge>()
            // Exists in headless test apps too: the mobile bridge arms itself
            // on the first touch (`MinimalPlugins` has no `InputPlugin`).
            .init_resource::<Touches>();
        let (tx, rx) = mpsc::channel();
        app.insert_resource(AuthInbox {
            rx: Mutex::new(rx),
        });
        app.insert_resource(AuthOutbox { tx });
        app.add_systems(Startup, (spawn_auth_ui, setup_mobile_keyboard));
        app.add_systems(
            Update,
            (
                poll_auth_inbox,
                // Before `auth_field_typing`/`sync_auth_ui`: the DOM mirror is
                // written first so the widgets render the newest text.
                auth_mobile_keyboard,
                auth_field_typing,
                sync_auth_ui,
                refresh_check,
            ),
        );
        app.add_observer(on_auth_button);
        app.add_observer(focus_auth_field);

        // Android only: summon the soft keyboard while a field is focused.
        #[cfg(target_os = "android")]
        app.add_systems(
            Update,
            (sync_android_ime, auto_activate_on_press),
        );
    }
}

/// Auto-insert ActivateOnPress on Android so UI buttons respond immediately to touch down.
#[cfg(target_os = "android")]
fn auto_activate_on_press(
    buttons: Query<Entity, (With<Button>, Without<ActivateOnPress>)>,
    mut commands: Commands,
) {
    for entity in buttons.iter() {
        commands.entity(entity).insert(ActivateOnPress);
    }
}


/// Android soft keyboard: `NativeActivity` only shows it when asked.
/// Mirrors form focus into [`Window::ime_enabled`] so tapping a field summons
/// the keyboard, and leaving the form (submit, logout, unfocus) hides it.
/// Uses edge-triggered transitions to avoid thrashing toggleSoftInput every frame.
#[cfg(target_os = "android")]
fn sync_android_ime(
    mut is_ime_shown: Local<bool>,
    focus: Res<InputFocus>,
    state: Res<AuthState>,
    fields: Query<Entity, With<AuthFieldKind>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let want = !matches!(*state, AuthState::SignedIn | AuthState::Working)
        && focus.get().is_some_and(|entity| fields.contains(entity));
    if want {
        window.ime_enabled = true;
    }
    if *is_ime_shown != want {
        *is_ime_shown = want;
        bevy::log::info!("android-ime: transition want={} focus={:?}", want, focus.get());
        if want {
            crate::android_ime::show_keyboard_forced();
        } else {
            window.ime_enabled = false;
            crate::android_ime::hide_keyboard();
        }
    }
}


// ---------------------------------------------------------------------------
/// Android only: turn a `partydungeon://login` launch intent into a live
/// session, so the player never has to type credentials on a keyboard the OS
/// will not show. Runs once at startup; taps while the app is already running
/// are ignored (kill the app first, then tap the link).
#[cfg(target_os = "android")]
pub(super) fn apply_android_token(
    commands: &mut Commands,
    form: &mut AuthForm,
    state: &mut AuthState,
    time: &Time,
    token: crate::android_link::LaunchToken,
    source: &str,
) {
    let pending = if token.display_name.is_empty() {
        None
    } else {
        Some(token.display_name.clone())
    };
    commands.insert_resource(AuthSession {
        id_token: token.id_token,
        refresh_token: token.refresh_token,
        display_name: token.display_name,
        pending_name: pending,
        obtained_at: time.elapsed_secs_f64(),
        expires_in: token.expires_in,
    });
    form.error = None;
    form.busy_label = None;
    *state = AuthState::SignedIn;
    bevy::log::info!("android-link: signed in from {source}");
}

// Rauthy HTTP (ehttp: native + WASM, callback -> channel)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

fn form_encode(pairs: &[(&str, &str)]) -> Vec<u8> {
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

fn post_form(
    url: String,
    body: Vec<u8>,
    tx: Sender<AuthEvent>,
    on_ok: impl FnOnce(TokenResponse) -> AuthEvent + Send + 'static,
) {
    let req = ehttp::Request::new(
        ehttp::Method::POST,
        url,
        &[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Accept", "application/json"),
        ],
    )
    .with_body(body);
    ehttp::fetch(req, move |result| {
        let event = match result {
            Err(e) => AuthEvent::Failed(format!("network error: {e}")),
            Ok(resp) => {
                if !(200..300).contains(&resp.status) {
                    let detail = parse_error_detail(&resp.bytes)
                        .unwrap_or_else(|| format!("HTTP {}", resp.status));
                    AuthEvent::Failed(detail)
                } else {
                    match serde_json::from_slice::<TokenResponse>(&resp.bytes) {
                        Err(e) => AuthEvent::Failed(format!("bad token response: {e}")),
                        Ok(t) => on_ok(t),
                    }
                }
            }
        };
        let _ = tx.send(event);
    });
}

fn json_post<T: serde::Serialize>(url: impl ToString, body: &T) -> ehttp::Request {
    ehttp::Request::new(
        ehttp::Method::POST,
        url,
        &[
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
        ],
    )
    .with_body(serde_json::to_vec(body).unwrap_or_default())
}

fn parse_error_detail(bytes: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let msg = v
        .get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("error_description")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            v.get("error")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })?;
    Some(msg.chars().take(160).collect())
}

#[derive(Debug, serde::Deserialize)]
struct ProxyToken {
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Response shape of the sim's `/login` and `/refresh` proxies.
#[derive(Debug, serde::Deserialize)]
struct ProxyResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    token: Option<ProxyToken>,
}

fn token_event(t: TokenResponse, display_name: String) -> AuthEvent {
    if let Some(err) = t.error {
        let desc = t.error_description.unwrap_or_default();
        let msg = if desc.is_empty() { err } else { desc };
        return AuthEvent::Failed(msg.chars().take(160).collect());
    }
    match t.id_token {
        Some(id_token) if !id_token.is_empty() => AuthEvent::LoggedIn {
            id_token,
            refresh_token: t.refresh_token,
            expires_in: t.expires_in.unwrap_or(3600),
            display_name,
        },
        _ => AuthEvent::Failed("login succeeded but no identity token was issued".to_string()),
    }
}

fn start_login(tx: Sender<AuthEvent>, email: String, password: String, display_name: String) {
    // Credentials go to the sim's login proxy: the OIDC client secret stays
    // server-side, so a confidential Rauthy client works without changing the
    // binary. (A public client could call Rauthy directly, but then the sim
    // also has to be trusted with token validation anyway.)
    #[derive(serde::Serialize)]
    struct Body<'a> {
        email: &'a str,
        password: &'a str,
    }
    let req = json_post(
        login_url(),
        &Body {
            email: &email,
            password: &password,
        },
    );
    ehttp::fetch(req, move |result| {
        let event = match result {
            Err(e) => AuthEvent::Failed(format!("network error: {e}")),
            Ok(resp) => match serde_json::from_slice::<ProxyResponse>(&resp.bytes) {
                Ok(p) if p.ok => match p.token {
                    Some(t) => token_event(
                        TokenResponse {
                            id_token: Some(t.id_token).filter(|s| !s.is_empty()),
                            refresh_token: t.refresh_token,
                            expires_in: t.expires_in,
                            error: None,
                            error_description: None,
                        },
                        display_name,
                    ),
                    None => AuthEvent::Failed("login returned no tokens".to_string()),
                },
                Ok(p) => AuthEvent::Failed(
                    p.error
                        .unwrap_or_else(|| "invalid credentials".to_string()),
                ),
                Err(_) => {
                    let detail = parse_error_detail(&resp.bytes)
                        .unwrap_or_else(|| format!("HTTP {}", resp.status));
                    AuthEvent::Failed(detail)
                }
            },
        };
        let _ = tx.send(event);
    });
}

fn start_refresh(tx: Sender<AuthEvent>, refresh_token: String, display_name: String) {
    #[derive(serde::Serialize)]
    struct Body {
        refresh_token: String,
    }
    let req = json_post(refresh_url(), &Body { refresh_token });
    ehttp::fetch(req, move |result| {
        let event = match result {
            Err(e) => AuthEvent::Failed(format!("refresh failed: {e}")),
            Ok(resp) => match serde_json::from_slice::<ProxyResponse>(&resp.bytes) {
                Ok(p) if p.ok => match p.token {
                    Some(t) => token_event(
                        TokenResponse {
                            id_token: Some(t.id_token).filter(|s| !s.is_empty()),
                            refresh_token: t.refresh_token,
                            expires_in: t.expires_in,
                            error: None,
                            error_description: None,
                        },
                        display_name,
                    ),
                    None => AuthEvent::Failed("refresh returned no tokens".to_string()),
                },
                Ok(p) => AuthEvent::Failed(
                    p.error
                        .unwrap_or_else(|| "session expired".to_string()),
                ),
                Err(_) => AuthEvent::Failed(format!("HTTP {}", resp.status)),
            },
        };
        let _ = tx.send(event);
    });
}

#[derive(Debug, serde::Serialize)]
struct SignupRequest<'a> {
    email: &'a str,
    name: &'a str,
    password: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite: Option<&'a str>,
}

fn start_signup(
    tx: Sender<AuthEvent>,
    email: String,
    name: String,
    password: String,
    invite: Option<String>,
) {
    let url = signup_url();
    let payload = SignupRequest {
        email: &email,
        name: &name,
        password: &password,
        invite: invite.as_deref().filter(|s| !s.is_empty()),
    };
    let req = json_post(url, &payload);
    ehttp::fetch(req, move |result| {
        let event = match result {
            Err(e) => AuthEvent::Failed(format!("signup request failed: {e}")),
            Ok(resp) => {
                if !(200..300).contains(&resp.status) {
                    let detail = parse_error_detail(&resp.bytes)
                        .unwrap_or_else(|| format!("HTTP {}", resp.status));
                    AuthEvent::Failed(format!("signup rejected: {detail}"))
                } else {
                    // Account exists now — log straight in with the same creds.
                    return start_login(tx, email, password, name);
                }
            }
        };
        let _ = tx.send(event);
    });
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

fn poll_auth_inbox(
    inbox: Res<AuthInbox>,
    mut state: ResMut<AuthState>,
    mut form: ResMut<AuthForm>,
    mut commands: Commands,
    time: Res<Time>,
) {
    for event in inbox.rx.lock().unwrap().try_iter() {
        match event {
            AuthEvent::LoggedIn {
                id_token,
                refresh_token,
                expires_in,
                display_name,
            } => {
                let pending = if display_name.is_empty() {
                    None
                } else {
                    Some(display_name.clone())
                };
                commands.insert_resource(AuthSession {
                    id_token,
                    refresh_token,
                    display_name,
                    pending_name: pending,
                    obtained_at: time.elapsed_secs_f64(),
                    expires_in,
                });
                form.error = None;
                form.busy_label = None;
                *state = AuthState::SignedIn;
            }
            AuthEvent::Failed(msg) => {
                form.error = Some(msg);
                form.busy_label = None;
                if *state == AuthState::Working {
                    *state = AuthState::SignedOut;
                }
            }
        }
    }
}

/// Silent refresh ~2 min before expiry; a failed refresh drops to SignedOut.
fn refresh_check(
    // One `ResMut<AuthState>`, not `Res` + `ResMut` of the same resource: Bevy
    // rejects that pair inside a single system (B0002), so this system used to
    // panic the moment the player signed in.
    mut state: ResMut<AuthState>,
    session: Option<Res<AuthSession>>,
    outbox: Res<AuthOutbox>,
    mut form: ResMut<AuthForm>,
    time: Res<Time>,
) {
    if *state != AuthState::SignedIn {
        return;
    }
    let Some(s) = session.as_deref() else { return };
    let Some(refresh) = s.refresh_token.clone() else {
        return;
    };
    if s.seconds_left(time.elapsed_secs_f64()) > 120.0 {
        return;
    }
    *state = AuthState::Working;
    form.busy_label = Some("Refreshing session…".to_string());
    start_refresh(outbox.tx.clone(), refresh, s.display_name.clone());
}

/// Mirror the native overlay keyboard into the form and (on wasm) keep the
/// overlays glued to the Bevy layout.
///
/// Runs on every target: native builds never arm the bridge (`enabled` stays
/// false), while the native test suite simulates the mailbox to cover the
/// mirroring rules. The wasm-only tail at the bottom is pure DOM work.
#[allow(clippy::too_many_arguments)]
fn auth_mobile_keyboard(
    mut state: ResMut<AuthState>,
    mut form: ResMut<AuthForm>,
    mut focus: ResMut<InputFocus>,
    mut bridge: ResMut<KeyboardBridge>,
    touches: Res<Touches>,
    mut fields: Query<(
        Entity,
        &AuthFieldKind,
        &mut AuthFieldValue,
        Option<&ComputedNode>,
        Option<&UiGlobalTransform>,
    )>,
    outbox: Res<AuthOutbox>,
) {
    // A browser can under-report `navigator.maxTouchPoints`, and a hybrid
    // laptop only becomes a touch device when its screen is used, so arm the
    // bridge on the first touch as well. It never disarms (see the type doc).
    if !bridge.enabled && (touches.any_just_pressed() || touches.iter().next().is_some()) {
        bridge.enabled = true;
        info!("auth: mobile keyboard overlays armed (touch input detected)");
    }
    if !bridge.enabled {
        return;
    }
    // Only the signed-out form takes text; the same gate as `auth_field_typing`.
    let show = *state == AuthState::SignedOut;
    let events = {
        let mut mailbox = bridge
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *mailbox)
    };
    // One read pass: current text of every row and its last laid-out rect.
    let mut values: [String; 4] = Default::default();
    let mut rects: [Option<[f32; 4]>; 4] = [None; 4];
    for (_, kind, value, node, transform) in fields.iter() {
        values[kind.slot()] = value.0.clone();
        if let (Some(node), Some(transform)) = (node, transform) {
            let rect = field_rect(node, transform);
            // The first frame(s) have no layout yet: keep the overlay hidden
            // rather than parking a zero-sized input in the corner.
            if rect[2] > 1.0 && rect[3] > 1.0 {
                rects[kind.slot()] = Some(rect);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &rects;
    let outcome = if show && !events.is_empty() {
        apply_dom_events(events, form.mode, &mut values, &mut form)
    } else {
        DomOutcome::default()
    };
    // Write mirrored text back into the rows.
    for (_, kind, mut value, _, _) in fields.iter_mut() {
        let next = &values[kind.slot()];
        if value.0 != *next {
            value.0 = next.clone();
        }
    }
    // Park the Bevy caret where the native input went.
    match outcome.focus {
        DomFocus::Keep => {}
        DomFocus::Clear => focus.clear(),
        DomFocus::Row(kind) => {
            if let Some((entity, _, _, _, _)) = fields.iter().find(|(_, k, _, _, _)| **k == kind) {
                if focus.get() != Some(entity) {
                    focus.set(entity, FocusCause::Pressed);
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    sync_mobile_overlays(&mut bridge, show, form.mode, &values, outcome, &rects);
    if show && outcome.submit {
        try_submit(values, &mut form, &mut state, &outbox);
    }
}

/// Create the four native overlay inputs once and arm the bridge on a touch
/// device. Desktop web and native builds create nothing: without a touch
/// screen the Bevy keyboard path is the right one.
fn setup_mobile_keyboard(mut bridge: ResMut<KeyboardBridge>) {
    #[cfg(target_arch = "wasm32")]
    install_mobile_overlays(&mut bridge);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut bridge;
}

/// DOM id of the native overlay for one row (`bevy-auth-*`, next to the
/// editor's `bevy-text-input`).
#[cfg(target_arch = "wasm32")]
fn auth_input_id(kind: AuthFieldKind) -> &'static str {
    match kind {
        AuthFieldKind::Email => "bevy-auth-email",
        AuthFieldKind::Name => "bevy-auth-name",
        AuthFieldKind::Invite => "bevy-auth-invite",
        AuthFieldKind::Password => "bevy-auth-password",
    }
}

#[cfg(target_arch = "wasm32")]
const AUTH_INPUT_HIDDEN_STYLE: &str = "display:none";

/// An overlay has to be a real, focusable, in-viewport editable element — that
/// is what makes the OS open its keyboard — but nothing of it may show, or it
/// would fight the Bevy-drawn field underneath. `font-size:16px` stops iOS from
/// zooming the page when it gains focus.
#[cfg(target_arch = "wasm32")]
fn auth_input_style([x, y, w, h]: [f32; 4]) -> String {
    format!(
        "display:block;position:fixed;left:{x:.1}px;top:{y:.1}px;width:{w:.1}px;\
         height:{h:.1}px;margin:0;padding:0;border:0;outline:none;\
         background:transparent;color:transparent;caret-color:transparent;opacity:0;\
         font-size:16px;z-index:30;touch-action:manipulation;\
         -webkit-user-select:none;user-select:none;-webkit-touch-callout:none;"
    )
}

#[cfg(target_arch = "wasm32")]
fn install_mobile_overlays(bridge: &mut KeyboardBridge) {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else { return };
    if window.navigator().max_touch_points() > 0 {
        bridge.enabled = true;
    }
    let Some(document) = window.document() else { return };
    let mut installed = 0;
    for kind in AuthFieldKind::TAB_ORDER {
        let id = auth_input_id(kind);
        if document.get_element_by_id(id).is_some() {
            continue;
        }
        let Ok(element) = document.create_element("input") else { continue };
        let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else { continue };
        input.set_id(id);
        input.set_type(match kind {
            AuthFieldKind::Password => "password",
            AuthFieldKind::Email => "email",
            AuthFieldKind::Name | AuthFieldKind::Invite => "text",
        });
        // Everything a phone keyboard likes to "help" with would fight the
        // Bevy copy of the text. Tab is forwarded by the keydown handler, so
        // the element also stays out of the page's tab order.
        let _ = input.set_attribute(
            "autocomplete",
            match kind {
                AuthFieldKind::Email => "username",
                AuthFieldKind::Password => "current-password",
                _ => "off",
            },
        );
        let _ = input.set_attribute(
            "autocapitalize",
            if kind == AuthFieldKind::Name { "words" } else { "off" },
        );
        let _ = input.set_attribute("autocorrect", "off");
        let _ = input.set_attribute("spellcheck", "false");
        let _ = input.set_attribute("tabindex", "-1");
        let _ = input.set_attribute(
            "aria-label",
            match kind {
                AuthFieldKind::Email => "E-Mail",
                AuthFieldKind::Name => "Character name",
                AuthFieldKind::Invite => "Invite code",
                AuthFieldKind::Password => "Password",
            },
        );
        let _ = input.set_attribute("style", AUTH_INPUT_HIDDEN_STYLE);
        bind_mobile_events(bridge, &input, kind);
        if let Some(body) = document.body() {
            let _ = body.append_child(&input);
            installed += 1;
        }
    }
    if installed > 0 {
        info!(
            "auth: mobile keyboard overlays installed (touch device: {})",
            bridge.enabled
        );
    }
}

/// Wire one overlay's DOM events into the shared mailbox. The closures are
/// forgotten on purpose: the element lives as long as the page and must keep
/// its handlers, so there is nothing to unregister.
#[cfg(target_arch = "wasm32")]
fn bind_mobile_events(bridge: &KeyboardBridge, input: &web_sys::HtmlInputElement, kind: AuthFieldKind) {
    use wasm_bindgen::JsCast;

    let on_input = {
        let events = Arc::clone(&bridge.events);
        let element = input.clone();
        wasm_bindgen::closure::Closure::wrap(Box::new(move |_event: web_sys::Event| {
            if let Ok(mut events) = events.lock() {
                events.edited = Some((kind, element.value()));
            }
        }) as Box<dyn FnMut(web_sys::Event)>)
    };
    input.set_oninput(Some(on_input.as_ref().unchecked_ref()));
    on_input.forget();

    let on_focus = {
        let events = Arc::clone(&bridge.events);
        wasm_bindgen::closure::Closure::wrap(Box::new(move |_event: web_sys::Event| {
            if let Ok(mut events) = events.lock() {
                events.focused = Some(kind);
            }
        }) as Box<dyn FnMut(web_sys::Event)>)
    };
    input.set_onfocus(Some(on_focus.as_ref().unchecked_ref()));
    on_focus.forget();

    let on_blur = {
        let events = Arc::clone(&bridge.events);
        wasm_bindgen::closure::Closure::wrap(Box::new(move |_event: web_sys::Event| {
            if let Ok(mut events) = events.lock() {
                events.blurred = Some(kind);
            }
        }) as Box<dyn FnMut(web_sys::Event)>)
    };
    input.set_onblur(Some(on_blur.as_ref().unchecked_ref()));
    on_blur.forget();

    let on_keydown = {
        let events = Arc::clone(&bridge.events);
        wasm_bindgen::closure::Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
            // Never act on Enter/Tab until the IME has committed a composition.
            if event.is_composing() {
                return;
            }
            let Ok(mut events) = events.lock() else { return };
            match event.key().as_str() {
                "Enter" => {
                    event.prevent_default();
                    events.entered = Some(kind);
                }
                "Tab" => {
                    event.prevent_default();
                    events.tabbed = Some(kind);
                }
                "Escape" => events.escaped = true,
                _ => {}
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>)
    };
    input.set_onkeydown(Some(on_keydown.as_ref().unchecked_ref()));
    on_keydown.forget();
}

/// Position/hide the native overlays and mirror the form's text into them.
///
/// `rects` are in canvas-logical coordinates; the canvas itself can be offset
/// inside the page, so its bounding rect is added before the CSS write.
#[cfg(target_arch = "wasm32")]
fn sync_mobile_overlays(
    bridge: &mut KeyboardBridge,
    show: bool,
    mode: AuthMode,
    values: &[String; 4],
    outcome: DomOutcome,
    rects: &[Option<[f32; 4]>; 4],
) {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let origin = document
        .get_element_by_id("bevy")
        .map(|canvas| canvas.get_bounding_client_rect());
    let (origin_x, origin_y) = origin
        .map(|rect| (rect.left() as f32, rect.top() as f32))
        .unwrap_or((0.0, 0.0));
    for kind in AuthFieldKind::TAB_ORDER {
        let slot = kind.slot();
        let Some(element) = document.get_element_by_id(auth_input_id(kind)) else { continue };
        let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else { continue };
        let visible = show && field_visible(kind, mode);
        let Some(rect) = rects[slot].filter(|_| visible) else {
            // Hide once: blurring lets the OS dismiss the keyboard instead of
            // leaving it open over a form that no longer accepts text.
            if bridge.shown[slot].take().is_some() {
                let _ = input.set_attribute("style", AUTH_INPUT_HIDDEN_STYLE);
                let _ = input.blur();
            }
            continue;
        };
        // A little slack around the row so a slightly-off fingertip still
        // lands on the field, without covering the labels or the buttons.
        let padded = [
            rect[0] + origin_x - 2.0,
            rect[1] + origin_y - 4.0,
            rect[2] + 4.0,
            rect[3] + 8.0,
        ];
        if bridge.shown[slot] != Some(padded) {
            bridge.shown[slot] = Some(padded);
            let _ = input.set_attribute("style", &auth_input_style(padded));
        }
        if input.value() != values[slot] {
            input.set_value(&values[slot]);
        }
        // Enter/Go submits on the last row; earlier rows say "next".
        let hint = if kind == AuthFieldKind::Password { "go" } else { "next" };
        if input.get_attribute("enterkeyhint").as_deref() != Some(hint) {
            let _ = input.set_attribute("enterkeyhint", hint);
        }
    }
    // Enter/Tab asked Bevy to move on: take the native focus with it, so the
    // keyboard follows to the next row instead of closing.
    if outcome.move_dom_focus {
        if let DomFocus::Row(kind) = outcome.focus {
            if let Some(element) = document.get_element_by_id(auth_input_id(kind)) {
                if let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() {
                    let _ = input.focus();
                }
            }
        }
    }
}

/// Keyboard handling for all four input rows.
///
/// This crate owns text entry instead of using Bevy's `TextInput` widget: that
/// widget only becomes focusable through [`TabIndex`] (a click on a field
/// without one bubbles to the window and *clears* focus), which is why every
/// keystroke used to go nowhere while the hand-written password field kept
/// working. One handler also means Enter submits from any field and Tab walks
/// the form, instead of only the password row reacting.
fn auth_field_typing(
    mut keys: MessageReader<KeyboardInput>,
    mut focus: ResMut<InputFocus>,
    mut fields: Query<(Entity, &AuthFieldKind, &mut AuthFieldValue)>,
    mut form: ResMut<AuthForm>,
    mut state: ResMut<AuthState>,
    outbox: Res<AuthOutbox>,
) {
    if *state == AuthState::SignedIn || *state == AuthState::Working {
        return;
    }
    let Some(focused) = focus.get() else {
        return;
    };
    let Ok((_, kind, mut value)) = fields.get_mut(focused) else {
        return;
    };
    let kind = *kind;
    let mut action = FieldAction::None;
    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        // Named keys first: on some platforms Enter/Backspace also carry a
        // `text` payload, which must not be inserted as a character.
        match &ev.logical_key {
            Key::Enter => {
                action = FieldAction::Submit;
                continue;
            }
            Key::Escape => {
                action = FieldAction::ClearError;
                continue;
            }
            Key::Tab => {
                action = FieldAction::FocusField(kind.next_visible(form.mode));
                continue;
            }
            Key::Backspace | Key::Delete => {
                value.0.pop();
                continue;
            }
            _ => {}
        }
        // Printable text (layout-aware: handles Shift, caps, dead keys).
        if let Some(text) = ev.text.as_ref() {
            if !text.chars().any(|c| c.is_control()) && value.0.chars().count() < 128 {
                value.0.push_str(text.as_str());
            }
        }
    }
    // The mutable borrow of `value` ends here, so the query can be read again.
    match action {
        FieldAction::None => {}
        FieldAction::ClearError => form.error = None,
        FieldAction::Submit => {
            let values = field_values(fields.iter().map(|(_, k, v)| (k, v)));
            try_submit(values, &mut form, &mut state, &outbox);
        }
        FieldAction::FocusField(next) => {
            if let Some((entity, _, _)) = fields.iter().find(|(_, k, _)| **k == next) {
                focus.set(entity, FocusCause::Navigated);
            }
        }
    }
}

/// What a key press asked the form to do (applied after the borrow ends).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldAction {
    None,
    ClearError,
    Submit,
    FocusField(AuthFieldKind),
}

/// Snapshot the current text of every field, indexed by [`AuthFieldKind::slot`].
fn field_values<'a>(
    fields: impl Iterator<Item = (&'a AuthFieldKind, &'a AuthFieldValue)>,
) -> [String; 4] {
    let mut values: [String; 4] = Default::default();
    for (kind, value) in fields {
        values[kind.slot()] = value.0.clone();
    }
    values
}

/// Validate the snapshot and fire the request, flipping to `Working` on success.
fn try_submit(
    values: [String; 4],
    form: &mut AuthForm,
    state: &mut AuthState,
    outbox: &AuthOutbox,
) -> bool {
    if *state == AuthState::Working {
        return false;
    }
    let [email, name, invite, password] = values;
    let sent = submit(
        email.trim().to_lowercase(),
        name.trim().to_string(),
        password,
        invite.trim().to_string(),
        form.mode,
        outbox,
    );
    if sent {
        *state = AuthState::Working;
        form.busy_label = Some(match form.mode {
            AuthMode::Login => "Logging in…".to_string(),
            AuthMode::Register => "Creating character…".to_string(),
        });
        form.error = None;
    }
    sent
}

/// Validate, then fire the request. Returns true when a request went out
/// (caller flips to Working); validation failures report inline instead.
fn submit(
    email: String,
    name: String,
    password: String,
    invite: String,
    mode: AuthMode,
    outbox: &AuthOutbox,
) -> bool {
    let fail = |msg: &str| {
        let _ = outbox
            .tx
            .send(AuthEvent::Failed(msg.to_string()));
    };
    if !email.contains('@') || email.len() < 3 {
        fail("enter a valid email address");
        return false;
    }
    if password.is_empty() {
        fail("enter your password");
        return false;
    }
    match mode {
        AuthMode::Login => {
            let display = name_or_email(&name, &email);
            start_login(outbox.tx.clone(), email, password, display);
            true
        }
        AuthMode::Register => {
            if name.is_empty() {
                fail("pick a character name");
                return false;
            }
            if name.chars().count() > 24 {
                fail("character name must be 24 characters or less");
                return false;
            }
            // Mirror the server/Rauthy policy so the player gets instant
            // feedback instead of a round trip.
            if password.len() < 8 {
                fail("password must be at least 8 characters");
                return false;
            }
            if !password.chars().any(|c| c.is_ascii_uppercase()) {
                fail("password needs at least one uppercase letter");
                return false;
            }
            if !password.chars().any(|c| c.is_ascii_lowercase()) {
                fail("password needs at least one lowercase letter");
                return false;
            }
            if !password.chars().any(|c| c.is_ascii_digit()) {
                fail("password needs at least one digit");
                return false;
            }
            if !password
                .chars()
                .any(|c| !c.is_ascii_alphanumeric() && !c.is_whitespace())
            {
                fail("password needs at least one special character");
                return false;
            }
            let invite = if invite.is_empty() { None } else { Some(invite) };
            start_signup(outbox.tx.clone(), email, name, password, invite);
            true
        }
    }
}

fn name_or_email(name: &str, email: &str) -> String {
    if name.is_empty() {
        email.split('@').next().unwrap_or("Adventurer").to_string()
    } else {
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Component)]
pub(crate) enum AuthButton {
    TabLogin,
    TabRegister,
    Submit,
    Logout,
    /// Android only: paste the `partydungeon://login?...` app-login link
    /// from the clipboard (the soft keyboard cannot be summoned, so typing
    /// credentials is not an option).
    #[cfg(target_os = "android")]
    PasteToken,
}

#[allow(clippy::too_many_arguments)]
fn on_auth_button(
    event: On<Activate>,
    time: Res<Time>,
    buttons: Query<&AuthButton>,
    mut form: ResMut<AuthForm>,
    mut state: ResMut<AuthState>,
    fields: Query<(Entity, &AuthFieldKind, &AuthFieldValue)>,
    outbox: Res<AuthOutbox>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    #[cfg(target_os = "android")]
    bevy::log::info!("android-button: Activate on {:?}", button);
    match button.clone() {
        AuthButton::TabLogin => {
            form.mode = AuthMode::Login;
            form.error = None;
            focus.clear();
        }
        AuthButton::TabRegister => {
            form.mode = AuthMode::Register;
            form.error = None;
            focus.clear();
        }
        AuthButton::Submit => {
            focus.clear();
            let values = field_values(fields.iter().map(|(_, k, v)| (k, v)));
            try_submit(values, &mut form, &mut state, &outbox);
        }
        AuthButton::Logout => {
            focus.clear();
            commands.remove_resource::<AuthSession>();
            form.error = None;
            form.busy_label = None;
            *state = AuthState::SignedOut;
        }
        #[cfg(target_os = "android")]
        AuthButton::PasteToken => {
            focus.clear();
            match crate::android_clipboard::Clipboard::get_text()
                .and_then(|text| crate::android_link::parse_pasted_token(&text))
            {
                Some(token) => {
                    apply_android_token(
                        &mut commands,
                        &mut form,
                        &mut state,
                        &time,
                        token,
                        "pasted link",
                    );
                }
                None => {
                    form.error = Some(
                        "Clipboard has no app login link — log in on the web page and copy it first."
                            .to_string(),
                    );
                }
            }
        }
    }
}

/// Focus an input row when it is clicked, so the editor's hotkeys stay muted
/// while the player types. `TabIndex` already makes Bevy's pointer-focus
/// resolver pick the row; doing it here as well keeps focus honest on the same
/// path the password row always used.
fn focus_auth_field(
    event: On<Activate>,
    fields: Query<&AuthFieldKind>,
    mut focus: ResMut<InputFocus>,
) {
    if fields.get(event.entity).is_ok() {
        #[cfg(target_os = "android")]
        bevy::log::info!("android-focus: row focused {:?}", event.entity);
        focus.set(event.entity, FocusCause::Pressed);
        #[cfg(target_os = "android")]
        crate::android_ime::show_keyboard_forced();
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

const PANEL: Color = Color::srgba(0.05, 0.07, 0.11, 0.97);
const FIELD_BG: Color = Color::srgba(0.10, 0.13, 0.18, 1.0);
const BTN_BG: Color = Color::srgba(0.16, 0.22, 0.32, 1.0);
const BTN_ON: Color = Color::srgba(0.85, 0.55, 0.16, 1.0);
const BORDER: Color = Color::srgba(0.35, 0.42, 0.55, 0.9);
const BORDER_ON: Color = Color::srgba(0.95, 0.65, 0.25, 1.0);
const TEXT_DIM: Color = Color::srgb_u8(0x9a, 0xa7, 0xbb);
const TEXT_ERR: Color = Color::srgb_u8(0xe5, 0x5a, 0x4e);

fn label(commands: &mut Commands, text: &str) -> Entity {
    commands
        .spawn((
            Text::new(text),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(TEXT_DIM),
        ))
        .id()
}

fn auth_button(commands: &mut Commands, action: AuthButton, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            ActivateOnPress,
            action,
            Node {
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BTN_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        ))
        .id()
}

/// One input row: a focusable button-like node plus a text child that mirrors
/// [`AuthFieldValue`]. `Button` gives the click -> `Activate` path (the same one
/// the password row always used) and `TabIndex` keeps Bevy's own focus resolver
/// happy, so a click focuses the row through both paths.
fn auth_field(commands: &mut Commands, kind: AuthFieldKind, tab: i32) -> Entity {
    commands
        .spawn((
            kind,
            AuthFieldValue::default(),
            Button,
            ActivateOnPress,
            TabIndex(tab),
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(30.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(FIELD_BG),
            BorderColor::all(BORDER),
            children![(
                AuthTextKind::Field(kind),
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        ))
        .id()
}

fn spawn_auth_ui(mut commands: Commands) {
    // Nobody can type in a headless run, and an overlay that never closes
    // would cover every rendered frame (and every `/screenshot`). The gate is
    // bypassed there, so the overlay has nothing to do anyway.
    if crate::headless::is_active() {
        return;
    }
    let root = commands
        .spawn((
            AuthSection::Root,
            GlobalZIndex(100),
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.88)),
        ))
        .id();

    let panel = commands
        .spawn((
            Node {
                width: Val::Px(400.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(20.0)),
                row_gap: Val::Px(8.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor::all(BORDER_ON),
        ))
        .id();

    let title = commands
        .spawn((
            Text::new("Party Dungeon"),
            TextFont {
                font_size: FontSize::Px(24.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    let subtitle = commands
        .spawn((
            Text::new("One world. One character."),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(TEXT_DIM),
        ))
        .id();

    let tabs = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(8.0),
            ..default()
        })
        .id();
    let tab_login = auth_button(&mut commands, AuthButton::TabLogin, "Log in");
    let tab_register = auth_button(&mut commands, AuthButton::TabRegister, "Sign up");
    commands.entity(tabs).add_children(&[tab_login, tab_register]);

    let email_label = label(&mut commands, "E-Mail");
    let email_field = auth_field(&mut commands, AuthFieldKind::Email, 0);
    #[cfg(not(target_os = "android"))]
    commands.entity(email_field).insert(AutoFocus);

    let name_row = commands
        .spawn((
            AuthSection::NameRow,
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .id();
    let name_label = label(&mut commands, "Character name");
    let name_field = auth_field(&mut commands, AuthFieldKind::Name, 1);
    commands.entity(name_row).add_children(&[name_label, name_field]);

    let pw_label = label(&mut commands, "Password");
    let password_field = auth_field(&mut commands, AuthFieldKind::Password, 3);

    let invite_row = commands
        .spawn((
            AuthSection::InviteRow,
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
        ))
        .id();
    let invite_label = label(&mut commands, "Invite code (if required)");
    let invite_field = auth_field(&mut commands, AuthFieldKind::Invite, 2);
    commands
        .entity(invite_row)
        .add_children(&[invite_label, invite_field]);

    let error_text = commands
        .spawn((
            AuthTextKind::Error,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(TEXT_ERR),
        ))
        .id();
    let status_text = commands
        .spawn((
            AuthTextKind::Status,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(TEXT_DIM),
        ))
        .id();
    let submit = commands
        .spawn((
            Button,
            ActivateOnPress,
            AuthButton::Submit,
            Node {
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BTN_BG),
            BorderColor::all(BORDER),
            children![(
                AuthTextKind::Submit,
                Text::new("Log in"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        ))
        .id();

    #[cfg(target_os = "android")]
    let paste = auth_button(&mut commands, AuthButton::PasteToken, "Paste login link");
    #[cfg(target_os = "android")]
    commands.entity(panel).add_child(paste);
    commands.entity(panel).add_children(&[
        title,
        subtitle,
        tabs,
        email_label,
        email_field,
        name_row,
        pw_label,
        password_field,
        invite_row,
        error_text,
        status_text,
        submit,
    ]);
    commands.entity(root).add_child(panel);
    let _ = password_field;

    // Compact top-right bar while signed in (identity + logout).
    let bar = commands
        .spawn((
            AuthSection::Bar,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                top: Val::Px(12.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
            BorderColor::all(BORDER),
        ))
        .id();
    let bar_label = commands
        .spawn((
            AuthTextKind::BarLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(TEXT_DIM),
        ))
        .id();
    let logout = auth_button(&mut commands, AuthButton::Logout, "Log out");
    commands.entity(bar).add_children(&[bar_label, logout]);
}

/// Mirror form state into the widgets each frame (cheap; avoids rebuilds).
#[allow(clippy::too_many_arguments)]
fn sync_auth_ui(
    state: Res<AuthState>,
    form: Res<AuthForm>,
    session: Option<Res<AuthSession>>,
    focus: Res<InputFocus>,
    mut sections: Query<(&mut Node, &AuthSection)>,
    mut tab_buttons: Query<(&AuthButton, &mut BackgroundColor)>,
    fields: Query<(Entity, &AuthFieldKind, &AuthFieldValue)>,
    mut texts: Query<(&mut Text, &AuthTextKind)>,
) {
    let signed_in = *state == AuthState::SignedIn;
    let show_register = form.mode == AuthMode::Register && !signed_in;
    // Visibility per section. One query over `&mut Node`: see `AuthSection`.
    for (mut node, section) in &mut sections {
        let display = match section {
            AuthSection::Root => {
                if signed_in {
                    Display::None
                } else {
                    Display::Flex
                }
            }
            AuthSection::Bar => {
                if signed_in {
                    Display::Flex
                } else {
                    Display::None
                }
            }
            AuthSection::NameRow | AuthSection::InviteRow => {
                if show_register {
                    Display::Flex
                } else {
                    Display::None
                }
            }
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (button, mut bg) in &mut tab_buttons {
        let on = match button {
            AuthButton::TabLogin => form.mode == AuthMode::Login,
            AuthButton::TabRegister => form.mode == AuthMode::Register,
            _ => continue,
        };
        bg.0 = if on { BTN_ON } else { BTN_BG };
    }
    // Field contents, resolved once so the tagged text pass stays a single
    // mutable query over `&mut Text`.
    let mut values: [String; 4] = Default::default();
    let mut focus_kind = None;
    for (entity, kind, value) in fields.iter() {
        values[kind.slot()] = value.0.clone();
        if focus.get() == Some(entity) {
            focus_kind = Some(*kind);
        }
    }
    // Every mutable `Text` in the panel, dispatched by tag (one query, so no
    // overlapping `&mut Text` borrow). Only write when the value changed to
    // keep change detection quiet.
    for (mut text, kind) in &mut texts {
        let next: Option<String> = match kind {
            AuthTextKind::Error => Some(form.error.clone().unwrap_or_default()),
            AuthTextKind::Status => Some(match *state {
                AuthState::Working => form
                    .busy_label
                    .clone()
                    .unwrap_or_else(|| "Working…".to_string()),
                _ => String::new(),
            }),
            AuthTextKind::Submit => Some(
                match form.mode {
                    AuthMode::Login => "Log in",
                    AuthMode::Register => "Create character",
                }
                .to_string(),
            ),
            AuthTextKind::Field(kind) => {
                let raw = &values[kind.slot()];
                let mut shown = if kind.masked() {
                    std::iter::repeat_n('•', raw.chars().count()).collect::<String>()
                } else {
                    raw.clone()
                };
                // A caret is the only focus affordance in a canvas UI: without
                // it a focused empty field looks the same as an unfocused one.
                if focus_kind == Some(*kind) {
                    shown.push('▏');
                }
                Some(shown)
            }
            AuthTextKind::BarLabel => session
                .as_deref()
                .map(|s| format!("{} (1 character)", s.display_name)),
        };
        if let Some(next) = next {
            if text.0 != next {
                text.0 = next;
            }
        }
    }
}

/// The live login's identity token, when one exists.
///
/// `None` covers both "signed out" and "signed in, but the `AuthSession`
/// resource has not been inserted yet" — the latter is real: `poll_auth_inbox`
/// flips [`AuthState`] to `SignedIn` immediately but inserts the session
/// through deferred `Commands`, so a consumer system can run in between.
pub fn live_token(session: Option<&AuthSession>) -> Option<String> {
    session
        .filter(|s| !s.id_token.is_empty())
        .map(|s| s.id_token.clone())
}

/// The token to hand to the sim in the handshake (`Hello` / matchbox `Hello`).
///
/// Prefer the live login token. Fall back to the legacy env chain
/// (`DUNGEON_AUTH_TOKEN` → … → `dev-anon`) **only when the auth gate is off**
/// (headless/dev). While the gate is on, `None` means "wait a frame": the
/// caller must not send the fallback, because the sim rejects `dev-anon` as a
/// malformed JWT and would deny a session the player just logged into — and a
/// denial is sticky client-side, so the fresh login never recovers.
pub fn hello_token(session: Option<&AuthSession>) -> Option<String> {
    hello_token_with(session, auth_required(), || crate::net_client::auth_token())
}

/// Testable core of [`hello_token`] (env-independent: the gate and the
/// fallback are passed in).
fn hello_token_with(
    session: Option<&AuthSession>,
    auth_required: bool,
    fallback: impl FnOnce() -> String,
) -> Option<String> {
    if let Some(token) = live_token(session) {
        return Some(token);
    }
    if auth_required {
        return None;
    }
    Some(fallback())
}

/// Token shape for diagnostics, never the token itself.
///
/// A JWT is `header.payload.signature` (3 dot-separated parts); the dev
/// fallback is one opaque word. That difference is exactly the failure mode
/// [`hello_token`] guards against, so the handshake log says which one went out.
pub fn token_shape(token: &str) -> String {
    format!(
        "len={} parts={} prefix={:?}",
        token.len(),
        token.split('.').count(),
        token.chars().take(8).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::MinimalPlugins;
    use bevy::input::keyboard::KeyboardInput;

    /// Headless stand-in for the app: just enough resources for the plugin's
    /// systems, no renderer and no network. `setup`/`update` below assert the
    /// things the browser would otherwise discover only at runtime.
    fn auth_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<InputFocus>();
        app.add_message::<KeyboardInput>();
        app.add_plugins(AuthPlugin);
        app
    }

    fn display_of(app: &mut App, section: AuthSection) -> Display {
        let mut query = app
            .world_mut()
            .query::<(&AuthSection, &Node)>();
        query
            .iter(app.world())
            .find(|(tag, _)| **tag == section)
            .map(|(_, node)| node.display)
            .unwrap_or_else(|| panic!("no {section:?} section in the auth UI"))
    }

    fn text_of(app: &mut App, kind: AuthTextKind) -> String {
        let mut query = app.world_mut().query::<(&AuthTextKind, &Text)>();
        query
            .iter(app.world())
            .find(|(tag, _)| **tag == kind)
            .map(|(_, text)| text.0.clone())
            .unwrap_or_else(|| panic!("no {kind:?} text in the auth UI"))
    }

    /// Initializing and running every system in the schedule is what raises
    /// error B0001 (conflicting access in one system's params, e.g. two
    /// `Query<&mut Node>` Bevy cannot prove disjoint). That panic used to show
    /// up only in the browser, which is why this test exists.
    #[test]
    fn auth_schedule_runs_without_panicking() {
        let mut app = auth_app();
        app.update();
        app.update();
    }

    fn field_entity(app: &mut App, kind: AuthFieldKind) -> Entity {
        let mut query = app.world_mut().query::<(Entity, &AuthFieldKind)>();
        query
            .iter(app.world())
            .find(|(_, k)| **k == kind)
            .map(|(entity, _)| entity)
            .unwrap_or_else(|| panic!("no {kind:?} field in the auth UI"))
    }

    fn field_text(app: &mut App, kind: AuthFieldKind) -> String {
        let mut query = app.world_mut().query::<(&AuthFieldKind, &AuthFieldValue)>();
        query
            .iter(app.world())
            .find(|(k, _)| **k == kind)
            .map(|(_, value)| value.0.clone())
            .unwrap_or_else(|| panic!("no {kind:?} field in the auth UI"))
    }

    fn focus_field(app: &mut App, kind: AuthFieldKind) {
        let entity = field_entity(app, kind);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(entity, FocusCause::Pressed);
    }

    /// Feed one key press the way `bevy_winit` would.
    fn press(app: &mut App, key: Key, text: Option<&str>) {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: key,
            state: ButtonState::Pressed,
            text: text.map(|t| t.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            press(
                app,
                Key::Character(c.to_string().into()),
                Some(&c.to_string()),
            );
        }
    }

    /// The whole reason this module owns text entry: a click has to be able to
    /// focus a row, and every keystroke has to land in the focused row.
    #[test]
    fn click_focuses_a_field_and_typing_lands_in_it() {
        let mut app = auth_app();
        app.update();
        let name = field_entity(&mut app, AuthFieldKind::Name);
        app.world_mut().trigger(Activate { entity: name });
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(name));

        type_str(&mut app, "vex@a.io");
        app.update();
        assert_eq!(field_text(&mut app, AuthFieldKind::Name), "vex@a.io");
        // Untouched fields stay empty: the text went to the focused row only.
        assert_eq!(field_text(&mut app, AuthFieldKind::Email), "");
    }

    #[test]
    fn backspace_and_tab_work() {
        let mut app = auth_app();
        app.update();
        focus_field(&mut app, AuthFieldKind::Email);
        type_str(&mut app, "abc");
        press(&mut app, Key::Backspace, None);
        app.update();
        assert_eq!(field_text(&mut app, AuthFieldKind::Email), "ab");

        // Login hides the name/invite rows, so Tab goes email -> password …
        press(&mut app, Key::Tab, None);
        app.update();
        let password = field_entity(&mut app, AuthFieldKind::Password);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(password));

        // … while signup walks all four rows in order.
        app.world_mut().resource_mut::<AuthForm>().mode = AuthMode::Register;
        press(&mut app, Key::Tab, None);
        app.update();
        let email = field_entity(&mut app, AuthFieldKind::Email);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(email));
    }

    /// Enter has to submit from any row, not just the password row.
    #[test]
    fn enter_submits_the_form() {
        let mut app = auth_app();
        app.update();
        focus_field(&mut app, AuthFieldKind::Email);
        press(&mut app, Key::Enter, None);
        app.update();
        app.update();
        let error = app
            .world()
            .resource::<AuthForm>()
            .error
            .clone()
            .unwrap_or_default();
        assert!(error.contains("valid email"), "unexpected error: {error:?}");
    }

    #[test]
    fn password_is_masked_on_screen() {
        let mut app = auth_app();
        app.update();
        focus_field(&mut app, AuthFieldKind::Password);
        type_str(&mut app, "abc");
        app.update();
        let mut query = app
            .world_mut()
            .query::<(&AuthTextKind, &Text)>();
        let shown = query
            .iter(app.world())
            .find(|(kind, _)| **kind == AuthTextKind::Field(AuthFieldKind::Password))
            .map(|(_, text)| text.0.clone())
            .expect("password row has a label");
        assert_eq!(shown, "•••▏");
    }

    #[test]
    fn signed_out_shows_login_forms_only() {
        let mut app = auth_app();
        app.update();
        assert_eq!(display_of(&mut app, AuthSection::Root), Display::Flex);
        assert_eq!(display_of(&mut app, AuthSection::Bar), Display::None);
        assert_eq!(display_of(&mut app, AuthSection::NameRow), Display::None);
        assert_eq!(display_of(&mut app, AuthSection::InviteRow), Display::None);
        assert_eq!(text_of(&mut app, AuthTextKind::Submit), "Log in");
    }

    #[test]
    fn register_mode_reveals_name_and_invite_rows() {
        let mut app = auth_app();
        app.update();
        app.world_mut().resource_mut::<AuthForm>().mode = AuthMode::Register;
        app.update();
        assert_eq!(display_of(&mut app, AuthSection::NameRow), Display::Flex);
        assert_eq!(display_of(&mut app, AuthSection::InviteRow), Display::Flex);
        assert_eq!(
            text_of(&mut app, AuthTextKind::Submit),
            "Create character"
        );
    }

    fn session_with(id_token: &str) -> AuthSession {
        AuthSession {
            id_token: id_token.to_string(),
            refresh_token: None,
            display_name: "Vex".to_string(),
            pending_name: None,
            obtained_at: 0.0,
            expires_in: 3600,
        }
    }

    /// The handshake must never substitute the dev token for a login. The
    /// regression: `SignedIn` is set a frame before the deferred `AuthSession`
    /// insert lands, so `hello_token` saw no session and returned `dev-anon`,
    /// which the sim rejects as "Malformed token" — on the very login the
    /// player just completed (and a denial is sticky, so they were stuck).
    #[test]
    fn hello_token_waits_for_the_session_instead_of_the_dev_fallback() {
        // Signed out: there is nothing to hand to the sim.
        assert_eq!(hello_token(None), None);
        // Signed in, session not visible yet (or empty): wait, do not fall
        // back to `dev-anon` — that is exactly what got the session denied.
        assert_eq!(hello_token(Some(&session_with(""))), None);
        // Once it lands, the identity token is used verbatim.
        assert_eq!(
            hello_token(Some(&session_with("a.b.c"))).as_deref(),
            Some("a.b.c")
        );
        // The dev fallback is still reachable — but only when the gate is off
        // (headless/dev), which is exactly what "auth not required" means.
        assert_eq!(
            hello_token_with(None, false, || "dev-anon".to_string()).as_deref(),
            Some("dev-anon")
        );
    }

    /// The diagnostic must describe a token without ever printing it.
    #[test]
    fn token_shape_separates_jwts_from_the_dev_fallback() {
        let jwt = token_shape("eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJ4In0.c2ln");
        assert!(jwt.contains("parts=3"), "{jwt}");
        assert!(!jwt.contains("c2ln"), "signature leaked: {jwt}");
        let dev = token_shape("dev-anon");
        assert!(dev.contains("parts=1"), "{dev}");
    }

    #[test]
    fn signed_in_hides_overlay_and_labels_the_bar() {
        let mut app = auth_app();
        app.update();
        app.world_mut().insert_resource(AuthSession {
            id_token: "token".to_string(),
            refresh_token: None,
            display_name: "Vex".to_string(),
            pending_name: None,
            obtained_at: 0.0,
            // Far enough out that `refresh_check` stays quiet (no HTTP).
            expires_in: 3600,
        });
        app.world_mut().resource_mut::<AuthState>().clone_from(&AuthState::SignedIn);
        app.update();
        assert_eq!(display_of(&mut app, AuthSection::Root), Display::None);
        assert_eq!(display_of(&mut app, AuthSection::Bar), Display::Flex);
        assert_eq!(
            text_of(&mut app, AuthTextKind::BarLabel),
            "Vex (1 character)"
        );
    }

    // -- Mobile soft-keyboard bridge --------------------------------------
    //
    // The DOM half only exists on wasm, so these drive the same mailbox the
    // native `<input>` closures fill and the same rules the wasm path uses.

    /// Arm the bridge and deposit one frame of DOM events, exactly like the
    /// overlay closures would.
    fn bridge_events(app: &mut App, mutate: impl FnOnce(&mut DomEvents)) {
        app.world_mut().resource_mut::<KeyboardBridge>().enabled = true;
        let events = {
            let bridge = app.world().resource::<KeyboardBridge>();
            Arc::clone(&bridge.events)
        };
        let mut mailbox = events.lock().unwrap();
        mutate(&mut mailbox);
    }

    /// Tapping a native overlay must focus that row and typing must land in
    /// the right `AuthFieldValue` — this is the whole fix.
    #[test]
    fn mobile_overlay_events_reach_the_form() {
        let mut app = auth_app();
        app.update();
        bridge_events(&mut app, |events| {
            events.focused = Some(AuthFieldKind::Email);
            events.edited = Some((AuthFieldKind::Email, "vex@a.io".to_string()));
        });
        app.update();
        assert_eq!(field_text(&mut app, AuthFieldKind::Email), "vex@a.io");
        assert_eq!(field_text(&mut app, AuthFieldKind::Password), "");
        let email = field_entity(&mut app, AuthFieldKind::Email);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(email));
    }

    /// A phone's Enter/"Go" advances to the next visible row and submits on
    /// the password row. Empty values fail validation inline, so no HTTP fires.
    #[test]
    fn mobile_enter_advances_then_submits() {
        let mut app = auth_app();
        app.update();
        app.world_mut().resource_mut::<AuthForm>().mode = AuthMode::Register;
        bridge_events(&mut app, |events| events.entered = Some(AuthFieldKind::Email));
        app.update();
        let name = field_entity(&mut app, AuthFieldKind::Name);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(name));

        bridge_events(&mut app, |events| {
            events.entered = Some(AuthFieldKind::Password)
        });
        app.update();
        app.update();
        let error = app
            .world()
            .resource::<AuthForm>()
            .error
            .clone()
            .unwrap_or_default();
        assert!(error.contains("valid email"), "unexpected error: {error:?}");
    }

    /// Pure rules: text cap, Tab skipping hidden signup rows, blur vs focus,
    /// and Enter semantics per row.
    #[test]
    fn mobile_dom_event_rules() {
        let mut values: [String; 4] = Default::default();
        let mut form = AuthForm::default();

        let outcome = apply_dom_events(
            DomEvents {
                edited: Some((AuthFieldKind::Invite, "x".repeat(200))),
                ..default()
            },
            AuthMode::Register,
            &mut values,
            &mut form,
        );
        assert_eq!(values[AuthFieldKind::Invite.slot()].chars().count(), 128);
        assert_eq!(outcome.focus, DomFocus::Row(AuthFieldKind::Invite));

        // Login hides name/invite, so Tab goes straight to the password.
        let outcome = apply_dom_events(
            DomEvents {
                tabbed: Some(AuthFieldKind::Email),
                ..default()
            },
            AuthMode::Login,
            &mut values,
            &mut form,
        );
        assert_eq!(outcome.focus, DomFocus::Row(AuthFieldKind::Password));
        assert!(outcome.move_dom_focus);

        // A lone blur drops the caret; a blur followed by a focus keeps it.
        let outcome = apply_dom_events(
            DomEvents {
                blurred: Some(AuthFieldKind::Email),
                ..default()
            },
            AuthMode::Login,
            &mut values,
            &mut form,
        );
        assert_eq!(outcome.focus, DomFocus::Clear);
        let outcome = apply_dom_events(
            DomEvents {
                blurred: Some(AuthFieldKind::Email),
                focused: Some(AuthFieldKind::Password),
                ..default()
            },
            AuthMode::Login,
            &mut values,
            &mut form,
        );
        assert_eq!(outcome.focus, DomFocus::Row(AuthFieldKind::Password));

        // Enter on password submits; elsewhere it advances.
        let outcome = apply_dom_events(
            DomEvents {
                entered: Some(AuthFieldKind::Password),
                ..default()
            },
            AuthMode::Register,
            &mut values,
            &mut form,
        );
        assert!(outcome.submit);
        assert_eq!(outcome.focus, DomFocus::Keep);
        let outcome = apply_dom_events(
            DomEvents {
                entered: Some(AuthFieldKind::Name),
                ..default()
            },
            AuthMode::Register,
            &mut values,
            &mut form,
        );
        assert_eq!(outcome.focus, DomFocus::Row(AuthFieldKind::Invite));
        assert!(!outcome.submit);
    }

    /// Bevy reports UI geometry in physical pixels with a centre origin; the
    /// DOM overlay needs CSS pixels with a top-left origin.
    #[test]
    fn field_rect_maps_physical_centre_to_css_top_left() {
        let node = ComputedNode {
            size: Vec2::new(400.0, 60.0),
            inverse_scale_factor: 0.5,
            ..Default::default()
        };
        let transform = UiGlobalTransform::from_xy(600.0, 200.0);
        assert_eq!(field_rect(&node, &transform), [200.0, 85.0, 200.0, 30.0]);

        assert!(field_visible(AuthFieldKind::Name, AuthMode::Register));
        assert!(!field_visible(AuthFieldKind::Name, AuthMode::Login));
        assert!(field_visible(AuthFieldKind::Password, AuthMode::Login));
    }
}
