use std::{
    fs,
    path::{Path, PathBuf},
};

#[test]
fn windows_setup_declares_offline_model_bundle_and_notices() {
    let root = repo_root();
    let setup = read(root.join("packaging/windows/yesvox.iss"));

    for required in [
        "AppName=YesVox",
        "OutputBaseFilename=yesvox-v1-unsigned",
        "#define ModelBundleDir",
        r#"{#ModelBundleDir}\*"#,
        r#"DestDir: "{app}\models""#,
        "THIRD-PARTY-NOTICES.txt",
        "LicenseFile=packaging\\windows\\THIRD-PARTY-NOTICES.txt",
    ] {
        assert!(
            setup.contains(required),
            "packaging/windows/yesvox.iss must contain {required:?}"
        );
    }
}

#[test]
fn setup_notices_cover_every_weight_manifest_row() {
    let root = repo_root();
    let notices = read(root.join("packaging/windows/THIRD-PARTY-NOTICES.txt"));
    let manifest = read(root.join("licenses/model-weights.tsv"));

    for (index, raw_line) in manifest.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = raw_line.split('\t').collect();
        assert_eq!(
            cols.len(),
            8,
            "licenses/model-weights.tsv:{} must have 8 tab-separated columns",
            index + 1
        );
        let model = cols[0].trim();
        let status = cols[1].trim();
        let spdx = cols[2].trim();
        let license_doc = cols[3].trim();

        for required in [model, status, spdx, license_doc] {
            assert!(
                notices.contains(required),
                "THIRD-PARTY-NOTICES.txt must mention {required:?} from licenses/model-weights.tsv:{}",
                index + 1
            );
        }
    }
}

/// #20: once a weight ships under Apache-2.0, the installed notice file must
/// carry the actual license text (a pointer is not distribution-compliant),
/// and the shipped hash must be stated next to the model it pins.
#[test]
fn notices_carry_license_text_and_hash_for_bundled_weights() {
    let root = repo_root();
    let notices = read(root.join("packaging/windows/THIRD-PARTY-NOTICES.txt"));
    let manifest = read(root.join("licenses/model-weights.tsv"));

    let mut bundled_apache = false;
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = raw_line.split('\t').collect();
        let model = cols[0].trim();
        let status = cols[1].trim();
        let spdx = cols[2].trim();
        let bundled_sha256 = cols[7].trim();
        if status != "bundled" {
            continue;
        }
        assert!(
            notices.contains(bundled_sha256),
            "THIRD-PARTY-NOTICES.txt must state the shipped SHA-256 for bundled model {model}"
        );
        if spdx.contains("Apache-2.0") {
            bundled_apache = true;
        }
    }

    if bundled_apache {
        for marker in [
            "Apache License",
            "Version 2.0, January 2004",
            "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
            "END OF TERMS AND CONDITIONS",
        ] {
            assert!(
                notices.contains(marker),
                "THIRD-PARTY-NOTICES.txt must include the full Apache-2.0 text ({marker:?} missing)"
            );
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the repo root")
        .to_path_buf()
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()))
}
