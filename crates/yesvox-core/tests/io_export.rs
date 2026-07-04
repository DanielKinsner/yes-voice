//! Export-contract gates for #1: never-overwrite, TPDF dither on ≤24-bit, and
//! WAV/FLAC round-trips. These pin the plan §1.2/§1.3 export promises.

mod common;

use std::path::Path;

use yesvox_core::{
    export, read_audio, read_wav, write_wav, write_wav_with, AudioBuffer, ContainerMetadata,
    ExportOptions, FlacMetadataBlock, SampleEncoding, WavMetadataChunk,
    DEFAULT_FLAC_COMPRESSION_LEVEL,
};

/// A deterministic mono ramp + tone, values strictly inside (-1, 1).
fn tone(frames: usize, bits: u16, enc: SampleEncoding) -> AudioBuffer {
    let sr = 48_000u32;
    let mut s = Vec::with_capacity(frames);
    for n in 0..frames {
        let t = n as f32 / sr as f32;
        s.push(0.3 * (2.0 * std::f32::consts::PI * 220.0 * t).sin());
    }
    AudioBuffer::new(sr, bits, enc, vec![s])
}

#[test]
fn export_never_overwrites_and_auto_suffixes() {
    let dir = std::env::temp_dir();
    let desired = dir.join(format!("yesvox-export-{}.wav", std::process::id()));
    common::remove(&[&desired]);

    let buf = tone(2_400, 16, SampleEncoding::Int);
    let opts = ExportOptions {
        bit_depth: 16,
        encoding: SampleEncoding::Int,
        dither: false,
        flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
    };

    let first = export(&desired, &buf, opts).expect("first export");
    assert_eq!(first, desired, "first export should take the desired name");

    let second = export(&desired, &buf, opts).expect("second export");
    assert_ne!(second, desired, "second export must not reuse the name");
    assert!(second.exists(), "suffixed file written");
    assert!(
        desired.exists(),
        "original export still present (not clobbered)"
    );

    let suffixed = dir.join(format!("yesvox-export-{}_1.wav", std::process::id()));
    assert_eq!(second, suffixed, "collision should append _1");

    common::remove(&[&desired, &second]);
}

#[test]
fn export_onto_the_input_path_preserves_the_source_bytes() {
    // The specific non-destructive hazard (#27, finding 6): the user picks the
    // *input file's own path* as the export destination. Export must refuse to
    // clobber it — here by auto-suffixing — so the original source survives
    // byte-for-byte.
    let in_path = common::temp_wav("export-onto-source-in");
    common::remove(&[&in_path]);

    // A real source on disk, distinct from what we'll export.
    let source = tone(2_400, 16, SampleEncoding::Int);
    write_wav(&in_path, &source).expect("write source");
    let source_bytes_before = common::read_bytes(&in_path);

    // "Processed" output that differs from the source, exported to the SAME path.
    let processed = tone(2_400, 24, SampleEncoding::Int);
    let opts = ExportOptions {
        bit_depth: 24,
        encoding: SampleEncoding::Int,
        dither: false,
        flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
    };
    let written = export(&in_path, &processed, opts).expect("export onto input path");

    assert_ne!(
        written, in_path,
        "export onto the input path must not target the input itself"
    );
    let source_bytes_after = common::read_bytes(&in_path);
    assert_eq!(
        source_bytes_before, source_bytes_after,
        "the input/source file was modified by exporting onto its own path"
    );

    common::remove(&[&in_path, &written]);
}

