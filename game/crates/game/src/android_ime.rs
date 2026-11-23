//! Android soft keyboard, forced open.
//!
//! [`NativeActivity`] can only *ask* for the keyboard (`showSoftInput` with
//! the implicit flag, which is what winit forwards from [`Window::ime_enabled`]).
//! The IME is free to ignore an implicit request when no editable view holds
//! focus — and nothing here is an editable view, so on this device the request
//! binds the IME (`startInput` runs) but the keyboard never appears.
//!
//! `InputMethodManager.toggleSoftInput(SHOW_FORCED, _)` is the standard game
//! workaround: it opens the keyboard unconditionally, and Gboard then delivers
//! ordinary key events, which `auth_field_typing` already turns into text
//! (including Enter-to-submit). Hiding goes through the decor view's window
//! token. Everything is best-effort: any JNI failure just logs and the login
//! form keeps its current behavior.
//!
//! [`NativeActivity`]: https://developer.android.com/games/agdk/native-activity
//! [`Window::ime_enabled`]: bevy::window::Window::ime_enabled
use jni::objects::JObject;
use jni::sys::jint;
use jni::JavaVM;

const SHOW_FORCED: jint = 2;

/// Run `f` with the app's `InputMethodManager`, if JNI is usable right now.
fn with_imm<T>(f: impl FnOnce(&mut jni::JNIEnv, JObject) -> jni::errors::Result<T>) -> Option<T> {
    let ctx = ndk_context::android_context();
    if ctx.vm().is_null() || ctx.context().is_null() {
        return None;
    }
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut jni::sys::JavaVM) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let result = (|| {
        let activity = unsafe { JObject::from_raw(ctx.context() as *mut jni::sys::_jobject) };
        let service = env.get_static_field(
            "android/content/Context",
            "INPUT_METHOD_SERVICE",
            "Ljava/lang/String;",
        )?;
        let imm = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[service.borrow()],
            )?
            .l()?;
        let out = f(&mut env, imm)?;
        if env.exception_check()? {
            env.exception_clear()?;
            return Err(jni::errors::Error::JavaException);
        }
        Ok(out)
    })();
    match result {
        Ok(out) => Some(out),
        Err(error) => {
            bevy::log::warn!("android keyboard: JNI call failed: {error:?}");
            None
        }
    }
}

/// Ask for the soft keyboard via `toggleSoftInput`.
pub(super) fn show_keyboard_forced() {
    bevy::log::info!("android keyboard: show_keyboard_forced");
    if let Some(app) = bevy::android::ANDROID_APP.get() {
        app.show_soft_input(false);
    }
    let shown = with_imm(|env, imm| {
        env.call_method(
            &imm,
            "toggleSoftInput",
            "(II)V",
            &[SHOW_FORCED.into(), 0.into()],
        )?;
        Ok(())
    });
    if shown.is_none() {
        bevy::log::warn!("android keyboard: forced show failed");
    }
}

/// Hide the soft keyboard, if it is up.
pub(super) fn hide_keyboard() {
    bevy::log::info!("android keyboard: hide_keyboard");
    // NativeActivity's hide_soft_input queries the decor view token and calls
    // hideSoftInputFromWindow(token, 0). Unlike toggleSoftInput, this is idempotent
    // and will never accidentally open the keyboard if it is already closed.
    if let Some(app) = bevy::android::ANDROID_APP.get() {
        app.hide_soft_input(false);
    }
}
