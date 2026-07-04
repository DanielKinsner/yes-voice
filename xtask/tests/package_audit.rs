use std::{fs, path::PathBuf, process::Command};

use yesvox_core::neural::sha256_hex;

#[test]
fn package_audit_without_model_bundle_is_skip_safe() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package-audit")
        .env_remove("YESVOX_MODEL_BUNDLE_DIR")
        .output()
        .expect("run xtask package-audit");

    assert!(
        output.status.success(),
        "package audit without staged models must be CI-safe; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("YESVOX_MODEL_BUNDLE_DIR"),
        "skip notice should name the env var; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("package-audit: skipped"),
        "skip notice should be explicit; stderr:\n{stderr}"
    );
}

#[test]
fn package_audit_rejects_staged_payload_without_a_bundled_manifest_hash() {
    let root = temp_root("candidate-manifest");
    let model_dir = root.join("models");
    fs::create_dir_all(&model_dir).expect("create staged model dir");
    fs::write(model_dir.join("unexpected.onnx"), b"not approved yet").expect("write model");

    let manifest = root.join("model-weights.tsv");
    fs::write(
        &manifest,
        concat!(
            "FakeModel\tcandidate\tApache-2.0\tlicenses/Fake/WEIGHTS_LICENSE.md\t",
            "https://example.com/fake.onnx\tfake-ref\t",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tTBD\n"
        ),
    )
    .expect("write manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package-audit")
        .env("YESVOX_MODEL_BUNDLE_DIR", &model_dir)
        .env("YESVOX_MODEL_WEIGHTS_MANIFEST", &manifest)
        .output()
        .expect("run xtask package-audit");

    assert!(
        !output.status.success(),
        "staged model bytes must not pass without a bundled manifest hash"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected.onnx"),
        "failure should name the unmanifested payload; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("not listed as a bundled shipped hash"),
        "failure should explain the manifest problem; stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_audit_enforces_model_bundle_size_ceiling() {
    let root = temp_root("size-ceiling");
    let model_dir = root.join("models");
    fs::create_dir_all(&model_dir).expect("create staged model dir");
    let payload = b"approved but too large";
    fs::write(model_dir.join("approved.onnx"), payload).expect("write model");

    let manifest = root.join("model-weights.tsv");
    write_bundled_manifest(&manifest, "FakeModel", &sha256_hex(payload));

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package-audit")
        .env("YESVOX_MODEL_BUNDLE_DIR", &model_dir)
        .env("YESVOX_MODEL_WEIGHTS_MANIFEST", &manifest)
        .env("YESVOX_MODEL_BUNDLE_MAX_BYTES", "4")
        .output()
        .expect("run xtask package-audit");

    assert!(
        !output.status.success(),
        "staged model bundle above the configured ceiling must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("above the 4 byte v1.0 ceiling"),
        "failure should name the active byte ceiling; stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_audit_accepts_staged_payload_with_matching_bundled_hash() {
    let root = temp_root("matching-hash");
    let model_dir = root.join("models");
    fs::create_dir_all(&model_dir).expect("create staged model dir");
    let payload = b"approved payload";
    fs::write(model_dir.join("approved.onnx"), payload).expect("write model");

    let manifest = root.join("model-weights.tsv");
    write_bundled_manifest(&manifest, "FakeModel", &sha256_hex(payload));

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package-audit")
        .env("YESVOX_MODEL_BUNDLE_DIR", &model_dir)
        .env("YESVOX_MODEL_WEIGHTS_MANIFEST", &manifest)
        .output()
        .expect("run xtask package-audit");

    assert!(
        output.status.success(),
        "matching staged payload and bundled manifest hash should pass; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("package-audit: green"),
        "success should be explicit; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("1 model files"),
        "success should report file count; stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(root);
}

fn temp_root(label: &str) -> PathBuf {
    let unique = format!(
        "yesvox-package-audit-{label}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    );
    std::env::temp_dir().join(unique)
}

fn write_bundled_manifest(path: &PathBuf, model: &str, bundled_sha256: &str) {
    fs::write(
        path,
        format!(
            "{model}\tbundled\tApache-2.0\tlicenses/{model}/WEIGHTS_LICENSE.md\t\
             https://example.com/{model}.onnx\tfake-ref\t\
             aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\t\
             {bundled_sha256}\n"
        ),
    )
    .expect("write manifest");
}