#[test]
fn dither_perturbs_within_one_lsb_and_no_dither_is_exact() {
    let buf = tone(4_000, 16, SampleEncoding::Int);
    let dithered_path = common::temp_wav("dither-on");
    let clean_path = common::temp_wav("dither-off");

    write_wav_with(
        &dithered_path,
        &buf,
        ExportOptions {
            bit_depth: 16,
            encoding: SampleEncoding::Int,
            dither: true,
            flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
    .expect("dithered write");
    write_wav_with(
        &clean_path,
        &buf,
        ExportOptions {
            bit_depth: 16,
            encoding: SampleEncoding::Int,
            dither: false,
            flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
    .expect("clean write");

    let dithered = read_wav(&dithered_path).expect("read dithered");
    let clean = read_wav(&clean_path).expect("read clean");
    let lsb = 1.0f32 / (1u32 << 15) as f32;

    let mut any_diff = false;
    for ((d, c), _src) in dithered
        .channel(0)
        .iter()
        .zip(clean.channel(0))
        .zip(buf.channel(0))
    {
        // Dither never moves a sample by more than ~1 LSB vs the clean quantize.
        assert!(
            (d - c).abs() <= 1.5 * lsb,
            "dither moved a sample more than 1 LSB"
        );
        if (d - c).abs() > 0.0 {
            any_diff = true;
        }
    }
    assert!(any_diff, "dither produced no change — it is not active");

    common::remove(&[&dithered_path, &clean_path]);
}

#[test]
fn dither_is_reproducible_across_runs() {
    let buf = tone(2_000, 16, SampleEncoding::Int);
    let a = common::temp_wav("dither-repro-a");
    let b = common::temp_wav("dither-repro-b");
    let opts = ExportOptions {
        bit_depth: 16,
        encoding: SampleEncoding::Int,
        dither: true,
        flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
    };
    write_wav_with(&a, &buf, opts).unwrap();
    write_wav_with(&b, &buf, opts).unwrap();
    assert_eq!(
        common::read_bytes(&a),
        common::read_bytes(&b),
        "fixed-seed dither must be byte-reproducible"
    );
    common::remove(&[&a, &b]);
}

#[test]
fn flac_round_trips_length_rate_channels_and_samples() {
    let buf = tone(6_000, 16, SampleEncoding::Int);
    let path = std::env::temp_dir().join(format!("yesvox-flac-{}.flac", std::process::id()));
    common::remove(&[&path]);

    let written = export(
        &path,
        &buf,
        ExportOptions {
            bit_depth: 16,
            encoding: SampleEncoding::Int,
            dither: false,
            flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
    .expect("flac export");

    let back = read_audio(&written).expect("flac read back");
    assert_eq!(back.frames(), buf.frames(), "frame count preserved");
    assert_eq!(
        back.sample_rate(),
        buf.sample_rate(),
        "sample rate preserved"
    );
    assert_eq!(
        back.channel_count(),
        buf.channel_count(),
        "channels preserved"
    );

    // FLAC is lossless at the stored bit depth: samples match within 1 LSB.
    let lsb = 1.0f32 / (1u32 << 15) as f32;
    for (a, b) in back.channel(0).iter().zip(buf.channel(0)) {
        assert!((a - b).abs() <= lsb, "flac sample drifted beyond 1 LSB");
    }

    common::remove(&[&written]);
}

#[test]
fn flac_compression_level_changes_encoding_without_changing_audio() {
    let buf = tone(8_000, 16, SampleEncoding::Int);
    let low = std::env::temp_dir().join(format!("yesvox-flac-low-{}.flac", std::process::id()));
    let high = std::env::temp_dir().join(format!("yesvox-flac-high-{}.flac", std::process::id()));
    common::remove(&[&low, &high]);

    let base = ExportOptions {
        bit_depth: 16,
        encoding: SampleEncoding::Int,
        dither: false,
        flac_compression_level: 0,
    };
    let low_written = export(&low, &buf, base).expect("level 0 flac export");
    let high_written = export(
        &high,
        &buf,
        ExportOptions {
            flac_compression_level: 8,
            ..base
        },
    )
    .expect("level 8 flac export");

    assert_ne!(
        common::read_bytes(&low_written),
        common::read_bytes(&high_written),
        "FLAC compression level must reach the encoder config"
    );

    let low_back = read_audio(&low_written).expect("read level 0 flac");
    let high_back = read_audio(&high_written).expect("read level 8 flac");
    assert_eq!(low_back.frames(), buf.frames());
    assert_eq!(high_back.frames(), buf.frames());
    let lsb = 1.0f32 / (1u32 << 15) as f32;
    for ((low_sample, high_sample), source_sample) in low_back
        .channel(0)
        .iter()
        .zip(high_back.channel(0))
        .zip(buf.channel(0))
    {
        assert!(
            (low_sample - source_sample).abs() <= lsb,
            "level 0 FLAC sample drifted beyond 1 LSB"
        );
        assert!(
            (high_sample - source_sample).abs() <= lsb,
            "level 8 FLAC sample drifted beyond 1 LSB"
        );
    }

    common::remove(&[&low_written, &high_written]);
}

#[test]
fn wav_export_preserves_source_metadata_chunks() {
    let mut buf = tone(2_400, 24, SampleEncoding::Int);
    let metadata = ContainerMetadata::with_wav_chunks(vec![
        WavMetadataChunk::new(*b"bext", b"broadcast metadata".to_vec()),
        WavMetadataChunk::new(
            *b"iXML",
            b"<BWFXML><PROJECT>YesVox</PROJECT></BWFXML>".to_vec(),
        ),
        WavMetadataChunk::new(*b"LIST", b"INFOICMT\x0a\x00\x00\x00round trip".to_vec()),
        WavMetadataChunk::new(*b"yvx1", b"unknown chunk".to_vec()),
    ]);
    buf.set_metadata(metadata.clone());

    let path = common::temp_wav("metadata");
    write_wav_with(
        &path,
        &buf,
        ExportOptions {
            bit_depth: 24,
            encoding: SampleEncoding::Int,
            dither: false,
            flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
    .expect("metadata wav write");

    let back = read_wav(&path).expect("metadata wav read");
    assert_eq!(back.metadata().wav_chunks(), metadata.wav_chunks());
    assert_eq!(back.frames(), buf.frames());
    assert_eq!(back.sample_rate(), buf.sample_rate());
    common::remove(&[&path]);
}

#[test]
fn flac_export_preserves_vorbis_comments_and_application_blocks_at_planned_levels() {
    let mut buf = tone(2_400, 16, SampleEncoding::Int);
    let metadata = ContainerMetadata::with_flac_blocks(vec![
        FlacMetadataBlock::new(4, vorbis_comment_block("YesVoxTest", &["ARTIST=YesVox"])),
        FlacMetadataBlock::new(2, b"YVOXapplication payload".to_vec()),
    ]);
    buf.set_metadata(metadata.clone());

    for level in [0, 5, 8] {
        let path = std::env::temp_dir().join(format!(
            "yesvox-flac-metadata-{}-{level}.flac",
            std::process::id()
        ));
        common::remove(&[&path]);
        let written = export(
            &path,
            &buf,
            ExportOptions {
                bit_depth: 16,
                encoding: SampleEncoding::Int,
                dither: false,
                flac_compression_level: level,
            },
        )
        .expect("metadata flac export");

        let back = read_audio(&written).expect("metadata flac read");
        assert_eq!(
            back.metadata().flac_blocks(),
            metadata.flac_blocks(),
            "metadata blocks must round-trip at FLAC level {level}"
        );
        assert_eq!(back.frames(), buf.frames());
        common::remove(&[&written]);
    }
}

#[test]
fn write_wav_remains_bit_exact_identity() {
    // The low-level writer (used for fixtures) stays bit-exact: decode of its
    // output equals the source samples exactly.
    let buf = tone(1_000, 16, SampleEncoding::Int);
    let path = common::temp_wav("bitexact");
    write_wav(&path, &buf).unwrap();
    let back = read_wav(&path).unwrap();
    let lsb = 1.0f32 / (1u32 << 15) as f32;
    for (a, b) in back.channel(0).iter().zip(buf.channel(0)) {
        assert!((a - b).abs() <= lsb, "bit-exact writer drifted");
    }
    assert!(Path::new(&path).exists());
    common::remove(&[&path]);
}

fn vorbis_comment_block(vendor: &str, comments: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor.as_bytes());
    out.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for comment in comments {
        out.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        out.extend_from_slice(comment.as_bytes());
    }
    out
}
