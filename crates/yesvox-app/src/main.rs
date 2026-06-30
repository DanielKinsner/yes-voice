//! `yesvox` — the thin standalone wrapper that feeds a whole file through the
//! core offline (plan §1.3). v1 usage: `yesvox <input.{wav,flac}> <output.{wav,flac}>`.
//!
//! The shell knows nothing about DSP/ML: it opens the file, runs the whole-file
//! analyzer, asks the core to assemble the bound restoration [`Chain`] for the
//! current settings + analysis, runs it, and exports. The chain (engine binding,
//! preset, Fast-mode downshift, de-bleed refusal) all live behind the core
//! boundary; the app only reports the decisions and writes the result.
//!
//! [`Chain`]: yesvox_core::Chain

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use yesvox_core::{
    analyze, export, read_audio, BleedVerdict, BuiltChain, Engine, Preset, Settings, VocalProcessor,
};

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

fn run(input: &Path, output: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // v1 has no on-disk config yet; defaults give voiceover + Balanced intensity.
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

    // Assemble the bound restoration chain for these settings + this analysis.
    // No SKU benchmark table at runtime, so est_render_s = 0.0 disables the
    // slow-hardware downshift here (the visible Fast-mode badge is the GUI's).
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1);
    let model = std::env::var_os("YESVOX_MODEL_PATH").map(PathBuf::from);
    // The headless CLI has no preset gallery, so it renders the content mode's
    // default preset (the GUI passes the user's gallery selection instead).
    let preset = Preset::default_for(settings.content_mode);
    let mut built = BuiltChain::build(&settings, preset, &report, model.as_deref(), workers, 0.0);

    eprintln!(
        "engine: preset {} | intensity {:?} | denoiser {:?}{}",
        built.preset.name,
        built.resolved.mode,
        built.engine,
        if built.resolved.fast_mode {
            "  [FAST MODE — downshifted]"
        } else {
            ""
        },
    );
    if built.resolved.binding.engine == Engine::Neural && built.engine == Engine::Light {
        eprintln!(
            "note: neural engine unavailable (no `neural` feature or YESVOX_MODEL_PATH) — using the light engine"
        );
    }

    // De-bleed refusal (plan §4.5): if the input looks like a mix, refuse with the
    // actionable YesStems pointer rather than pretending to restore it.
    let verdict = built.chain.assess_bleed(&buffer);
    if verdict == BleedVerdict::Block {
        return Err(verdict
            .message()
            .unwrap_or("input looks like a mix — run YesStems first")
            .into());
    }
    if let Some(msg) = verdict.message() {
        eprintln!("warning: {msg}");
    }

    built.chain.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    built.chain.reset();
    built.chain.process(&mut buffer);

    let opts = settings
        .export
        .to_options(buffer.bits_per_sample(), buffer.encoding());
    Ok(export(output, &buffer, opts)?)
}
