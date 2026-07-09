//! Imaging Study→Series→Instance model (see docs/014_Imaging_Overhaul.md, P1).
//!
//! A multi-slice DICOM scan (CT/MR) becomes ONE scrollable imaging-study
//! document rather than N separate documents. The first slice of a study
//! creates the document (normal `DocumentAdded`); every later slice of the
//! same `StudyInstanceUID` attaches as an `imaging_instance` row via the
//! `ImagingInstanceAdded` event — no new document. Instances are stored/ordered
//! by (series_number, instance_number): the slice-stack order.

use crate::event::{DocRef, Event};
use crate::types::{ImagingInstance, NewImagingInstance};
use crate::{MedmeError, Vault};
use rusqlite::OptionalExtension;

impl Vault {
    /// Find the existing imaging-study document for a `StudyInstanceUID`, if any.
    /// Used by the ingest pipeline to decide "new study document" vs "attach
    /// slice to existing study".
    pub fn document_id_for_study(&self, study_uid: &str) -> Result<Option<i64>, MedmeError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM document WHERE study_uid = ?1",
                [study_uid],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// If this source_file is already attached as an imaging slice, return the
    /// study document it belongs to. Lets the ingest pipeline treat a
    /// re-imported sibling slice (which has no document of its own) as a dedup.
    pub fn imaging_document_for_source(
        &self,
        source_file_id: i64,
    ) -> Result<Option<i64>, MedmeError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT document_id FROM imaging_instance WHERE source_file_id = ?1",
                [source_file_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Attach a DICOM slice to a study document: append `ImagingInstanceAdded`
    /// (referencing both rows by content hash, DB-independently) and materialize.
    /// Also stamps `study_uid` onto the document on first use. Returns the new
    /// `imaging_instance` row id.
    pub fn add_imaging_instance(&self, i: NewImagingInstance) -> Result<i64, MedmeError> {
        let doc = self
            .document_by_id(i.document_id)?
            .ok_or_else(|| MedmeError::Other(format!("document {} not found", i.document_id)))?;
        let anchor = self.source_file_by_id(doc.source_file_id)?.ok_or_else(|| {
            MedmeError::Other(format!("source_file {} not found", doc.source_file_id))
        })?;
        let inst_sf = self.source_file_by_id(i.source_file_id)?.ok_or_else(|| {
            MedmeError::Other(format!("source_file {} not found", i.source_file_id))
        })?;
        let now = Self::now_rfc3339();
        let (document_id, source_file_id) = (i.document_id, i.source_file_id);
        self.append_event(Event::ImagingInstanceAdded {
            document_ref: DocRef {
                source_file_hash: anchor.content_hash,
            },
            source_file_hash: inst_sf.content_hash,
            study_uid: i.study_uid,
            series_uid: i.series_uid,
            series_number: i.series_number,
            instance_number: i.instance_number,
            created_at: now,
        })?;
        self.materialize()?;
        let id: i64 = self.conn().query_row(
            "SELECT id FROM imaging_instance WHERE document_id = ?1 AND source_file_id = ?2",
            rusqlite::params![document_id, source_file_id],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// Number of DICOM slices attached to a study document (0 for non-imaging
    /// documents). Cheap count for timeline cards ("N 张切片").
    pub fn imaging_instance_count(&self, document_id: i64) -> Result<i64, MedmeError> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) FROM imaging_instance WHERE document_id = ?1",
            [document_id],
            |r| r.get(0),
        )?)
    }

    /// All slices of a study document, in slice-stack order:
    /// (series_number, instance_number) ascending, NULLs last, id as tiebreak.
    pub fn imaging_instances(&self, document_id: i64) -> Result<Vec<ImagingInstance>, MedmeError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, document_id, source_file_id, series_uid, series_number, instance_number
             FROM imaging_instance WHERE document_id = ?1
             ORDER BY series_number IS NULL, series_number ASC,
                      instance_number IS NULL, instance_number ASC, id ASC",
        )?;
        let rows = stmt.query_map([document_id], |r| {
            Ok(ImagingInstance {
                id: r.get(0)?,
                document_id: r.get(1)?,
                source_file_id: r.get(2)?,
                series_uid: r.get(3)?,
                series_number: r.get(4)?,
                instance_number: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{NewDocument, NewImagingInstance};
    use crate::{DocType, Vault};

    /// Create a study document + attach 3 slices out of order → they come back
    /// ordered by (series_number, instance_number), and study→document lookup works.
    #[test]
    fn study_doc_groups_ordered_instances() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        // First slice's source_file anchors the study document.
        let imp0 = v
            .import("slice_02.dcm", "application/dicom", b"dcm-anchor")
            .unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp0.source_file.id,
                doc_type: DocType::ImagingReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("头颅CT".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();

        let study = "1.2.3.STUDY";
        // No study_uid on the document yet — first instance stamps it.
        assert_eq!(v.document_id_for_study(study).unwrap(), None);

        // Attach slices out of order: instance 2, then 1, then 3.
        let mk = |name: &str, body: &[u8], inst: i32| {
            let imp = v.import(name, "application/dicom", body).unwrap();
            v.add_imaging_instance(NewImagingInstance {
                document_id: doc.id,
                source_file_id: imp.source_file.id,
                study_uid: study.into(),
                series_uid: Some("1.2.3.SERIES".into()),
                series_number: Some(1),
                instance_number: Some(inst),
            })
            .unwrap();
        };
        mk("slice_02.dcm", b"dcm-anchor", 2); // reuse anchor source_file as slice 2
        mk("slice_01.dcm", b"dcm-1", 1);
        mk("slice_03.dcm", b"dcm-3", 3);

        // Lookup now resolves the study to this document.
        assert_eq!(v.document_id_for_study(study).unwrap(), Some(doc.id));
        assert_eq!(
            v.document_by_id(doc.id).unwrap().unwrap().source_file_id,
            imp0.source_file.id
        );

        let insts = v.imaging_instances(doc.id).unwrap();
        assert_eq!(insts.len(), 3);
        let order: Vec<i32> = insts.iter().map(|i| i.instance_number.unwrap()).collect();
        assert_eq!(order, vec![1, 2, 3], "ordered by instance_number");
    }

    /// CRITICAL: rebuild_from_log after adding imaging instances reproduces
    /// identical state (mirrors the audit-event rebuild test).
    #[test]
    fn rebuild_from_log_reproduces_imaging_instances() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let imp0 = v.import("s1.dcm", "application/dicom", b"a").unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp0.source_file.id,
                doc_type: DocType::ImagingReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("头颅CT".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        let study = "1.2.3.STUDY";
        for (i, body) in [b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]
            .iter()
            .enumerate()
        {
            let imp = v
                .import(&format!("s{i}.dcm"), "application/dicom", body)
                .unwrap();
            v.add_imaging_instance(NewImagingInstance {
                document_id: doc.id,
                source_file_id: imp.source_file.id,
                study_uid: study.into(),
                series_uid: Some("1.2.3.SERIES".into()),
                series_number: Some(1),
                instance_number: Some(3 - i as i32),
            })
            .unwrap();
        }

        let before_insts = v.imaging_instances(doc.id).unwrap();
        let before_lookup = v.document_id_for_study(study).unwrap();
        let before_ii_count = v.debug_count("imaging_instance");
        let before_study_uid = v.document_by_id(doc.id).unwrap().unwrap().source_file_id;

        v.rebuild_from_log().unwrap();

        assert_eq!(v.imaging_instances(doc.id).unwrap(), before_insts);
        assert_eq!(v.document_id_for_study(study).unwrap(), before_lookup);
        assert_eq!(v.debug_count("imaging_instance"), before_ii_count);
        assert_eq!(
            v.document_by_id(doc.id).unwrap().unwrap().source_file_id,
            before_study_uid
        );
    }
}
