//! Decoder robustness (#30): malformed audio is the #1 hostile input a shipping
//! restoration tool sees. Rust prevents memory unsafety, not panics/OOM — so a
//! curated malformed-fixture corpus plus a byte-mutation proptest assert that
//! WAV/FLAC ingest (and the raw RIFF metadata walker it now runs) either returns
//! a clean `IoError` or a sane buffer, but never panics or allocates absurdly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use proptest::prelude::*;
use yesvox_core::{read_flac, read_wav, AudioBuffer};

/// Frames a decoded buffer must not exceed for a small fixture — a cap that
/// catches "declared a giant size, allocated it" without a real allocator hook.
const MAX_SANE_FRAMES: usize = 10_000_000;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp(ext: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-fuzz-{}-{n}.{ext}", std::process::id()));
    p
}

/// A canonical, valid 16-bit mono PCM WAV built by hand so field offsets are
/// exact: channels @22, sample_rate @24, bits @34, data_size @40.
fn valid_wav() -> Vec<u8> {
    let n = 128u32;
    let data_size = n * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_size).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // channels @22
    v.extend_from_slice(&48_000u32.to_le_bytes()); // sample rate @24
    v.extend_from_slice(&96_000u32.to_le_bytes()); // byte rate
    v.extend_from_slice(&2u16.to_le_bytes()); // block align
    v.extend_from_slice(&16u16.to_le_bytes()); // bits @34
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_size.to_le_bytes()); // data size @40
    for i in 0..n {
        v.extend_from_slice(&((i as i16).wrapping_mul(100)).to_le_bytes());
    }
    v
}

/// Decode raw bytes as WAV via a temp file. Returns the decode result; the call
/// completing at all is the no-panic proof.
fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, yesvox_core::IoError> {
    let p = temp("wav");
    std::fs::write(&p, bytes).expect("write temp");
    let r = read_wav(&p);
    let _ = std::fs::remove_file(&p);
    r
}

fn decode_flac(bytes: &[u8]) -> Result<AudioBuffer, yesvox_core::IoError> {
    let p = temp("flac");
    std::fs::write(&p, bytes).expect("write temp");
    let r = read_flac(&p);
    let _ = std::fs::remove_file(&p);
    r
}

/// A decode outcome must be either an error or a finite, size-bounded buffer.
fn assert_sane(result: &Result<AudioBuffer, yesvox_core::IoError>) {
    if let Ok(buf) = result {
        assert!(buf.frames() <= MAX_SANE_FRAMES, "absurd decoded length");
        for ch in 0..buf.channel_count() {
            assert!(
                buf.channel(ch).iter().all(|s| s.is_finite()),
                "decoded a non-finite sample"
            );
        }
    }
}

fn set_u16(bytes: &mut [u8], off: usize, val: u16) {
    if off + 2 <= bytes.len() {
        bytes[off..off + 2].copy_from_slice(&val.to_le_bytes());
    }
}
fn set_u32(bytes: &mut [u8], off: usize, val: u32) {
    if off + 4 <= bytes.len() {
        bytes[off..off + 4].copy_from_slice(&val.to_le_bytes());
    }
}

#[test]
fn the_valid_base_fixture_decodes() {
    // Sanity: the hand-built base is actually a valid WAV, so mutation tests
    // start from a real file.
    let buf = decode_wav(&valid_wav()).expect("valid base should decode");
    assert_eq!(buf.frames(), 128);
    assert_eq!(buf.channel_count(), 1);
}

#[test]
fn malformed_wav_corpus_never_panics_and_mostly_errors() {
    let base = valid_wav();
    let with = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = base.clone();
        f(&mut b);
        b
    };

    let corpus: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("magic_only", b"RIFF".to_vec()),
        ("riff_wrong_form", {
            let mut v = b"RIFF".to_vec();
            v.extend_from_slice(&64u32.to_le_bytes());
            v.extend_from_slice(b"XXXX"); // not WAVE
            v.extend_from_slice(&[0u8; 32]);
            v
        }),
        ("truncated_header", base[..8.min(base.len())].to_vec()),
        ("header_only_no_data", base[..44.min(base.len())].to_vec()),
        ("truncated_mid_data", base[..base.len() / 2].to_vec()),
        ("huge_riff_size", with(&|b| set_u32(b, 4, u32::MAX))),
        ("data_size_bomb", with(&|b| set_u32(b, 40, u32::MAX))),
        ("zero_channels", with(&|b| set_u16(b, 22, 0))),
        ("nine_channels", with(&|b| set_u16(b, 22, 9))),
        ("zero_sample_rate", with(&|b| set_u32(b, 24, 0))),
        ("absurd_sample_rate", with(&|b| set_u32(b, 24, u32::MAX))),
        ("zero_bit_depth", with(&|b| set_u16(b, 34, 0))),
        ("absurd_bit_depth", with(&|b| set_u16(b, 34, 255))),
        ("garbage_after_magic", {
            let mut v = b"RIFFWAVE".to_vec();
            v.extend_from_slice(&[0xABu8; 64]);
            v
        }),
    ];

    for (name, bytes) in &corpus {
        // The key invariant: the call returns (no panic, no OOM) and is sane.
        let result = decode_wav(bytes);
        assert_sane(&result);
        // Fundamentally-undecodable inputs should be a clean IoError.
        if matches!(
            *name,
            "empty" | "magic_only" | "truncated_header" | "zero_channels" | "nine_channels"
        ) {
            assert!(
                result.is_err(),
                "{name}: expected a clean IoError, got a buffer"
            );
        }
    }
}

#[test]
fn malformed_flac_corpus_never_panics() {
    let corpus: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("magic_only", b"fLaC".to_vec()),
        ("magic_plus_garbage", {
            let mut v = b"fLaC".to_vec();
            v.extend_from_slice(&[0x7Fu8; 128]);
            v
        }),
        ("not_flac", vec![0u8; 200]),
    ];
    for (_name, bytes) in &corpus {
        let result = decode_flac(bytes);
        assert_sane(&result);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Flip/overwrite/truncate a valid WAV in structure-aware ways.
    #[test]
    fn mutating_a_valid_wav_never_panics(
        flips in prop::collection::vec((any::<u16>(), any::<u8>()), 0..12),
        field in prop::option::of((prop::sample::select(vec![4usize, 22, 24, 34, 40]), any::<u32>())),
        truncate_to in prop::option::of(0usize..200),
    ) {
        let mut bytes = valid_wav();
        for (idx, val) in flips {
            let i = (idx as usize) % bytes.len();
            bytes[i] = val;
        }
        if let Some((off, val)) = field {
            set_u32(&mut bytes, off, val);
        }
        if let Some(t) = truncate_to {
            bytes.truncate(t.min(bytes.len()));
        }
        let result = decode_wav(&bytes);
        assert_sane(&result);
    }

    /// Pure garbage after a WAV magic — the metadata walker + hound must cope.
    #[test]
    fn arbitrary_bytes_with_wav_magic_never_panic(
        tail in prop::collection::vec(any::<u8>(), 0..256)
    ) {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&tail);
        let result = decode_wav(&bytes);
        assert_sane(&result);
    }
}
