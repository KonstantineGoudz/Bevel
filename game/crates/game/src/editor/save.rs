//! save systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

pub(super) fn level_json(editor: &Editor, orbit: &Orbit, movers: &Movers) -> String {
    let pieces: Vec<String> = editor
        .pieces
        .iter()
        .map(|p| {
            let shape = &shapes()[p.shape];
            let name = editor
                .outline
                .label(p.id)
                .map(|name| {
                    format!(
                        ",\"name\":{}",
                        serde_json::to_string(name).expect("label serializes")
                    )
                })
                .unwrap_or_default();
            let group = editor
                .outline
                .piece_parent(p.id)
                .map(|group| format!(",\"group\":{group}"))
                .unwrap_or_default();
            let locked = if editor.outline.is_locked(p.id) {
                ",\"locked\":true"
            } else {
                ""
            };
            let interact = if p.interact.is_none() {
                String::new()
            } else {
                format!(",\"interact\":\"{}\"", p.interact.id())
            };
            // Trigger volumes are parametric: without their extents the copy
            // loads a default box. Only non-default configs are written, the
            // same rule the session snapshot uses.
            let trigger = if shape.is_parametric() {
                let size = model::sanitize_trigger_size(p.trigger_size);
                let alpha = if shape.is_glade() {
                    crate::glade::roof_pitch(p, shape)
                } else {
                    model::sanitize_trigger_alpha(p.trigger_alpha)
                };
                let form = if shape.is_glade() {
                    p.trigger_shape_override.id()
                } else {
                    model::piece_trigger_shape(p, shape).id()
                };
                let is_default = (size - model::TRIGGER_DEFAULT_SIZE).length_squared() < 1e-6
                    && form == "box"
                    && (alpha - model::TRIGGER_DEFAULT_ALPHA).abs() < 1e-6
                    && p.trigger_color == model::TRIGGER_DEFAULT_COLOR
                    && p.trigger_visible == model::TRIGGER_DEFAULT_VISIBLE;
                if is_default {
                    String::new()
                } else {
                    let mut extra = format!(
                        ",\"triggerWidth\":{:.3},\"triggerLength\":{:.3},\"triggerHeight\":{:.3}",
                        size.x, size.z, size.y,
                    );
                    if form != "box" {
                        extra += &format!(",\"triggerShape\":\"{form}\"");
                    }
                    if (alpha - model::TRIGGER_DEFAULT_ALPHA).abs() >= 1e-6 {
                        extra += &format!(",\"triggerAlpha\":{alpha:.3}");
                    }
                    if p.trigger_color != model::TRIGGER_DEFAULT_COLOR {
                        extra += &format!(
                            ",\"triggerColor\":\"{}\"",
                            model::trigger_color_to_hex_string(p.trigger_color)
                        );
                    }
                    if p.trigger_visible != model::TRIGGER_DEFAULT_VISIBLE {
                        extra += &format!(",\"triggerVisible\":{}", p.trigger_visible);
                    }
                    extra
                }
            } else {
                String::new()
            };
            format!(
                "{{\"id\":{},\"shape\":\"{}\",\"x\":{:.3},\"z\":{:.3},\"y\":{:.3},\"rotDeg\":{:.1},\"pitchDeg\":{:.1},\"rollDeg\":{:.1},\"dynamic\":{},\"damage\":{:.1}{interact}{trigger}{name}{group}{locked}}}",
                p.id,
                shape.id,
                p.pos.x,
                p.pos.y,
                p.elevation,
                p.rot.to_degrees(),
                p.pitch.to_degrees(),
                p.roll.to_degrees(),
                p.dynamic,
                p.trigger.damage
            )
        })
        .collect();
    let groups: Vec<String> = editor
        .outline
        .groups
        .iter()
        .map(|group| {
            format!(
                "{{\"id\":{},\"name\":{},\"parent\":{},\"collapsed\":{}}}",
                group.id,
                serde_json::to_string(&group.name).expect("group name serializes"),
                group
                    .parent
                    .map(|parent| parent.to_string())
                    .unwrap_or_else(|| "null".to_owned()),
                group.collapsed
            )
        })
        .collect();
    let character = editor
        .character
        .map(|c| {
            format!(
                ",\"character\":{{\"x\":{:.3},\"z\":{:.3},\"feet\":{:.3},\"yawDeg\":{:.1},\"hp\":{:.1},\"maxHp\":{:.1}}}",
                c.pos.x,
                c.pos.y,
                c.feet,
                c.yaw.to_degrees(),
                c.health.current,
                c.health.max,
            )
        })
        .unwrap_or_default();
    // Moving-platform paths, sorted by piece so the output is stable. These
    // are the "paths" a plain piece copy used to drop on the floor.
    let mut platform_ids: Vec<u32> = movers.map.keys().copied().collect();
    platform_ids.sort_unstable();
    let platforms: Vec<String> = platform_ids
        .iter()
        .filter_map(|id| movers.map.get(id).map(|mover| (*id, mover)))
        .map(|(id, mover)| {
            serde_json::to_string(&movers::to_saved(id, mover)).expect("platform serializes")
        })
        .collect();
    format!(
        "{{\"format\":\"party-dungeon-level\",\"version\":2,\"units\":\"metres\",\"hexSize\":{},\"nextId\":{},\"groups\":[{}],\"pieces\":[{}],\"platforms\":[{}]{character},\"camera\":{{\"targetX\":{:.3},\"targetY\":{:.3},\"targetZ\":{:.3},\"yawDeg\":{:.1},\"pitchDeg\":{:.1},\"distance\":{:.3},\"mode\":\"{}\"}}}}",
        model::HEX_SIZE,
        editor.next_id,
        groups.join(","),
        pieces.join(","),
        platforms.join(","),
        orbit.target.x,
        orbit.target.y,
        orbit.target.z,
        orbit.yaw.to_degrees(),
        orbit.pitch.to_degrees(),
        orbit.distance,
        orbit.mode.id(),
    )
}

