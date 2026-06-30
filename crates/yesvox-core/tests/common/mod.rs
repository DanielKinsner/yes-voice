//! Shared helpers for the guarantee integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use yesvox_core::{write_wav, AudioBuffer, SampleEncoding};

/// Write a deterministic 16-bit stereo WAV fixture (a quiet 440 Hz tone, 100 ms)
/// to `path` and return the in-memory buffer it was built from.
pub fn write_fixture(path: &Path) -> AudioBuffer {
    let sample_rate = 48_000u32;
    let frames = 4_800usize; // 100 ms
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for n in 0..frames {
        let t = n as f32 / sample_rate as f32;
        let s = 0.25 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
        left.push(s);
        right.push(s * 0.5);
    }
    let buffer = AudioBuffer::new(sample_rate, 16, SampleEncoding::Int, vec![left, right]);
    write_wav(path, &buffer).expect("write fixture");
    buffer
}

/// A unique temp WAV path for this test process and `tag`.
pub fn temp_wav(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-test-{tag}-{}.wav", std::process::id()));
    p
}

/// Read a file's full byte contents.
pub fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read bytes")
}

/// Best-effort cleanup of temp files.
pub fn remove(paths: &[&Path]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}
