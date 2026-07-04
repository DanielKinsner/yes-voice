//! #21 Lane 3 — final-validation gates that don't need Dan's corpus.
//!
//! Cross-checks the hardening lane on the *full session pipeline* (open →
//! render → export) with pathological-but-legal inputs, and pins the suite
//! round-trip contract: a stem exported by YesStems survives YesVox with its
//! sample rate, bit depth, channel count, and container metadata intact for
//! yes-master to consume.

mod common;

use std::path::Path;

use yesvox_core::{
    loudness::true_peak_dbtp, read_wav, write_wav, write_wav_with, AnalysisError, AudioBuffer,
    ContainerMetadata, ExportOptions, SampleEncoding, Session, SessionError, WavMetadataChunk,
    DEFAULT_FLAC_COMPRESSION_LEVEL,
};

fn render_file(path: &Path) -> (Session, AudioBuffer) {
    let mut session = Session::new();
    session.open(path).expect("open input");
    session.render().expect("render input");
    let output = session.output().cloned().expect("rendered output");
    (session, output)
}

fn assert_finite(buffer: &AudioBuffer, label: &str) {
    for channel in buffer.channels() {
        assert!(
            channel.iter().all(|s| s.is_finite()),
            "{label}: output must stay finite"
        );
    }
}

#[test]
fn digital_silence_is_refused_with_a_clean_error_not_a_crash() {
    // Fully-silent input has nothing to restore; the analyzer refuses it with
    // a friendly, typed error at open — never a panic, never a garbage render.
    let sr = 48_000u32;
    let frames = sr as usize * 3;
    let buf = AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![vec![0.0; frames]]);
    let path = common::temp_wav("lane3-silent");
    write_wav(&path, &buf).expect("write silent fixture");

    let mut session = Session::new();
    let err = session.open(&path).expect_err("silence must be refused");
    assert!(
        matches!(err, SessionError::Analysis(AnalysisError::Silence)),
        "expected the Silence analysis error, got: {err}"
    );

    common::remove(&[&path]);
}

#[test]
fn pathological_clipping_is_refused_with_a_clean_error_not_a_crash() {
    // A take that is mostly clipped flats (84% of samples at full scale) is
    // unrecoverable; the analyzer refuses it with a typed error at open.
    let sr = 48_000u32;
    let frames = sr as usize * 3;
    let samples: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f32 / sr as f32;
            (4.0 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()).clamp(-1.0, 1.0)
        })
        .collect();
    let buf = AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![samples]);
    let path = common::temp_wav("lane3-hard-clipped");
    write_wav(&path, &buf).expect("write clipped fixture");

    let mut session = Session::new();
    let err = session
        .open(&path)
        .expect_err("hard clipping must be refused");
    assert!(
        matches!(
            err,
            SessionError::Analysis(AnalysisError::ExcessiveClipping { .. })
        ),
        "expected the ExcessiveClipping analysis error, got: {err}"
    );

    common::remove(&[&path]);
}

#[test]
fn recoverably_clipped_input_renders_within_the_true_peak_ceiling() {
    // A take with real clipping damage below the refusal threshold is exactly
    // what declip exists for; the full chain must survive it and still honor
    // the -1 dBTP export ceiling.
    let sr = 48_000u32;
    let frames = sr as usize * 3;
    let samples: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f32 / sr as f32;
            // A voice-like tone stack driven slightly past full scale, so only
            // the wave crests clip (a recoverable fraction of samples).
            let s = 1.15
                * (0.7 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                    + 0.35 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 450.0 * t).sin());
            s.clamp(-1.0, 1.0)
        })
        .collect();
    let clipped = samples.iter().filter(|s| s.abs() >= 0.999).count() as f32 / frames as f32;
    assert!(
        clipped > 0.001,
        "fixture must actually clip (got {clipped:.4})"
    );
    let buf = AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![samples]);
    let path = common::temp_wav("lane3-clipped");
    write_wav(&path, &buf).expect("write clipped fixture");

    let (_, output) = render_file(&path);
    assert_eq!(output.frames(), frames, "clipped take must keep its length");
    assert_finite(&output, "clipped");
    let tp = true_peak_dbtp(&output);
    assert!(
        tp <= -1.0 + 0.05,
        "clipped input: true peak {tp:.2} dBTP exceeds the -1.0 ceiling"
    );

    common::remove(&[&path]);
}

#[test]
fn suite_round_trip_preserves_format_and_metadata() {
    // YesStems → YesVox → yes-master: a separated stem arrives as a stereo
    // 44.1 kHz 24-bit WAV carrying bext/iXML provenance; after restoration the
    // exported file must present the identical container contract downstream.
    let sr = 44_100u32;
    let frames = sr as usize * 2;
    let (left, right): (Vec<f32>, Vec<f32>) = (0..frames)
        .map(|n| {
            let t = n as f32 / sr as f32;
            let s = 0.2 * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
                + 0.1 * (2.0 * std::f32::consts::PI * 400.0 * t).sin();
            (s, 0.8 * s)
        })
        .unzip();
    let mut buf = AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![left, right]);
    let metadata = ContainerMetadata::with_wav_chunks(vec![
        WavMetadataChunk::new(*b"bext", b"YesStems separation take 7".to_vec()),
        WavMetadataChunk::new(
            *b"iXML",
            b"<BWFXML><PROJECT>Suite</PROJECT></BWFXML>".to_vec(),
        ),
    ]);
    buf.set_metadata(metadata.clone());

    let input_path = common::temp_wav("lane3-roundtrip-in");
    write_wav_with(
        &input_path,
        &buf,
        ExportOptions {
            bit_depth: 24,
            encoding: SampleEncoding::Int,
            dither: false,
            flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
    .expect("write stem fixture");

    let (mut session, _) = render_file(&input_path);
    let out_dir = std::env::temp_dir();
    session.set_output_location(&out_dir);
    session.set_output_name(&format!("lane3-roundtrip-out-{}", std::process::id()));
    let written = session.export_to_target().expect("export rendered stem");

    let back = read_wav(&written).expect("read exported stem");
    assert_eq!(back.sample_rate(), sr, "sample rate must round-trip");
    assert_eq!(back.bits_per_sample(), 24, "bit depth must round-trip");
    assert_eq!(back.channel_count(), 2, "channel count must round-trip");
    assert_eq!(back.frames(), frames, "length must round-trip");
    assert_eq!(
        back.metadata().wav_chunks(),
        metadata.wav_chunks(),
        "container metadata must survive for the next suite tool"
    );

    common::remove(&[&input_path, &written]);
}