/// Put the same level JSON that `S` writes on the system clipboard. This is a
/// debugging aid, so it copies the exact saved document rather than the larger
/// session snapshot used by the browser's autosave.
pub(super) fn copy_level_json(
    editor: &mut Editor,
    orbit: &Orbit,
    movers: &Movers,
    clipboard: &mut Clipboard,
) {
    let json = level_json(editor, orbit, movers);
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
    movers: Res<Movers>,
    mut last: ResMut<LastSaved>,
    mut clock: ResMut<SaveClock>,
    mut focus: MessageReader<WindowFocused>,
    home: Option<Res<HomeMenu>>,
) {
    // The home screen owns saving while open; autosave resumes in the editor.
    if home_modal_open(home) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    if editor.session.is_none() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if editor.level_name.is_none() {
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

    match persist(&editor, &orbit, &movers) {
        Ok(()) => record_save_success(&mut editor, &orbit, &mut last, &mut clock, now),
        Err(error) => {
            editor.note = save_failure_note(error);
            clock.retry_at = now + 2.0;
        }
    }
}

pub(super) fn persist(
    editor: &Editor,
    orbit: &Orbit,
    movers: &Movers,
) -> Result<(), session::StorageError> {
    #[cfg(target_arch = "wasm32")]
    {
        let id = editor
            .session
            .as_deref()
            .ok_or(session::StorageError::Unavailable)?;
        let snapshot = editor.capture(orbit, movers);
        let mut store = session::LocalStore::new()?;
        session::save_session(&mut store, id, &snapshot, session::now_ms())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(name) = editor.level_name.as_deref() else {
            return Err(session::StorageError::Unavailable);
        };
        let snapshot = editor.capture(orbit, movers);
        crate::session::save_native_level(name, &snapshot)
            .map(|_| ())
            .map_err(|_| session::StorageError::Unavailable)
    }
}

/// S-key manual save, split out of `interact` so that system stays under
/// Bevy's 16-parameter limit while still seeing platform paths.
pub(super) fn save_shortcut(
    input: PlayerInput,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    movers: Res<Movers>,
    mut last: ResMut<LastSaved>,
    mut clock: ResMut<SaveClock>,
    mut clipboard: ResMut<Clipboard>,
    time: Res<Time>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
    home: Option<Res<HomeMenu>>,
) {
    // A focused text field (properties, Add-menu search, home name field)
    // owns the keyboard: typing "s" there must not save.
    if focus.get().is_some() || menu.open || home_modal_open(home) {
        return;
    }
    if input.just_pressed(Action::CopyJson) {
        copy_level_json(&mut editor, &orbit, &movers, &mut clipboard);
    } else if input.just_pressed(Action::SaveLevel) {
        if object_editing_allowed(editor.mode) {
            save(
                &mut editor,
                &orbit,
                &movers,
                &mut last,
                &mut clock,
                time.elapsed_secs_f64(),
            );
        }
    }
}

pub(super) fn save(
    editor: &mut Editor,
    orbit: &Orbit,
    movers: &Movers,
    last: &mut LastSaved,
    clock: &mut SaveClock,
    now: f64,
) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(name) = editor.level_name.clone() else {
            editor.note = "no map open — open Maps to name it";
            return;
        };
        let snapshot = editor.capture(orbit, movers);
        match crate::session::save_native_level(&name, &snapshot) {
            Ok(path) => {
                info!("saved {} pieces to {}", editor.pieces.len(), path.display());
                record_save_success(editor, orbit, last, clock, now);
                editor.note = "saved";
            }
            Err(error) => warn!("could not save level: {error}"),
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        match persist(editor, orbit, movers) {
            Ok(()) => {
                record_save_success(editor, orbit, last, clock, now);
                editor.note = "saved";
            }
            Err(error) => editor.note = save_failure_note(error),
        }
    }
}
