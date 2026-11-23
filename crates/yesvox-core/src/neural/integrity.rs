//! Weight-integrity checking for shipped neural models (#33).
//!
//! Corrupt or tampered weights must not silently produce garbage or crash ONNX
//! Runtime. Shipped weights (bundled by the installer, #20) are pinned by a
//! SHA-256 manifest; at load we hash the file and refuse a mismatch with a
//! reinstall-oriented message (the caller then falls back to the light engine).
//! A user-provided model (via `YESVOX_MODEL_PATH`) is *not* in the manifest, so
//! it is trusted as-is — we can only vouch for what we shipped.
//!
//! The SHA-256 is a small, dependency-free implementation (FIPS 180-4), tested
//! against the NIST vectors, so this lane runs in the normal gate without the
//! `neural` feature or any fetched weights.

use std::path::{Path, PathBuf};

/// Manifest of shipped weights: `(file name, expected lowercase-hex SHA-256)`.
/// Every `bundled` row in `licenses/model-weights.tsv` must be pinned here
/// (enforced by `xtask/tests/weights_license_audit.rs`). The MossFormer2 entry
/// is the fp32 ONNX export the #10 golden test validates — the v1.0 payload.
const WEIGHT_MANIFEST: &[(&str, &str)] = &[(
    "mossformer2_se_48k.onnx",
    "af292f1459aeb36d9a96ca956ff68e7eed646680335a953d42008b9dffe8f2a8",
)];

/// Resolve the neural model to load: the `YESVOX_MODEL_PATH` override wins;
/// otherwise the first shipped model found in `models/` next to the running
/// executable — the layout the Windows installer stages (#20). Without either,
/// the engine falls back to light as before.
pub fn default_model_path() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os("YESVOX_MODEL_PATH") {
        return Some(PathBuf::from(overridden));
    }
    let exe = std::env::current_exe().ok()?;
    bundled_model_in(&exe.parent()?.join("models"))
}

/// Locate a shipped model inside `dir` (the installed `models/` directory).
/// Only manifest-pinned file names are discovered, so a stray file dropped
/// next to the app can never silently become the engine.
pub fn bundled_model_in(dir: &Path) -> Option<PathBuf> {
    find_manifest_model(dir, WEIGHT_MANIFEST)
}

/// The testable core of bundled-model discovery.
fn find_manifest_model(dir: &Path, manifest: &[(&str, &str)]) -> Option<PathBuf> {
    manifest
        .iter()
        .map(|(name, _)| dir.join(name))
        .find(|path| path.is_file())
}

/// The platform's ONNX Runtime dynamic-library file name.
fn ort_dylib_name() -> &'static str {
    if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

/// The bundled ONNX Runtime dylib inside `dir`, if present.
fn bundled_ort_dylib_in(dir: &Path) -> Option<PathBuf> {
    let path = dir.join(ort_dylib_name());
    path.is_file().then_some(path)
}

/// Point `ort`'s load-dynamic probe at the runtime the installer staged next
/// to the executable (#20) when the user has not set `ORT_DYLIB_PATH`. Must
/// run at process start, before the first neural model load — without a path
/// the probe hangs (#33). A no-op when the override is set or nothing is
/// bundled (the dev/light-engine case).
pub fn configure_bundled_ort_runtime() {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return;
    }
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    if let Some(dylib) = bundled_ort_dylib_in(&dir) {
        std::env::set_var("ORT_DYLIB_PATH", dylib);
    }
}

