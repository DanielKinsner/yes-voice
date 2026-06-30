//! `yesvox` — the thin standalone wrapper that feeds a whole file through the
//! core offline (plan §1.3). v1 usage: `yesvox <input.{wav,flac}> <output.{wav,flac}>`.
//!
//! Real restoration modules land behind the same core boundary in later slices;
//! today this proves the open → process → export path end-to-end, with the
//! never-overwrite, dither, and settings contracts from #1.

use std::path::Path;
use std::process::ExitCode;

use yesvox_core::{analyze, export, read_audio, Passthrough, Settings, VocalProcessor};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        let exe = args.first().map(String::as_str).unwrap_or("yesvox");
        eprintln!("usage: {exe} <input.{{wav,flac}}> <output.{{wav,flac}}>");
        return ExitCode::from(2);
    }

    match run(Path::new(&args[1]), Path::new(&args[2])) {
        Ok(written) => {
            println!("{}", written.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, output: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    // v1 has no on-disk config yet; defaults give voiceover + dithered export.
    let settings = Settings::default();

    let mut buffer = read_audio(input)?;

    // Whole-file analysis pass before any processing (plan §5.1); unusable
    // inputs are rejected here with an actionable message.
    let report = analyze(&buffer)?;
    eprintln!(
        "analysis: {:?}, {:.1} LUFS, {:.1} dBTP, noise {:.0} dB, clips {}, hum {} Hz, content {:?} ({:.0}% conf)",
        report.channel_mode,
        report.integrated_lufs,
        report.true_peak_dbtp,
        report.noise_floor_db,
        report.clip_sample_count,
        report.hum.frequency,
        report.content.mode,
        report.content.confidence * 100.0,
    );

    let mut processor = Passthrough;
    processor.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    processor.reset();
    processor.process(&mut buffer);

    let opts = settings
        .export
        .to_options(buffer.bits_per_sample(), buffer.encoding());
    Ok(export(output, &buffer, opts)?)
}
