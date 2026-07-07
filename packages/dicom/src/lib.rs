//! DICOM (.dcm) support: metadata parsing + rendered PNG preview.
//!
//! v0.1 scope (see docs/010_Imaging_DICOM.md): parse the handful of tags MedMe
//! needs to file a DICOM instance as an imaging document (no OCR needed —
//! DICOM carries structured metadata), and render a single representative
//! frame to an 8-bit windowed grayscale PNG for viewing.

use anyhow::Context;
use dicom_object::{FileDicomObject, InMemDicomObject};
use dicom_pixeldata::{ConvertOptions, PixelDecoder};
use std::io::Cursor;

/// Metadata extracted from a DICOM instance's tags (no pixel decoding).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DicomMeta {
    /// Modality (0008,0060) — e.g. "CT", "MR", "US", "CR", "DX".
    pub modality: Option<String>,
    /// StudyDate (0008,0020), parsed from DICOM "YYYYMMDD" into an RFC3339
    /// UTC-midnight string (e.g. "2004-01-19T00:00:00+00:00").
    pub study_date: Option<String>,
    /// StudyDescription (0008,1030).
    pub description: Option<String>,
    /// BodyPartExamined (0018,0015).
    pub body_part: Option<String>,
    /// InstitutionName (0008,0080).
    pub institution: Option<String>,
    /// PatientName (0010,0010).
    pub patient_name: Option<String>,
    /// PatientSex (0010,0040).
    pub patient_sex: Option<String>,
    /// AccessionNumber (0008,0050).
    pub accession: Option<String>,
    /// StudyInstanceUID (0020,000D).
    pub study_uid: Option<String>,
}

/// Reads a named element as a trimmed, non-empty string, if present.
fn tag_str(obj: &FileDicomObject<InMemDicomObject>, name: &str) -> Option<String> {
    obj.element_by_name(name)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parses DICOM "YYYYMMDD" (StudyDate) into an RFC3339 UTC-midnight string.
/// Returns `None` if the input isn't exactly 8 digits or isn't a valid date.
fn parse_study_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = raw[0..4].parse().ok()?;
    let month: u32 = raw[4..6].parse().ok()?;
    let day: u32 = raw[6..8].parse().ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let dt = date.and_hms_opt(0, 0, 0)?;
    let utc = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc);
    Some(utc.to_rfc3339())
}

/// Parses the DICOM tags MedMe needs from a raw `.dcm` byte buffer.
///
/// Reads the standard on-disk structure (128-byte preamble + `DICM` magic +
/// file meta group + data set); preamble detection is automatic so this also
/// tolerates meta-group-only streams.
pub fn parse_meta(dcm_bytes: &[u8]) -> anyhow::Result<DicomMeta> {
    let obj = dicom_object::from_reader(Cursor::new(dcm_bytes))
        .context("failed to parse DICOM object")?;

    Ok(DicomMeta {
        modality: tag_str(&obj, "Modality"),
        study_date: tag_str(&obj, "StudyDate").and_then(|s| parse_study_date(&s)),
        description: tag_str(&obj, "StudyDescription"),
        body_part: tag_str(&obj, "BodyPartExamined"),
        institution: tag_str(&obj, "InstitutionName"),
        patient_name: tag_str(&obj, "PatientName"),
        patient_sex: tag_str(&obj, "PatientSex"),
        accession: tag_str(&obj, "AccessionNumber"),
        study_uid: tag_str(&obj, "StudyInstanceUID"),
    })
}

/// Decodes the first frame's pixel data and renders it as an 8-bit,
/// windowed (VOI LUT applied when present) grayscale PNG.
///
/// Errors if the object has no pixel data, or the pixel data can't be
/// decoded (e.g. an unsupported compressed transfer syntax).
pub fn render_png(dcm_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let obj = dicom_object::from_reader(Cursor::new(dcm_bytes))
        .context("failed to parse DICOM object")?;
    let pixel_data = obj
        .decode_pixel_data()
        .context("failed to decode DICOM pixel data")?;
    let opts = ConvertOptions::new().force_8bit();
    let image = pixel_data
        .to_dynamic_image_with_options(0, &opts)
        .context("failed to render DICOM pixel data to an image")?;

    let mut png_bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut png_bytes, image::ImageFormat::Png)
        .context("failed to encode PNG")?;
    Ok(png_bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/dicom/"
        ))
        .join(name);
        std::fs::read(&path)
            .unwrap_or_else(|e| panic!("failed to read sample {}: {e}", path.display()))
    }

    #[test]
    fn parses_ct_small_metadata() {
        let bytes = sample("CT_small.dcm");
        let meta = parse_meta(&bytes).unwrap();
        assert_eq!(meta.modality.as_deref(), Some("CT"));
        assert_eq!(meta.study_date.as_deref(), Some("2004-01-19T00:00:00+00:00"));
        assert_eq!(meta.institution.as_deref(), Some("JFK IMAGING CENTER"));
        assert_eq!(meta.patient_name.as_deref(), Some("CompressedSamples^CT1"));
        assert_eq!(meta.patient_sex.as_deref(), Some("O"));
        assert!(meta.study_uid.is_some());
    }

    #[test]
    fn parses_mr_small_metadata() {
        let bytes = sample("MR_small.dcm");
        let meta = parse_meta(&bytes).unwrap();
        assert_eq!(meta.modality.as_deref(), Some("MR"));
        assert_eq!(meta.study_date.as_deref(), Some("2004-08-26T00:00:00+00:00"));
        assert_eq!(meta.institution.as_deref(), Some("TOSHIBA"));
        assert_eq!(meta.patient_sex.as_deref(), Some("F"));
    }

    #[test]
    fn renders_ct_small_to_valid_png() {
        let bytes = sample("CT_small.dcm");
        let png = render_png(&bytes).unwrap();
        assert!(!png.is_empty());
        let img = image::load_from_memory(&png).unwrap();
        use image::GenericImageView;
        assert_eq!(img.dimensions(), (128, 128));
    }

    #[test]
    fn renders_mr_small_to_valid_png() {
        let bytes = sample("MR_small.dcm");
        let png = render_png(&bytes).unwrap();
        assert!(!png.is_empty());
        let img = image::load_from_memory(&png).unwrap();
        use image::GenericImageView;
        assert_eq!(img.dimensions(), (64, 64));
    }

    #[test]
    fn parse_study_date_rejects_malformed_input() {
        assert_eq!(parse_study_date("20040119"), Some("2004-01-19T00:00:00+00:00".to_string()));
        assert_eq!(parse_study_date(""), None);
        assert_eq!(parse_study_date("2004-01-19"), None);
        assert_eq!(parse_study_date("99999999"), None);
    }

    #[test]
    fn parse_meta_errors_on_garbage_bytes() {
        assert!(parse_meta(b"not a dicom file").is_err());
    }
}
