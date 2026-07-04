use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

const ALLOWED_STATUS: &[&str] = &["candidate", "bundled"];
const ALLOWED_SPDX: &[&str] = &["Apache-2.0", "MIT OR Apache-2.0"];

#[test]
fn weights_license_manifest_is_complete() {
    let root = repo_root();
    let manifest_path = root.join("licenses/model-weights.tsv");
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "{} must exist and be readable: {e}",
            manifest_path.display()
        )
    });

    let mut rows = 0usize;
    let mut models = HashSet::new();
    for (index, raw_line) in manifest.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = raw_line.split('\t').collect();
        assert_eq!(
            cols.len(),
            8,
            "licenses/model-weights.tsv:{line_no} must have 8 tab-separated columns"
        );

        let model = cols[0].trim();
        let status = cols[1].trim();
        let spdx = cols[2].trim();
        let license_doc = cols[3].trim();
        let source_url = cols[4].trim();
        let pinned_ref = cols[5].trim();
        let source_sha256 = cols[6].trim();
        let bundled_sha256 = cols[7].trim();

        assert!(!model.is_empty(), "line {line_no}: model id is required");
        assert!(
            models.insert(model.to_string()),
            "line {line_no}: duplicate model id {model}"
        );
        assert!(
            ALLOWED_STATUS.contains(&status),
            "line {line_no}: status {status:?} must be one of {ALLOWED_STATUS:?}"
        );
        assert!(
            ALLOWED_SPDX.contains(&spdx),
            "line {line_no}: SPDX expression {spdx:?} is not approved for bundled weights"
        );
        assert!(
            source_url.starts_with("https://"),
            "line {line_no}: source URL must be an https URL"
        );
        assert!(
            !pinned_ref.is_empty() && pinned_ref != "TBD",
            "line {line_no}: upstream commit/revision must be pinned"
        );
        assert!(
            is_sha256(source_sha256),
            "line {line_no}: source artifact SHA-256 is required"
        );

        let doc_path = root.join(license_doc);
        let doc = fs::read_to_string(&doc_path)
            .unwrap_or_else(|e| panic!("{} must exist and be readable: {e}", doc_path.display()));
        assert!(
            doc.contains("Distribution decision:"),
            "{} must state a distribution decision",
            doc_path.display()
        );
        assert!(
            doc.contains("Source evidence:"),
            "{} must cite source evidence",
            doc_path.display()
        );

        if status == "bundled" {
            assert!(
                is_sha256(bundled_sha256),
                "line {line_no}: bundled weights must have a shipped-artifact SHA-256"
            );
        } else {
            assert_eq!(
                bundled_sha256, "TBD",
                "line {line_no}: candidate rows must not pretend to have a shipped hash"
            );
        }

        rows += 1;
    }

    assert!(rows > 0, "weights manifest must list at least one model");

    let licenses_dir = root.join("licenses");
    for entry in fs::read_dir(&licenses_dir)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", licenses_dir.display()))
    {
        let entry = entry.expect("license directory entry must be readable");
        if !entry
            .file_type()
            .expect("file type must be readable")
            .is_dir()
        {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            models.contains(&dir_name),
            "licenses/{dir_name}/ has no model-weights.tsv row"
        );
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the repo root")
        .to_path_buf()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
