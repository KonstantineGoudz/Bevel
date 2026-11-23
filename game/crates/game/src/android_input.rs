//! TEMPORARY Android input diagnostics: log every touch/keyboard event with
//! coordinates so a `logcat` capture shows exactly what the app receives while
//! the login screen ignores taps. Delete this module once fixed.
use bevy::input::keyboard::KeyboardInput;
use bevy::input::touch::{TouchInput, TouchPhase};
use bevy::input::ButtonState;
use bevy::picking::events::{PointerClick, PointerPress, PointerRelease};
use bevy::prelude::*;

pub(super) fn log_android_input(
    mut touches: MessageReader<TouchInput>,
    mut keys: MessageReader<KeyboardInput>,
    windows: Query<&Window>,
) {
    for touch in touches.read() {
        let (w, h) = windows
            .iter()
            .next()
            .map(|window| (window.width(), window.height()))
            .unwrap_or((0.0, 0.0));
        let phase = match touch.phase {
            TouchPhase::Started => "started",
            TouchPhase::Moved => "moved",
            TouchPhase::Ended => "ended",
            TouchPhase::Canceled => "canceled",
        };
        info!(
            "android-touch: id={} phase={} pos=({:.0},{:.0}) window={}x{} force={:?}",
            touch.id, phase, touch.position.x, touch.position.y, w, h, touch.force,
        );
    }
    for key in keys.read() {
        if key.state == ButtonState::Pressed {
            info!(
                "android-key: logical={:?} text={:?}",
                key.logical_key, key.text
            );
        }
    }
}

/// TEMPORARY: log every pointer press/click with its target entity, to see
/// which node (if any) actually receives taps on the auth form.
pub(super) fn log_pointer_down(
    event: On<PointerPress>,
    names: Query<(
        Option<&crate::auth::AuthButton>,
        Option<&crate::auth::AuthFieldKind>,
        Option<&Text>,
        Option<&Node>,
    )>,
) {
    let details = names.get(event.entity).ok();
    info!(
        "android-pointer: down target={:?} button={:?} details={:?}",
        event.entity, event.button, details
    );
}

/// TEMPORARY: log pointer ups the same way.
pub(super) fn log_pointer_up(event: On<PointerRelease>) {
    info!("android-pointer: up target={:?}", event.entity);
}

/// TEMPORARY: log pointer clicks the same way, plus whether the target
/// looks like an auth button (has Button) and whether it is pressed.
pub(super) fn log_pointer_click(
    event: On<PointerClick>,
    buttons: Query<Has<bevy::ui_widgets::Button>>,
    pressed: Query<Has<bevy::ui::Pressed>>,
) {
    let has_button = buttons.get(event.entity).unwrap_or(false);
    let is_pressed = pressed.get(event.entity).unwrap_or(false);
    info!(
        "android-pointer: click target={:?} button={} pressed={}",
        event.entity, has_button, is_pressed
    );
}
