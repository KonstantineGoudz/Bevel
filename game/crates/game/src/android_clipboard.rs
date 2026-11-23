//! System clipboard on Android.
//!
//! Bevy's `system_clipboard` feature pulls in `arboard`, which has no Android
//! backend, so Android builds compile Bevy without it and the editor uses the
//! [`Clipboard`] stub below (copy degrades to the existing "clipboard
//! unavailable" note). Reading, however, is implemented here over JNI: the
//! clipboard service only needs an application context, which
//! `ndk_context` provides — unlike `getIntent`, no `Activity` is required.
//!
//! Reading (not typing) is also how login works on Android for now: the
//! player logs in once in the system browser and pastes the app-login link
//! into the game (see `android_link`), because `NativeActivity` cannot summon
//! a soft keyboard for Bevy-drawn fields.
use std::fmt::Formatter;
use std::fmt::{self, Display};

use bevy::prelude::Resource;
use jni::objects::JObject;

/// Stub handle kept so editor code compiles unchanged on Android; only
/// [`Clipboard::get_text`] does real work, the rest mirrors the old stub.
#[derive(Clone, Debug, Default, Resource)]
pub struct Clipboard;

#[derive(Clone, Debug)]
pub struct ClipboardError(pub String);

impl Display for ClipboardError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClipboardError {}

impl Clipboard {
    pub fn set_text(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError(
            "clipboard unavailable on this platform".to_string(),
        ))
    }

    /// Read the current primary clip as text. `None` when the clipboard is
    /// empty, holds no text, or JNI is unusable — all mean "nothing to paste".
    pub fn get_text() -> Option<String> {
        read_primary_clip()
    }
}

fn read_primary_clip() -> Option<String> {
    let ctx = ndk_context::android_context();
    if ctx.vm().is_null() || ctx.context().is_null() {
        return None;
    }
    let out: Option<String> = (|| {
        let vm = unsafe { jni::JavaVM::from_raw(ctx.vm() as *mut jni::sys::JavaVM) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let context =
            unsafe { JObject::from_raw(ctx.context() as *mut jni::sys::_jobject) };
        let service = env
            .get_static_field(
                "android/content/Context",
                "CLIPBOARD_SERVICE",
                "Ljava/lang/String;",
            )
            .ok()?
            .l()
            .ok()?;
        let manager = env
            .call_method(
                &context,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[(&service).into()],
            )
            .ok()?
            .l()
            .ok()?;
        if manager.is_null() {
            return None;
        }
        let clip = env
            .call_method(
                &manager,
                "getPrimaryClip",
                "()Landroid/content/ClipData;",
                &[],
            )
            .ok()?
            .l()
            .ok()?;
        if clip.is_null() {
            return None;
        }
        if env.call_method(&clip, "getItemCount", "()I", &[]).ok()?.i().ok()? == 0 {
            return None;
        }
        let item = env
            .call_method(
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[0.into()],
            )
            .ok()?
            .l()
            .ok()?;
        let text = env
            .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])
            .ok()?
            .l()
            .ok()?;
        if text.is_null() {
            return None;
        }
        let text: String = env.get_string((&text).into()).ok()?.into();
        if text.is_empty() {
            return None;
        }
        Some(text)
    })();
    if out.is_none() {
        bevy::log::info!("android-clipboard: nothing text to paste");
    }
    out
}
