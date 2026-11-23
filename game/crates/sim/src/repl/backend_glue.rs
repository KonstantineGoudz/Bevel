//! The only module that talks to the `party-dungeon-repl-matchbox` backend API.
//!
//! Kept deliberately thin: the backend authenticates the JWT handshake, spawns
//! `ConnectedClient` + `ClientSession` entities and owns the channel pump. This
//! module supplies the JWT [`Authenticator`] and translates the backend's client
//! lifecycle into authority [`ClientIntent`]s — everything else in the embedded
//! app is backend-agnostic.
//!
//! `ClientSession` is re-exported so the rest of the replicon module never has
//! to spell out the backend crate (and the `server` feature gate) again.

use bevy::prelude::*;

pub use party_dungeon_repl_matchbox::ClientSession;

use party_dungeon_repl_matchbox::{
    Authenticator, BackendAuthenticator, MatchboxServerPlugin, ServerBackendConfig, Session,
};

use crate::authority::verify_token;
use crate::repl::entities::{ReplBridge, SessionEntities};
use crate::repl::protocol::ClientIntent;

/// JWT authenticator for the replicon handshake.
///
/// Uses the exact same claim decoder/gate as the legacy `ClientMsg::Hello`
/// path ([`crate::authority::verify_token`]): decode `sub`, let SurrealDB's row
/// rules enforce `iss`/`aud` on every write. The display name is claimed by the
/// client *after* replication starts (the handshake only carries the token), so
/// the backend session starts nameless and the authority resumes the row's name.
pub struct JwtAuthenticator;

impl Authenticator for JwtAuthenticator {
    fn authenticate(&self, token: &str) -> Result<Session, String> {
        let sub = verify_token(token)?;
        Ok(Session {
            sub,
            name: String::new(),
        })
    }
}

/// Build the backend plugin (with our JWT authenticator resource).
pub fn server_plugins(app: &mut App, config: ServerBackendConfig, authenticator: Option<BackendAuthenticator>) {
    app.add_plugins(MatchboxServerPlugin)
        .insert_resource(config);
    if let Some(authenticator) = authenticator {
        app.insert_resource(authenticator);
    }
}

/// `BackendAuthenticator` wrapping the JWT verifier.
pub fn jwt_authenticator() -> BackendAuthenticator {
    BackendAuthenticator(Box::new(JwtAuthenticator))
}

/// A client finished the handshake and is authorized: register its sub → entity
/// mapping and ask the authority to persist/join the player row.
pub fn on_client_session_added(
    added: On<Add<ClientSession>>,
    clients: Query<&ClientSession>,
    mut sessions: ResMut<SessionEntities>,
    bridge: Res<ReplBridge>,
) {
    let client = added.entity;
    let Ok(session) = clients.get(client) else {
        return;
    };
    eprintln!(
        "sim: replicon client authorized sub={} entity={client:?}",
        session.sub
    );
    sessions.insert_session(session.sub.clone(), client);
    // Join takes the claimed name; an empty handshake name means "resume the
    // stored row's display name".
    let _ = bridge.intents.send(ClientIntent::Join {
        sub: session.sub.clone(),
        name: session.name.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_authenticator_accepts_a_valid_hs256_token() {
        // Same dev token shape the legacy path and wire probes use.
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1aWQtMSIsImlzcyI6ImRldiIsImF1ZCI6InBhcnR5LWR1bmdlb24iLCJleHAiOjQxMDI0NDQ4MDB9.c2ln";
        let session = JwtAuthenticator.authenticate(token).expect("valid token");
        assert_eq!(session.sub, "uid-1");
        assert!(session.name.is_empty());
    }

    #[test]
    fn jwt_authenticator_rejects_garbage() {
        assert!(JwtAuthenticator.authenticate("not-a-jwt").is_err());
        assert!(JwtAuthenticator.authenticate("a.b.c").is_err());
    }
}
