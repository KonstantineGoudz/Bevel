//! save systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

pub(super) fn level_json(editor: &Editor) -> String {
    let pieces: Vec<String> = editor
        .pieces
        .iter()
        .map(|p| {
            format!(
                "{{\"shape\":\"{}\",\"x\":{:.3},\"z\":{:.3},\"y\":{:.3},\"rotDeg\":{:.1},\"pitchDeg\":{:.1},\"rollDeg\":{:.1}}}",
                shapes()[p.shape].id,
                p.pos.x,
                p.pos.y,
                p.elevation,
                p.rot.to_degrees(),
                p.pitch.to_degrees(),
                p.roll.to_degrees()
            )
        })
        .collect();
    let character = editor
        .character
        .map(|c| {
            format!(
                ",\"character\":{{\"x\":{:.3},\"z\":{:.3},\"yawDeg\":{:.1}}}",
                c.pos.x,
                c.pos.y,
                c.yaw.to_degrees()
            )
        })
        .unwrap_or_default();
    format!(
        "{{\"format\":\"party-dungeon-level\",\"version\":1,\"units\":\"metres\",\"hexSize\":{},\"pieces\":[{}]{character}}}",
        model::HEX_SIZE,
        pieces.join(",")
    )
}

/// Put the same level JSON that `S` writes on the system clipboard. This is a
/// debugging aid, so it copies the exact saved document rather than the larger
/// session snapshot used by the browser's autosave.
pub(super) fn copy_level_json(editor: &mut Editor, clipboard: &mut Clipboard) {
    let json = level_json(editor);
    match copy_to_clipboard(&json, clipboard) {
        Ok(()) => editor.note = "copied level JSON",
        Err(()) => {
            warn!("could not copy level JSON; here it is: {json}");
            editor.note = "copy failed: clipboard unavailable";
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn copy_to_clipboard(text: &str, clipboard: &mut Clipboard) -> Result<(), ()> {
    clipboard.set_text(text).map_err(|error| {
        warn!("could not write to the clipboard: {error}");
    })
}

/// The Clipboard API only exists in a secure context. The dev server is often
/// opened over plain HTTP on a phone's LAN address, where `navigator.clipboard`
/// is undefined; fall back to the old textarea-and-`execCommand` path there so
/// copying still works while debugging on a real device.
#[cfg(target_arch = "wasm32")]
pub(super) fn copy_to_clipboard(text: &str, clipboard: &mut Clipboard) -> Result<(), ()> {
    if web_clipboard_available() {
        return clipboard.set_text(text).map_err(|error| {
            warn!("could not write to the clipboard: {error}");
        });
    }
    if exec_command_copy(text) {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn web_clipboard_available() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    if !window.is_secure_context() {
        return false;
    }
    let navigator = wasm_bindgen::JsValue::from(window.navigator());
    match js_sys::Reflect::get(&navigator, &wasm_bindgen::JsValue::from_str("clipboard")) {
        Ok(clipboard) => !clipboard.is_undefined() && !clipboard.is_null(),
        Err(_) => false,
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn exec_command_copy(text: &str) -> bool {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return false;
    };
    let Some(document) = window.document() else {
        return false;
    };
    let Ok(element) = document.create_element("textarea") else {
        return false;
    };
    let Ok(textarea) = element.dyn_into::<web_sys::HtmlTextAreaElement>() else {
        return false;
    };
    textarea.set_value(text);
    let _ = textarea.set_attribute("readonly", "");
    let _ = textarea.set_attribute(
        "style",
        "position:fixed;top:-1000px;left:-1000px;opacity:0;",
    );
    let Some(parent) = document.document_element() else {
        return false;
    };
    if parent.append_child(&textarea).is_err() {
        return false;
    }
    let _ = textarea.focus();
    textarea.select();
    let copied = document
        .dyn_into::<web_sys::HtmlDocument>()
        .ok()
        .and_then(|document| document.exec_command("copy").ok())
        .unwrap_or(false);
    textarea.remove();
    copied
}

pub(super) fn save_due(hard: bool, soft: bool, last_save: f64, now: f64, retry_at: f64) -> bool {
    now >= retry_at && (hard || (soft && now - last_save >= session::SOFT_SAVE_INTERVAL))
}

pub(super) fn soft_changed(seen: &Seen, orbit: &Orbit, character: Option<Character>) -> bool {
    &seen.orbit != orbit
        || match (seen.character, character) {
            (Some(seen), Some(current)) => {
                seen.pos != current.pos || seen.feet != current.feet || seen.yaw != current.yaw
            }
            (None, None) => false,
            _ => true,
        }
}

pub(super) fn save_failure_note(error: session::StorageError) -> &'static str {
    match error {
        session::StorageError::QuotaExceeded => "save failed: storage is full",
        session::StorageError::Unavailable => "save failed: storage unavailable",
        session::StorageError::IndexMissing => "save failed: saved map list is invalid",
    }
}

pub(super) fn record_save_success(
    editor: &mut Editor,
    orbit: &Orbit,
    last: &mut LastSaved,
    clock: &mut SaveClock,
    now: f64,
) {
    editor.hard_dirty = false;
    editor.soft_dirty = false;
    if matches!(
        editor.note,
        "save failed: storage is full"
            | "save failed: storage unavailable"
            | "save failed: saved map list is invalid"
    ) {
        editor.note = "saved";
    }
    clock.last_save = now;
    clock.retry_at = 0.0;
    last.0 = Some(Seen {
        orbit: orbit.clone(),
        character: editor.character,
    });
}

pub(super) fn autosave_session(
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    mut last: ResMut<LastSaved>,
    mut clock: ResMut<SaveClock>,
    mut focus: MessageReader<WindowFocused>,
) {
    if editor.session.is_none() {
        return;
    }

    if focus.read().any(|event| !event.focused) {
        editor.hard_dirty = true;
    }

    let now = time.elapsed_secs_f64();
    let character = editor.character;
    let Some(seen) = last.0.as_ref() else {
        last.0 = Some(Seen {
            orbit: orbit.clone(),
            character,
        });
        return;
    };

    if soft_changed(seen, &orbit, character) {
        editor.soft_dirty = true;
    }
    if !save_due(
        editor.hard_dirty,
        editor.soft_dirty,
        clock.last_save,
        now,
        clock.retry_at,
    ) {
        return;
    }

    match persist(&editor, &orbit) {
        Ok(()) => record_save_success(&mut editor, &orbit, &mut last, &mut clock, now),
        Err(error) => {
            editor.note = save_failure_note(error);
            clock.retry_at = now + 2.0;
        }
    }
}

pub(super) fn persist(editor: &Editor, orbit: &Orbit) -> Result<(), session::StorageError> {
    #[cfg(target_arch = "wasm32")]
    {
        let id = editor
            .session
            .as_deref()
            .ok_or(session::StorageError::Unavailable)?;
        let snapshot = editor.capture(orbit);
        let mut store = session::LocalStore::new()?;
        session::save_session(&mut store, id, &snapshot, session::now_ms())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (editor, orbit);
        Ok(())
    }
}

pub(super) fn save(
    editor: &mut Editor,
    orbit: &Orbit,
    last: &mut LastSaved,
    clock: &mut SaveClock,
    now: f64,
) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = orbit;
        let json = level_json(editor);
        let path = std::path::Path::new("level.json");
        match std::fs::write(path, &json) {
            Ok(()) => {
                info!("saved {} pieces to level.json", editor.pieces.len());
                record_save_success(editor, orbit, last, clock, now);
                editor.note = "saved level.json";
            }
            Err(err) => warn!("could not save level.json: {err}"),
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        match persist(editor, orbit) {
            Ok(()) => {
                record_save_success(editor, orbit, last, clock, now);
                editor.note = "saved";
            }
            Err(error) => editor.note = save_failure_note(error),
        }
    }
}
