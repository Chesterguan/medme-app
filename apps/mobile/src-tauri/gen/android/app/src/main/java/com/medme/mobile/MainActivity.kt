package com.medme.mobile

import android.os.Bundle
import android.util.Log
import androidx.activity.enableEdgeToEdge
import java.io.File

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // Extract the bundled OCR models before the Rust layer starts, so the
    // engine can find them on the first ingest.
    copyOcrModels()
    super.onCreate(savedInstanceState)
  }

  /**
   * Copies the PP-OCRv5 model files bundled in the APK (under `assets/oar`) into
   * `<dataDir>/oar` so the Rust `oar-ocr` ONNX Runtime engine can load them by
   * filesystem path. Android assets live inside the APK zip and are not real
   * files; ONNX Runtime needs an on-disk path, hence this one-time extraction.
   *
   * The Rust side reads the same directory: `app_data_dir()` resolves to
   * `Context.getDataDir()` (the same `dataDir` used here), and it appends `oar`.
   *
   * Idempotent and crash-safe: each file is streamed to a `.tmp` sibling then
   * atomically renamed, and files already present with non-zero length are
   * skipped. Any failure is logged and swallowed — OCR then stays unavailable
   * and ingest falls back to storing files without extracted text, which is
   * non-fatal.
   */
  private fun copyOcrModels() {
    val names = listOf(
      "pp-ocrv5_mobile_det.onnx",
      "pp-ocrv5_mobile_rec.onnx",
      "ppocrv5_dict.txt",
    )
    try {
      val outDir = File(dataDir, "oar")
      if (!outDir.exists()) {
        outDir.mkdirs()
      }
      for (name in names) {
        val outFile = File(outDir, name)
        if (outFile.exists() && outFile.length() > 0L) {
          continue
        }
        val tmp = File(outDir, "$name.tmp")
        assets.open("oar/$name").use { input ->
          tmp.outputStream().use { output ->
            input.copyTo(output)
          }
        }
        if (!tmp.renameTo(outFile)) {
          // Fallback: copy over any partial and drop the temp.
          tmp.copyTo(outFile, overwrite = true)
          tmp.delete()
        }
      }
    } catch (e: Exception) {
      Log.e("MedMe", "OCR model extraction failed", e)
    }
  }
}
