//! Filesystem/export hazard gates (#31): paying users' files live in OneDrive
//! folders, get locked by DAWs, and sit on nearly-full disks. Export must be
//! atomic (never a truncated file at the target), clean up its temp on any
//! failure, and turn each hazard into a friendly error — never a panic or a
//! silent success.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use yesvox_core::{read_wav, write_wav, DEFAULT_FLAC_COMPRESSION_LEVEL};
use yesvox_core::{write_wav_with, AudioBuffer, ExportOptions, SampleEncoding};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-hazard-{tag}-{}-{n}", std::process::id()));
    p
}

fn tone() -> AudioBuffer {
    let s: Vec<f32> = (0..2400).map(|i| 0.2 * (i as f32 * 0.05).sin()).collect();
    AudioBuffer::new(48_000, 16, SampleEncoding::Int, vec![s])
}

fn opts() -> ExportOptions {
    ExportOptions {
        bit_depth: 16,
        encoding: SampleEncoding::Int,
        dither: false,
        flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
    }
}

/// The sibling temp export writes before the atomic rename.
fn sibling_temp(path: &std::path::Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yesvox-tmp");
    PathBuf::from(s)
}

#[test]
fn export_into_a_missing_directory_errors_and_leaves_nothing() {
    let dir = unique("missing-dir");
    // `dir` is never created.
    let target = dir.join("out.wav");
    let result = write_wav_with(&target, &tone(), opts());
    assert!(result.is_err(), "export into a missing dir must error");
    assert!(!target.exists(), "no partial target");
    assert!(!sibling_temp(&target).exists(), "no leaked temp");
    assert!(!dir.exists(), "we did not create the missing dir");
}

#[test]
fn export_onto_an_existing_directory_errors_and_cleans_the_temp() {
    // A target path that is actually a directory: the temp can be written but
    // the rename onto a directory fails — the temp must be cleaned up and the
    // directory left intact.
    let target = unique("dir-target");
    std::fs::create_dir(&target).expect("make dir");
    let result = write_wav_with(&target, &tone(), opts());
    assert!(result.is_err(), "rename onto a directory must error");
    assert!(target.is_dir(), "the directory is untouched");
    assert!(!sibling_temp(&target).exists(), "the temp was cleaned up");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn a_clean_export_leaves_no_temp_behind() {
    let target = unique("clean").with_extension("wav");
    write_wav_with(&target, &tone(), opts()).expect("export");
    assert!(target.exists());
    assert!(!sibling_temp(&target).exists(), "no temp after success");
    let _ = std::fs::remove_file(&target);
}

// --- Windows share-mode / lock realities (run for real on the Windows CI leg) --

#[cfg(windows)]
mod windows_locks {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    /// Open `path` with an exclusive share mode (0 = no sharing), the way a DAW
    /// holding a file does, keeping the handle alive for the returned guard.
    fn lock_exclusive(path: &std::path::Path) -> std::fs::File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(path)
            .expect("exclusive open")
    }

    #[test]
    fn a_locked_output_target_yields_a_friendly_error_and_no_leak() {
        let target = unique("locked-out").with_extension("wav");
        // Create + exclusively lock the target, like an app holding the file.
        write_wav(&target, &tone()).expect("seed target");
        let _held = lock_exclusive(&target);

        let result = write_wav_with(&target, &tone(), opts());
        assert!(result.is_err(), "a locked target must error, not overwrite");
        let msg = format!("{}", result.unwrap_err());
        assert!(!msg.is_empty(), "the error is user-presentable");
        assert!(
            !sibling_temp(&target).exists(),
            "temp cleaned up after the lock"
        );

        drop(_held);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn a_locked_input_yields_a_friendly_error() {
        let input = unique("locked-in").with_extension("wav");
        write_wav(&input, &tone()).expect("seed input");
        let _held = lock_exclusive(&input);

        let result = read_wav(&input);
        assert!(result.is_err(), "a locked input must error, not panic");
        assert!(!format!("{}", result.unwrap_err()).is_empty());

        drop(_held);
        let _ = std::fs::remove_file(&input);
    }
}
