//! Windows on-device OCR via **Windows.Media.Ocr** (WinRT), pure Rust through
//! the `windows` crate. PRIMARY recognizer on the Windows desktop build;
//! `recognize` falls back to oar-ocr / PP-OCRv5 if the OCR engine (Chinese
//! language pack) is unavailable or finds nothing. On-device + offline.
//!
//! NOTE: Windows.Media.Ocr needs the target language's OCR pack. Chinese Windows
//! ships zh-Hans OCR; on an English install it may be absent, in which case
//! `TryCreateFromLanguage` fails and we fall back to the user's profile
//! languages, then to the engine. Cannot be compiled or tested on macOS — this
//! module is verified by the Windows CI build + a Windows tester (see #41).

use crate::OcrOutcome;
use anyhow::{Context, Result};
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

/// Recognize text in raw encoded image bytes (JPEG/PNG/TIFF/…) via Windows OCR.
pub fn recognize_bytes(image_bytes: &[u8]) -> Result<OcrOutcome> {
    // Raw bytes → in-memory random-access stream that BitmapDecoder can read.
    let stream = InMemoryRandomAccessStream::new().context("InMemoryRandomAccessStream::new")?;
    let output = stream.GetOutputStreamAt(0).context("GetOutputStreamAt")?;
    let writer = DataWriter::CreateDataWriter(&output).context("CreateDataWriter")?;
    writer.WriteBytes(image_bytes).context("WriteBytes")?;
    writer
        .StoreAsync()
        .context("StoreAsync")?
        .get()
        .context("StoreAsync.get")?;
    writer
        .FlushAsync()
        .context("FlushAsync")?
        .get()
        .context("FlushAsync.get")?;
    let _ = writer.DetachStream();
    stream.Seek(0).context("Seek")?;

    // Decode (any WIC-supported format) → SoftwareBitmap.
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .context("BitmapDecoder::CreateAsync")?
        .get()
        .context("BitmapDecoder.get")?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .context("GetSoftwareBitmapAsync")?
        .get()
        .context("SoftwareBitmap.get")?;

    // Prefer Simplified Chinese; fall back to the user's profile languages.
    let engine = OcrEngine::TryCreateFromLanguage(
        &Language::CreateLanguage(&HSTRING::from("zh-Hans")).context("Language::CreateLanguage")?,
    )
    .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())
    .context("no Windows OCR engine available (Chinese OCR language pack missing?)")?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .context("RecognizeAsync")?
        .get()
        .context("OcrResult.get")?;
    let text = result.Text().context("OcrResult.Text")?.to_string_lossy();

    Ok(OcrOutcome {
        // Windows.Media.Ocr exposes no confidence; report a fixed moderate value
        // when text was found, 0.0 otherwise.
        confidence: if text.trim().is_empty() { 0.0 } else { 0.85 },
        text,
    })
}
