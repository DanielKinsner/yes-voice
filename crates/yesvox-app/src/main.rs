//! `yesvox` — the thin standalone wrapper that feeds a whole file through the
//! core offline (plan §1.3). v1 usage: `yesvox <input.wav> <output.wav>`.
//!
//! Real restoration modules land behind the same core boundary in later slices;
//! today this proves the open → process → export path end-to-end.

use std::path::Path;
use std::process::ExitCode;

use yesvox_core::{read_wav, write_wav, IoError, Passthrough, VocalProcessor};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        let exe = args.first().map(String::as_str).unwrap_or("yesvox");
        eprintln!("usage: {exe} <input.wav> <output.wav>");
        return ExitCode::from(2);
    }

    match run(Path::new(&args[1]), Path::new(&args[2])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, output: &Path) -> Result<(), IoError> {
    let mut buffer = read_wav(input)?;

    let mut processor = Passthrough;
    processor.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    processor.reset();
    processor.process(&mut buffer);

    write_wav(output, &buffer)
}