/// Verify `path` against the shipped-weight manifest. `Ok(())` when the file
/// matches its pinned hash *or* is not a shipped weight (user-provided). `Err`
/// with a user-presentable message on a hash mismatch or read failure.
pub fn verify_model_integrity(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    // Only shipped weights are in the manifest; read the file lazily so an
    // unknown (user) model costs nothing.
    if WEIGHT_MANIFEST.iter().all(|(n, _)| *n != name) {
        return Ok(());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read neural model {name}: {e}"))?;
    check_hash(&name, &bytes, WEIGHT_MANIFEST)
}

/// The testable core: compare `bytes`' SHA-256 to the manifest entry for `name`.
fn check_hash(name: &str, bytes: &[u8], manifest: &[(&str, &str)]) -> Result<(), String> {
    match manifest.iter().find(|(n, _)| *n == name) {
        None => Ok(()), // not a shipped weight — trust it
        Some((_, expected)) => {
            let actual = sha256_hex(bytes);
            if actual.eq_ignore_ascii_case(expected) {
                Ok(())
            } else {
                Err(format!(
                    "neural model {name} failed its integrity check — the file is corrupt or tampered; reinstall"
                ))
            }
        }
    }
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data` as lowercase hex (FIPS 180-4). Dependency-free so it runs
/// in the normal gate.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Pad: 0x80, zeros to 56 mod 64, then the 64-bit big-endian bit length.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let (blocks, remainder) = msg.as_chunks::<64>();
    debug_assert!(remainder.is_empty());
    for chunk in blocks {
        let mut w = [0u32; 64];
        for (i, slot) in w.iter_mut().enumerate().take(16) {
            let b = i * 4;
            *slot = u32::from_be_bytes([chunk[b], chunk[b + 1], chunk[b + 2], chunk[b + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn check_hash_accepts_a_match_rejects_a_mismatch_and_ignores_unknowns() {
        let good = b"the real weights";
        let expected = sha256_hex(good);
        let manifest: Vec<(&str, &str)> = vec![("model.onnx", expected.as_str())];

        assert!(
            check_hash("model.onnx", good, &manifest).is_ok(),
            "matching hash"
        );
        assert!(
            check_hash("model.onnx", b"tampered", &manifest).is_err(),
            "mismatched hash is rejected"
        );
        assert!(
            check_hash("someones-own.onnx", b"anything", &manifest).is_ok(),
            "a model not in the manifest is trusted"
        );
    }

    #[test]
    fn verify_model_integrity_trusts_unknown_user_models() {
        // A name outside the shipped manifest (a user's own model) is trusted
        // as-is — the path need not even exist.
        assert!(verify_model_integrity(Path::new("does-not-matter.onnx")).is_ok());
    }

    #[test]
    fn find_manifest_model_only_discovers_manifest_names() {
        let dir = std::env::temp_dir().join(format!("yesvox-discovery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp models dir");
        let manifest: &[(&str, &str)] = &[("shipped.onnx", "irrelevant-here")];

        // Empty dir: nothing to discover.
        assert_eq!(find_manifest_model(&dir, manifest), None);

        // A stray file must never become the engine (#20).
        std::fs::write(dir.join("stray.onnx"), b"not ours").expect("write stray");
        assert_eq!(find_manifest_model(&dir, manifest), None);

        // The manifest-pinned name is discovered.
        std::fs::write(dir.join("shipped.onnx"), b"payload").expect("write shipped");
        assert_eq!(
            find_manifest_model(&dir, manifest),
            Some(dir.join("shipped.onnx"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_ort_dylib_discovery_matches_only_the_platform_name() {
        let dir = std::env::temp_dir().join(format!("yesvox-ort-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp runtime dir");

        assert_eq!(bundled_ort_dylib_in(&dir), None);
        std::fs::write(dir.join("unrelated.dll"), b"nope").expect("write unrelated");
        assert_eq!(bundled_ort_dylib_in(&dir), None);

        let expected = dir.join(ort_dylib_name());
        std::fs::write(&expected, b"runtime").expect("write runtime");
        assert_eq!(bundled_ort_dylib_in(&dir), Some(expected));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundled_model_discovery_finds_the_real_shipped_name() {
        // The installed layout: {app}\models\<first manifest name> (#20).
        let (name, _) = WEIGHT_MANIFEST[0];
        let dir = std::env::temp_dir().join(format!("yesvox-bundle-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp models dir");

        assert_eq!(bundled_model_in(&dir), None);
        std::fs::write(dir.join(name), b"weights").expect("write model");
        assert_eq!(bundled_model_in(&dir), Some(dir.join(name)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
