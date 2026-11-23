//! Android paste-login token parsing: `partydungeon://login?token=…`.
//!
//! The soft keyboard cannot be summoned reliably on `NativeActivity` (no
//! editable view for the IME to attach to), so typing credentials in-app is
//! not an option. Instead the player logs in once in the system browser
//! (which has a real keyboard) on a tiny same-origin page
//! (`web/app-login.html`), copies the app-login link, and pastes it into the
//! game's login form with the "Paste login link" button. This module parses
//! that text into a live [`AuthSession`](crate::auth::AuthSession) — no
//! typing, no secrets baked into the binary.
//!
//! (An earlier revision read the launch `Intent` directly, but the only
//! context handle available has no `getIntent` method, so the clipboard is
//! the handoff instead.)

/// A login handoff parsed from pasted text.
pub(super) struct LaunchToken {
    pub id_token: String,
    pub refresh_token: Option<String>,
    pub display_name: String,
    pub expires_in: u64,
}

/// Percent-decode a query component (`+` stays literal: tokens never contain
/// it and `encodeURIComponent` never emits it).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse pasted text into a launch token. Accepts the full
/// `partydungeon://login?...` link as well as the bare query string the web
/// page copies to the clipboard.
pub(super) fn parse_pasted_token(text: &str) -> Option<LaunchToken> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains("://") {
        parse_login_link(text)
    } else {
        parse_login_link(&format!("partydungeon://login?{text}"))
    }
}

/// Parse `partydungeon://login?token=…&refresh=…&name=…&exp=…`.
fn parse_login_link(uri: &str) -> Option<LaunchToken> {
    let (scheme, rest) = uri.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("partydungeon") {
        return None;
    }
    let (host, query) = match rest.split_once('?') {
        Some((host, query)) => (host, query),
        None => (rest, ""),
    };
    if !host.eq_ignore_ascii_case("login") {
        return None;
    }
    let mut token = None;
    let mut refresh = None;
    let mut name = String::new();
    let mut exp: u64 = 1800;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "token" => token = Some(percent_decode(value)),
            "refresh" => refresh = Some(percent_decode(value)),
            "name" => name = percent_decode(value),
            "exp" => {
                if let Ok(seconds) = value.parse() {
                    exp = seconds;
                }
            }
            _ => {}
        }
    }
    Some(LaunchToken {
        id_token: token.filter(|token| !token.is_empty())?,
        refresh_token: refresh.filter(|refresh| !refresh.is_empty()),
        display_name: name,
        expires_in: exp,
    })
}
