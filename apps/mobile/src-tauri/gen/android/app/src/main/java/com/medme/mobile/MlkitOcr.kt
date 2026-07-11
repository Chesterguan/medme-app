package com.medme.mobile

import android.graphics.BitmapFactory
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions

/**
 * On-device Chinese OCR via ML Kit Text Recognition v2 (bundled model → offline,
 * no Google Play download). Replaces the weaker PP-OCRv5 mobile model on Android.
 *
 * Called from Rust via JNI (`src/mlkit.rs`). Runs synchronously (`Tasks.await`),
 * so the caller MUST invoke it off the main thread — the Rust ingest thread does.
 * Any failure returns "" so the Rust side falls back to storing the file with no
 * extracted text (non-fatal), exactly like any other OCR failure.
 */
object MlkitOcr {
    @JvmStatic
    fun recognize(bytes: ByteArray): String {
        return try {
            val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return ""
            val image = InputImage.fromBitmap(bitmap, 0)
            val recognizer =
                TextRecognition.getClient(ChineseTextRecognizerOptions.Builder().build())
            Tasks.await(recognizer.process(image)).text
        } catch (e: Exception) {
            ""
        }
    }
}
