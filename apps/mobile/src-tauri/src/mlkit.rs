//! Android ML Kit OCR bridge (Android only).
//!
//! On Android the Rust `oar-ocr` / PP-OCRv5 mobile model recognizes Chinese
//! medical records poorly. Google **ML Kit Text Recognition v2** (Chinese,
//! bundled model → offline) does far better. ML Kit is a Java/Kotlin SDK, so
//! Rust calls it over **JNI**: the Kotlin `MlkitOcr.recognize(ByteArray): String`
//! (see `gen/android/.../MlkitOcr.kt`) is invoked with the image bytes and
//! returns the recognized text.
//!
//! Mirrors the iOS Apple Vision bridge (`vision.rs`) at the ingest layer, but
//! over JNI instead of C-ABI (Android runs on the JVM). Compiled out on every
//! non-Android target.

use anyhow::{Context, Result};
use jni::objects::{JString, JValue};
use jni::{jni_sig, jni_str};

/// Recognize text in raw encoded image bytes via ML Kit (blocks until done).
/// Returns "" when ML Kit finds nothing or errors (caller treats as no text).
pub fn recognize(bytes: &[u8]) -> Result<String> {
    // The JavaVM is stashed by the Android runtime (ndk-glue / Tauri) — reuse it.
    // `JavaVM::from_raw` in jni 0.22 is infallible; it just wraps the pointer.
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) };

    // jni 0.22 attaches the thread via a callback that receives `&mut Env`.
    vm.attach_current_thread(|env| -> Result<String> {
        let bytes_array = env
            .byte_array_from_slice(bytes)
            .context("byte_array_from_slice")?;
        let result = env
            .call_static_method(
                jni_str!("com/medme/mobile/MlkitOcr"),
                jni_str!("recognize"),
                jni_sig!("([B)Ljava/lang/String;"),
                &[JValue::Object(bytes_array.as_ref())],
            )
            .context("call MlkitOcr.recognize")?;
        let jstr = env
            .cast_local::<JString>(result.l().context("result not an object")?)
            .context("cast result to JString")?;
        let text = jstr.mutf8_chars(env)?.to_string();
        Ok(text)
    })
}
