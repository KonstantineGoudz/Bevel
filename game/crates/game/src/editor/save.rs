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

