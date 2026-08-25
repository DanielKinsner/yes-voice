//! Project automation. `cargo xtask gate` runs the full local gate, identical
//! to CI (docs/TESTING.md): fmt --check -> clippy -D warnings -> test -> deny.
//!
//! Every step is **required**. `cargo deny` is an external tool; if it is not
//! installed the gate fails with an install hint rather than silently skipping
//! the license/advisory check (a silent skip let supply-chain drift slip past
//! the local gate — #27, finding 8).

use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Instant,
};
use yesvox_core::{
    loudness::{integrated_lufs, true_peak_dbtp},
    neural::sha256_hex,
    singing::{
        f0_cents_deviation, hnr_db, pitch_contour, spectral_envelope_cosine, tail_survival_db,
        vibrato,
    },
    AudioBuffer, ContentMode, Preset, Session,
};

const EVAL_CORPUS_ENV: &str = "YESVOX_EVAL_CORPUS";
const EVAL_REPORT_ENV: &str = "YESVOX_EVAL_REPORT";
const MODEL_BUNDLE_ENV: &str = "YESVOX_MODEL_BUNDLE_DIR";
const MODEL_WEIGHTS_MANIFEST_ENV: &str = "YESVOX_MODEL_WEIGHTS_MANIFEST";
const MODEL_BUNDLE_MAX_BYTES_ENV: &str = "YESVOX_MODEL_BUNDLE_MAX_BYTES";
const RUNTIME_BUNDLE_ENV: &str = "YESVOX_RUNTIME_BUNDLE_DIR";
const RUNTIME_MANIFEST_ENV: &str = "YESVOX_RUNTIME_MANIFEST";
const DEFAULT_EVAL_REPORT: &str = "target/eval/report.json";
const DEFAULT_MODEL_WEIGHTS_MANIFEST: &str = "licenses/model-weights.tsv";
const DEFAULT_RUNTIME_MANIFEST: &str = "packaging/windows/onnxruntime-1.26.0-sha256.tsv";
const EVAL_MIN_PER_MODE: usize = 12;
const MAX_MODEL_BUNDLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const LOUDNESS_TOLERANCE_LU: f32 = 2.0;
const TRUE_PEAK_CEILING_DBTP: f32 = 0.2;
const MIN_VOICED_RATIO: f32 = 0.80;
const MAX_F0_P95_CENTS: f32 = 35.0;
const MIN_HNR_DELTA_DB: f32 = -8.0;
const MIN_SPECTRAL_COSINE_VOICEOVER: f32 = 0.82;
const MIN_SPECTRAL_COSINE_SINGING: f32 = 0.88;
const MIN_SINGING_TAIL_DB: f32 = -12.0;
const MAX_VIBRATO_RATE_ERROR: f32 = 0.25;
const MIN_VIBRATO_DEPTH_RATIO: f32 = 0.50;
const MAX_VIBRATO_DEPTH_RATIO: f32 = 1.60;

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "gate" => gate(),
        "bless" => bless(),
        "eval" => {
            // `eval --check` previews the corpus without rendering (#21/W4);
            // bare `eval` runs the real render harness.
            if std::env::args().any(|a| a == "--check") {
                eval_check()
            } else {
                eval()
            }
        }
        "package-audit" => package_audit(),
        other => {
            eprintln!("unknown xtask: {other:?}\n\ntasks:\n  gate           fmt --check + clippy -D warnings + test + deny\n  bless          deliberately regenerate goldens (YESVOX_BLESS=1 cargo test)\n  eval           run local #21 corpus eval (YESVOX_EVAL_CORPUS); add --check to preflight only\n  package-audit  audit staged #20 model package payloads (YESVOX_MODEL_BUNDLE_DIR)");
            ExitCode::from(2)
        }
    }
}

/// Audit staged #20 package inputs. Fresh checkouts and CI do not have the
/// private release model bundle, so absence of `YESVOX_MODEL_BUNDLE_DIR` is a
/// success-path skip. Once it is set, bundle problems are hard failures.
fn package_audit() -> ExitCode {
    let Some(model_dir) = std::env::var_os(MODEL_BUNDLE_ENV).map(PathBuf::from) else {
        eprintln!(
            "package-audit: skipped — {MODEL_BUNDLE_ENV} is not set. \
             Set it to the staged model bundle dir to verify #20 package payloads."
        );
        return ExitCode::SUCCESS;
    };

    let runtime_dir = std::env::var_os(RUNTIME_BUNDLE_ENV).map(PathBuf::from);
    match run_package_audit(&model_dir).and_then(|model| {
        runtime_dir
            .as_deref()
            .map(run_runtime_audit)
            .transpose()
            .map(|runtime| (model, runtime))
    }) {
        Ok((outcome, runtime)) => {
            eprintln!(
                "package-audit: green — {} model files, {} bytes, {} bundled manifest rows",
                outcome.files, outcome.bytes, outcome.bundled_rows
            );
            if let Some(runtime) = runtime {
                eprintln!(
                    "package-audit: green — {} runtime files, {} bytes",
                    runtime.files, runtime.bytes
                );
            } else {
                eprintln!("package-audit: runtime skipped — {RUNTIME_BUNDLE_ENV} is not set");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("package-audit: {e}");
            ExitCode::FAILURE
        }
    }
}

struct PackageAuditOutcome {
    files: usize,
    bytes: u64,
    bundled_rows: usize,
}

struct RuntimeAuditOutcome {
    files: usize,
    bytes: u64,
}

fn run_package_audit(model_dir: &Path) -> Result<PackageAuditOutcome, String> {
    if !model_dir.is_dir() {
        return Err(format!(
            "{MODEL_BUNDLE_ENV} must point at a directory, got {}",
            model_dir.display()
        ));
    }

    let manifest_path = model_weights_manifest_path();
    let bundled = bundled_manifest_hashes(&manifest_path)?;
    let max_bundle_bytes = max_model_bundle_bytes()?;

    let mut files = Vec::new();
    collect_payload_files(model_dir, model_dir, &mut files)?;
    if files.is_empty() {
        return Err(format!(
            "{MODEL_BUNDLE_ENV} contains no staged model files: {}",
            model_dir.display()
        ));
    }

    let mut total_bytes = 0u64;
    let mut seen_hashes = BTreeSet::new();
    for file in &files {
        let metadata = fs::metadata(file)
            .map_err(|e| format!("could not inspect staged model {}: {e}", file.display()))?;
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "staged model bundle size overflowed u64".to_string())?;
        if total_bytes > max_bundle_bytes {
            return Err(format!(
                "staged model bundle is {} bytes, above the {} byte v1.0 ceiling",
                total_bytes, max_bundle_bytes
            ));
        }

        let bytes = fs::read(file)
            .map_err(|e| format!("could not read staged model {}: {e}", file.display()))?;
        let hash = sha256_hex(&bytes);
        let Some(model) = bundled.get(&hash) else {
            let relative = file.strip_prefix(model_dir).unwrap_or(file);
            return Err(format!(
                "{} ({hash}) is not listed as a bundled shipped hash in {}",
                display_path(relative),
                manifest_path.display()
            ));
        };
        eprintln!(
            "package-audit: matched {} -> {model}",
            display_path(file.strip_prefix(model_dir).unwrap_or(file))
        );
        seen_hashes.insert(hash);
    }

    for (hash, model) in &bundled {
        if !seen_hashes.contains(hash) {
            return Err(format!(
                "bundled manifest row for {model} ({hash}) has no matching staged model file in {}",
                model_dir.display()
            ));
        }
    }

    Ok(PackageAuditOutcome {
        files: files.len(),
        bytes: total_bytes,
        bundled_rows: bundled.len(),
    })
}

fn model_weights_manifest_path() -> PathBuf {
    std::env::var_os(MODEL_WEIGHTS_MANIFEST_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join(DEFAULT_MODEL_WEIGHTS_MANIFEST))
}

fn runtime_manifest_path() -> PathBuf {
    std::env::var_os(RUNTIME_MANIFEST_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join(DEFAULT_RUNTIME_MANIFEST))
}

fn run_runtime_audit(runtime_dir: &Path) -> Result<RuntimeAuditOutcome, String> {
    if !runtime_dir.is_dir() {
        return Err(format!(
            "{RUNTIME_BUNDLE_ENV} must point at a directory, got {}",
            runtime_dir.display()
        ));
    }

    let manifest_path = runtime_manifest_path();
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("could not read {}: {e}", manifest_path.display()))?;
    let mut expected = BTreeMap::new();
    for (index, raw_line) in raw.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = raw_line.split('\t').collect();
        if cols.len() != 3 {
            return Err(format!(
                "{}:{line_no} must have 3 tab-separated columns",
                manifest_path.display()
            ));
        }
        let file = cols[0].trim().replace('\\', "/");
        let version = cols[1].trim();
        let hash = cols[2].trim().to_ascii_lowercase();
        if file.is_empty() || version.is_empty() || !is_sha256(&hash) {
            return Err(format!(
                "{}:{line_no} needs file, version, and SHA-256",
                manifest_path.display()
            ));
        }
        if expected
            .insert(file.clone(), (version.to_string(), hash))
            .is_some()
        {
            return Err(format!(
                "{}:{line_no} duplicates runtime file {file}",
                manifest_path.display()
            ));
        }
    }
    if expected.is_empty() {
        return Err(format!(
            "runtime manifest contains no files: {}",
            manifest_path.display()
        ));
    }

    let mut files = Vec::new();
    collect_payload_files(runtime_dir, runtime_dir, &mut files)?;
    let mut seen = BTreeSet::new();
    let mut total_bytes = 0u64;
    for file in &files {
        let relative = display_path(file.strip_prefix(runtime_dir).unwrap_or(file));
        let Some((version, expected_hash)) = expected.get(&relative) else {
            return Err(format!(
                "{relative} is not listed in runtime manifest {}",
                manifest_path.display()
            ));
        };
        let metadata = fs::metadata(file)
            .map_err(|e| format!("could not inspect staged runtime {}: {e}", file.display()))?;
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "staged runtime bundle size overflowed u64".to_string())?;
        let bytes = fs::read(file)
            .map_err(|e| format!("could not read staged runtime {}: {e}", file.display()))?;
        let actual_hash = sha256_hex(&bytes);
        if &actual_hash != expected_hash {
            return Err(format!(
                "{relative} ({actual_hash}) does not match runtime manifest {} ({version}, {expected_hash})",
                manifest_path.display()
            ));
        }
        eprintln!("package-audit: matched runtime {relative} ({version})");
        seen.insert(relative);
    }

    for file in expected.keys() {
        if !seen.contains(file) {
            return Err(format!(
                "runtime manifest entry {file} has no matching staged file in {}",
                runtime_dir.display()
            ));
        }
    }

    Ok(RuntimeAuditOutcome {
        files: files.len(),
        bytes: total_bytes,
    })
}

fn max_model_bundle_bytes() -> Result<u64, String> {
    let Some(raw) = std::env::var_os(MODEL_BUNDLE_MAX_BYTES_ENV) else {
        return Ok(MAX_MODEL_BUNDLE_BYTES);
    };
    raw.to_string_lossy().parse::<u64>().map_err(|e| {
        format!("{MODEL_BUNDLE_MAX_BYTES_ENV} must be an unsigned byte count, got {raw:?}: {e}")
    })
}

fn bundled_manifest_hashes(manifest_path: &Path) -> Result<BTreeMap<String, String>, String> {
    let raw = fs::read_to_string(manifest_path)
        .map_err(|e| format!("could not read {}: {e}", manifest_path.display()))?;
    let mut bundled = BTreeMap::new();
    for (index, raw_line) in raw.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = raw_line.split('\t').collect();
        if cols.len() != 8 {
            return Err(format!(
                "{}:{line_no} must have 8 tab-separated columns",
                manifest_path.display()
            ));
        }
        let model = cols[0].trim();
        let status = cols[1].trim();
        let bundled_sha256 = cols[7].trim();
        match status {
            "candidate" => {}
            "bundled" => {
                if !is_sha256(bundled_sha256) {
                    return Err(format!(
                        "{}:{line_no} bundled row for {model} needs a shipped SHA-256",
                        manifest_path.display()
                    ));
                }
                if let Some(existing) =
                    bundled.insert(bundled_sha256.to_ascii_lowercase(), model.to_string())
                {
                    return Err(format!(
                        "{}:{line_no} duplicates shipped hash with {existing}",
                        manifest_path.display()
                    ));
                }
            }
            other => {
                return Err(format!(
                    "{}:{line_no} status must be candidate or bundled, got {other:?}",
                    manifest_path.display()
                ));
            }
        }
    }
    Ok(bundled)
}

fn collect_payload_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| {
            format!(
                "could not read model bundle directory {}: {e}",
                dir.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("could not read a model bundle directory entry: {e}"))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
        if file_type.is_dir() {
            collect_payload_files(root, &path, files)?;
        } else if file_type.is_file() {
            let _ = path
                .strip_prefix(root)
                .map_err(|e| format!("could not relativize {}: {e}", path.display()))?;
            files.push(path);
        }
    }
    Ok(())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the repo root")
        .to_path_buf()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Run the #21 local real-stem no-regression harness. CI and fresh checkouts do
/// not have Dan's private corpus, so absence of `YESVOX_EVAL_CORPUS` is an
/// explicit success-path skip; once the env var is set, every corpus problem is
/// a loud failure and a JSON report is written for diffing.
fn eval() -> ExitCode {
    let Some(root) = std::env::var_os(EVAL_CORPUS_ENV).map(PathBuf::from) else {
        eprintln!(
            "eval: skipped — {EVAL_CORPUS_ENV} is not set. \
             Set it to a local license-clean corpus dir to run #21 real-stem evaluation."
        );
        return ExitCode::SUCCESS;
    };

    match run_eval(&root) {
        Ok(outcome) if outcome.failed == 0 => {
            eprintln!(
                "eval: green — {} stems passed; report: {}",
                outcome.total,
                outcome.report_path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            eprintln!(
                "eval: failed — {}/{} stems failed; report: {}",
                outcome.failed,
                outcome.total,
                outcome.report_path.display()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("eval: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A single stem the eval would run, resolved WITHOUT rendering — the unit
/// `cargo xtask eval --check` reports (#21/W4).
#[derive(Debug)]
struct PreflightStem {
    relative: String,
    mode: ContentMode,
    preset: &'static str,
}

/// Resolve every stem the corpus would evaluate — its mode and the preset the
/// run would pick — without opening or rendering any audio. Reuses the exact
/// loader `run_eval` uses, so the preview matches the run and every corpus
/// problem (bad extension, unreadable manifest row, too few stems, empty dir)
/// surfaces here as the same precise error the real eval would hit.
fn preflight(root: &Path) -> Result<Vec<PreflightStem>, String> {
    if !root.is_dir() {
        return Err(format!(
            "{EVAL_CORPUS_ENV} must point at a directory, got {}",
            root.display()
        ));
    }
    let stems = load_stems(root)?;
    validate_corpus_shape(&stems)?;
    Ok(stems
        .iter()
        .map(|stem| PreflightStem {
            relative: stem.relative.clone(),
            mode: stem.mode,
            preset: Preset::default_for(stem.mode).name,
        })
        .collect())
}

/// `cargo xtask eval --check`: preflight the corpus without rendering. Skip-safe
/// exactly like `eval` (unset env -> exit 0 with a notice) so it stays gate-safe;
/// with a corpus set it lists every stem plus the mode/preset it would use, or
/// fails loudly with the precise reason the corpus is malformed. Dan runs this
/// himself to check a corpus before the (slow) real eval, so the text is
/// friendly.
fn eval_check() -> ExitCode {
    let Some(root) = std::env::var_os(EVAL_CORPUS_ENV).map(PathBuf::from) else {
        eprintln!(
            "eval --check: skipped — {EVAL_CORPUS_ENV} is not set. \
             Set it to a local license-clean corpus dir to preflight #21 real-stem evaluation."
        );
        return ExitCode::SUCCESS;
    };

    match preflight(&root) {
        Ok(stems) => {
            let voiceover = stems
                .iter()
                .filter(|stem| stem.mode == ContentMode::Voiceover)
                .count();
            let singing = stems.len() - voiceover;
            eprintln!(
                "eval --check: {} stems ready ({voiceover} voiceover, {singing} singing) — nothing rendered:",
                stems.len()
            );
            for stem in &stems {
                eprintln!(
                    "  {} — {} — preset {}",
                    stem.relative,
                    mode_label(stem.mode),
                    stem.preset
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("eval --check: {e}");
            ExitCode::FAILURE
        }
    }
}

struct EvalOutcome {
    report_path: PathBuf,
    total: usize,
    failed: usize,
}

fn run_eval(root: &Path) -> Result<EvalOutcome, String> {
    if !root.is_dir() {
        return Err(format!(
            "{EVAL_CORPUS_ENV} must point at a directory, got {}",
            root.display()
        ));
    }

    let stems = load_stems(root)?;
    validate_corpus_shape(&stems)?;

    let mut reports = Vec::with_capacity(stems.len());
    for stem in &stems {
        eprintln!(
            "eval: rendering {} ({})",
            stem.relative,
            mode_label(stem.mode)
        );
        reports.push(eval_stem(stem));
    }

    let summary = EvalSummary::from_reports(&reports);
    let report = EvalReport {
        schema_version: 1,
        corpus: display_path(root),
        min_per_mode: EVAL_MIN_PER_MODE,
        summary: summary.clone(),
        stems: reports,
    };

    let report_path = eval_report_path();
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "could not create eval report directory {}: {e}",
                parent.display()
            )
        })?;
    }
    let file = fs::File::create(&report_path).map_err(|e| {
        format!(
            "could not create eval report {}: {e}",
            report_path.display()
        )
    })?;
    serde_json::to_writer_pretty(file, &report)
        .map_err(|e| format!("could not write eval report {}: {e}", report_path.display()))?;

    Ok(EvalOutcome {
        report_path,
        total: summary.total,
        failed: summary.failed,
    })
}

fn eval_report_path() -> PathBuf {
    std::env::var_os(EVAL_REPORT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EVAL_REPORT))
}

#[derive(Clone)]
struct Stem {
    path: PathBuf,
    relative: String,
    mode: ContentMode,
}

fn load_stems(root: &Path) -> Result<Vec<Stem>, String> {
    let manifest = root.join("manifest.tsv");
    let mut stems = if manifest.exists() {
        load_manifest(root, &manifest)?
    } else {
        let mut found = Vec::new();
        collect_audio_files(root, root, &mut found)?;
        found
    };
    stems.sort_by(|a, b| a.relative.cmp(&b.relative));
    if stems.is_empty() {
        return Err(format!(
            "no eval stems found under {} (use .wav/.flac files, or manifest.tsv)",
            root.display()
        ));
    }
    Ok(stems)
}

fn load_manifest(root: &Path, manifest: &Path) -> Result<Vec<Stem>, String> {
    let raw = fs::read_to_string(manifest)
        .map_err(|e| format!("could not read {}: {e}", manifest.display()))?;
    let mut stems = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 {
            return Err(format!(
                "{}:{line_no} must have at least path<TAB>mode",
                manifest.display()
            ));
        }
        if cols[0].eq_ignore_ascii_case("path") && cols[1].eq_ignore_ascii_case("mode") {
            continue;
        }
        let relative = Path::new(cols[0]);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "{}:{line_no} path must be relative and stay inside the corpus",
                manifest.display()
            ));
        }
        let mode = parse_mode(cols[1]).ok_or_else(|| {
            format!(
                "{}:{line_no} mode must be voiceover or singing, got {:?}",
                manifest.display(),
                cols[1]
            )
        })?;
        let path = root.join(relative);
        if !is_audio_path(&path) {
            return Err(format!(
                "{}:{line_no} path must point at a .wav or .flac file",
                manifest.display()
            ));
        }
        if !path.is_file() {
            return Err(format!(
                "{}:{line_no} missing file {}",
                manifest.display(),
                path.display()
            ));
        }
        stems.push(Stem {
            path,
            relative: display_path(relative),
            mode,
        });
    }
    Ok(stems)
}

fn collect_audio_files(root: &Path, dir: &Path, stems: &mut Vec<Stem>) -> Result<(), String> {
    let mut entries = fs::read_dir(dir)
        .map_err(|e| format!("could not read corpus directory {}: {e}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("could not read a corpus directory entry: {e}"))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("could not inspect {}: {e}", path.display()))?;
        if file_type.is_dir() {
            collect_audio_files(root, &path, stems)?;
        } else if file_type.is_file() && is_audio_path(&path) {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| format!("could not relativize {}: {e}", path.display()))?;
            let mode = infer_mode_from_path(relative).ok_or_else(|| {
                format!(
                    "could not infer content mode for {} — put it under singing/ or voiceover/, or add manifest.tsv",
                    relative.display()
                )
            })?;
            let relative = display_path(relative);
            stems.push(Stem {
                path,
                relative,
                mode,
            });
        }
    }
    Ok(())
}

fn validate_corpus_shape(stems: &[Stem]) -> Result<(), String> {
    let voiceover = stems
        .iter()
        .filter(|s| s.mode == ContentMode::Voiceover)
        .count();
    let singing = stems
        .iter()
        .filter(|s| s.mode == ContentMode::Singing)
        .count();
    if voiceover < EVAL_MIN_PER_MODE || singing < EVAL_MIN_PER_MODE {
        return Err(format!(
            "corpus must contain at least {EVAL_MIN_PER_MODE} voiceover and {EVAL_MIN_PER_MODE} singing stems; found {voiceover} voiceover, {singing} singing"
        ));
    }
    Ok(())
}

fn eval_stem(stem: &Stem) -> StemReport {
    match eval_stem_inner(stem) {
        Ok(report) => report,
        Err(reason) => StemReport::failed(stem, vec![reason]),
    }
}

fn eval_stem_inner(stem: &Stem) -> Result<StemReport, String> {
    let mut session = Session::new();
    session
        .open(&stem.path)
        .map_err(|e| format!("open/analyze failed: {e}"))?;
    session.select_preset(Preset::default_for(stem.mode));
    session
        .apply_smart_assist()
        .map_err(|e| format!("smart assist failed: {e}"))?;

    let input = session
        .input()
        .cloned()
        .ok_or_else(|| "session did not retain the opened input".to_string())?;
    let input_report = audio_info(&input);

    let started = Instant::now();
    session
        .render()
        .map_err(|e| format!("render failed: {e}"))?;
    let render_seconds = started.elapsed().as_secs_f32();

    let output = session
        .output()
        .cloned()
        .ok_or_else(|| "render succeeded without producing output".to_string())?;
    let output_report = audio_info(&output);

    let metrics = metric_report(stem.mode, &input, &output);
    let mut failures = Vec::new();
    validate_audio_contract(
        stem.mode,
        &input,
        &output,
        &input_report,
        &output_report,
        &mut failures,
    );
    validate_metrics(stem.mode, &metrics, &mut failures);

    Ok(StemReport {
        path: stem.relative.clone(),
        mode: mode_label(stem.mode).to_string(),
        status: if failures.is_empty() {
            "passed"
        } else {
            "failed"
        }
        .to_string(),
        failures,
        render_seconds,
        input: Some(input_report),
        output: Some(output_report),
        metrics: Some(metrics),
    })
}

fn validate_audio_contract(
    mode: ContentMode,
    input: &AudioBuffer,
    output: &AudioBuffer,
    input_info: &AudioInfo,
    output_info: &AudioInfo,
    failures: &mut Vec<String>,
) {
    if output.frames() != input.frames() {
        failures.push(format!(
            "length changed: {} -> {} frames",
            input.frames(),
            output.frames()
        ));
    }
    if output.sample_rate() != input.sample_rate() {
        failures.push(format!(
            "sample rate changed: {} -> {} Hz",
            input.sample_rate(),
            output.sample_rate()
        ));
    }
    if output.channel_count() != input.channel_count() {
        failures.push(format!(
            "channel count changed: {} -> {}",
            input.channel_count(),
            output.channel_count()
        ));
    }
    if output_info.true_peak_dbtp > TRUE_PEAK_CEILING_DBTP {
        failures.push(format!(
            "true peak {:.2} dBTP exceeds {:.2} dBTP ceiling",
            output_info.true_peak_dbtp, TRUE_PEAK_CEILING_DBTP
        ));
    }
    if let Some(output_lufs) = output_info.integrated_lufs {
        let target = match mode {
            ContentMode::Voiceover => -16.0,
            ContentMode::Singing => -14.0,
        };
        if (output_lufs - target).abs() > LOUDNESS_TOLERANCE_LU {
            failures.push(format!(
                "output loudness {:.2} LUFS is outside {:.1} LU of {:.1} LUFS",
                output_lufs, LOUDNESS_TOLERANCE_LU, target
            ));
        }
    } else {
        failures.push("output loudness could not be measured".to_string());
    }
    if input_info.integrated_lufs.is_none() {
        failures.push("input loudness could not be measured".to_string());
    }
}

fn validate_metrics(mode: ContentMode, metrics: &MetricReport, failures: &mut Vec<String>) {
    if metrics.reference_voiced_frames >= 24 {
        if metrics.voiced_ratio < MIN_VOICED_RATIO {
            failures.push(format!(
                "voiced support dropped to {:.2} (minimum {:.2})",
                metrics.voiced_ratio, MIN_VOICED_RATIO
            ));
        }
        if metrics.f0_p95_cents > MAX_F0_P95_CENTS {
            failures.push(format!(
                "F0 p95 deviation {:.2} cents exceeds {:.2}",
                metrics.f0_p95_cents, MAX_F0_P95_CENTS
            ));
        }
    }

    if let Some(delta) = metrics.hnr_delta_db {
        if delta < MIN_HNR_DELTA_DB {
            failures.push(format!(
                "HNR regressed by {:.2} dB (minimum delta {:.2} dB)",
                delta, MIN_HNR_DELTA_DB
            ));
        }
    }

    let min_cosine = match mode {
        ContentMode::Voiceover => MIN_SPECTRAL_COSINE_VOICEOVER,
        ContentMode::Singing => MIN_SPECTRAL_COSINE_SINGING,
    };
    if metrics.spectral_envelope_cosine < min_cosine {
        failures.push(format!(
            "spectral-envelope cosine {:.4} below {:.4}",
            metrics.spectral_envelope_cosine, min_cosine
        ));
    }

    if mode == ContentMode::Singing {
        if metrics.tail_survival_db < MIN_SINGING_TAIL_DB {
            failures.push(format!(
                "singing tail survival {:.2} dB below {:.2} dB",
                metrics.tail_survival_db, MIN_SINGING_TAIL_DB
            ));
        }
        if let Some(rate_error) = metrics.vibrato_rate_error {
            if rate_error > MAX_VIBRATO_RATE_ERROR {
                failures.push(format!(
                    "vibrato rate error {:.2} exceeds {:.2}",
                    rate_error, MAX_VIBRATO_RATE_ERROR
                ));
            }
        }
        if let Some(depth_ratio) = metrics.vibrato_depth_ratio {
            if !(MIN_VIBRATO_DEPTH_RATIO..=MAX_VIBRATO_DEPTH_RATIO).contains(&depth_ratio) {
                failures.push(format!(
                    "vibrato depth ratio {:.2} outside {:.2}..{:.2}",
                    depth_ratio, MIN_VIBRATO_DEPTH_RATIO, MAX_VIBRATO_DEPTH_RATIO
                ));
            }
        }
    }
}

fn audio_info(buffer: &AudioBuffer) -> AudioInfo {
    AudioInfo {
        sample_rate: buffer.sample_rate(),
        channels: buffer.channel_count(),
        frames: buffer.frames(),
        duration_seconds: buffer.frames() as f32 / buffer.sample_rate().max(1) as f32,
        integrated_lufs: integrated_lufs(buffer).filter(|v| v.is_finite()),
        true_peak_dbtp: true_peak_dbtp(buffer),
    }
}

fn metric_report(mode: ContentMode, input: &AudioBuffer, output: &AudioBuffer) -> MetricReport {
    let sr = input.sample_rate() as f32;
    let reference = mono_mix(input);
    let processed_raw = mono_mix(output);
    let processed = level_match(&reference, &processed_raw);

    let reference_pitch = pitch_contour(&reference, sr);
    let processed_pitch = pitch_contour(&processed, sr);
    let reference_voiced_frames = reference_pitch.iter().filter(|f| f.is_some()).count();
    let processed_voiced_frames = processed_pitch.iter().filter(|f| f.is_some()).count();
    let voiced_ratio = if reference_voiced_frames == 0 {
        1.0
    } else {
        processed_voiced_frames as f32 / reference_voiced_frames as f32
    };

    let cents = f0_cents_deviation(&reference, &processed, sr);
    let reference_hnr = hnr_db(&reference, sr).filter(|v| v.is_finite());
    let processed_hnr = hnr_db(&processed, sr).filter(|v| v.is_finite());
    let reference_vibrato = vibrato(&reference, sr);
    let processed_vibrato = vibrato(&processed, sr);
    let vibrato_rate_error = reference_vibrato.zip(processed_vibrato).and_then(|(a, b)| {
        if a.rate_hz > 0.0 && a.depth_cents >= 15.0 {
            Some(((b.rate_hz - a.rate_hz) / a.rate_hz).abs())
        } else {
            None
        }
    });
    let vibrato_depth_ratio = reference_vibrato.zip(processed_vibrato).and_then(|(a, b)| {
        if a.depth_cents >= 15.0 {
            Some(b.depth_cents / a.depth_cents.max(1.0e-6))
        } else {
            None
        }
    });

    MetricReport {
        reference_voiced_frames,
        processed_voiced_frames,
        common_voiced_frames: cents.voiced_frames,
        voiced_ratio,
        f0_median_cents: cents.median,
        f0_p95_cents: cents.p95,
        reference_vibrato_rate_hz: reference_vibrato.map(|v| v.rate_hz),
        processed_vibrato_rate_hz: processed_vibrato.map(|v| v.rate_hz),
        reference_vibrato_depth_cents: reference_vibrato.map(|v| v.depth_cents),
        processed_vibrato_depth_cents: processed_vibrato.map(|v| v.depth_cents),
        vibrato_rate_error,
        vibrato_depth_ratio,
        reference_hnr_db: reference_hnr,
        processed_hnr_db: processed_hnr,
        hnr_delta_db: reference_hnr.zip(processed_hnr).map(|(a, b)| b - a),
        spectral_envelope_cosine: spectral_envelope_cosine(&reference, &processed, sr),
        tail_survival_db: tail_survival_db(&reference, &processed, sr),
        singing_thresholds_applied: mode == ContentMode::Singing,
    }
}

fn mono_mix(buffer: &AudioBuffer) -> Vec<f32> {
    let frames = buffer.frames();
    let channels = buffer.channels();
    let gain = 1.0 / channels.len().max(1) as f32;
    let mut mixed = vec![0.0; frames];
    for channel in channels {
        for (dst, &sample) in mixed.iter_mut().zip(channel) {
            *dst += sample * gain;
        }
    }
    mixed
}

fn level_match(reference: &[f32], processed: &[f32]) -> Vec<f32> {
    let r = rms(reference);
    let p = rms(processed);
    let gain = if p > 1.0e-9 { r / p } else { 1.0 };
    processed.iter().map(|sample| sample * gain).collect()
}

fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    (signal.iter().map(|x| x * x).sum::<f32>() / signal.len() as f32).sqrt()
}

#[derive(Clone, Serialize)]
struct EvalReport {
    schema_version: u32,
    corpus: String,
    min_per_mode: usize,
    summary: EvalSummary,
    stems: Vec<StemReport>,
}

#[derive(Clone, Serialize)]
struct EvalSummary {
    total: usize,
    passed: usize,
    failed: usize,
    voiceover: usize,
    singing: usize,
}

impl EvalSummary {
    fn from_reports(reports: &[StemReport]) -> Self {
        let failed = reports.iter().filter(|r| r.status == "failed").count();
        EvalSummary {
            total: reports.len(),
            passed: reports.len() - failed,
            failed,
            voiceover: reports.iter().filter(|r| r.mode == "voiceover").count(),
            singing: reports.iter().filter(|r| r.mode == "singing").count(),
        }
    }
}

#[derive(Clone, Serialize)]
struct StemReport {
    path: String,
    mode: String,
    status: String,
    failures: Vec<String>,
    render_seconds: f32,
    input: Option<AudioInfo>,
    output: Option<AudioInfo>,
    metrics: Option<MetricReport>,
}

impl StemReport {
    fn failed(stem: &Stem, failures: Vec<String>) -> Self {
        StemReport {
            path: stem.relative.clone(),
            mode: mode_label(stem.mode).to_string(),
            status: "failed".to_string(),
            failures,
            render_seconds: 0.0,
            input: None,
            output: None,
            metrics: None,
        }
    }
}

#[derive(Clone, Serialize)]
struct AudioInfo {
    sample_rate: u32,
    channels: usize,
    frames: usize,
    duration_seconds: f32,
    integrated_lufs: Option<f32>,
    true_peak_dbtp: f32,
}

#[derive(Clone, Serialize)]
struct MetricReport {
    reference_voiced_frames: usize,
    processed_voiced_frames: usize,
    common_voiced_frames: usize,
    voiced_ratio: f32,
    f0_median_cents: f32,
    f0_p95_cents: f32,
    reference_vibrato_rate_hz: Option<f32>,
    processed_vibrato_rate_hz: Option<f32>,
    reference_vibrato_depth_cents: Option<f32>,
    processed_vibrato_depth_cents: Option<f32>,
    vibrato_rate_error: Option<f32>,
    vibrato_depth_ratio: Option<f32>,
    reference_hnr_db: Option<f32>,
    processed_hnr_db: Option<f32>,
    hnr_delta_db: Option<f32>,
    spectral_envelope_cosine: f32,
    tail_survival_db: f32,
    singing_thresholds_applied: bool,
}

fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "wav" | "flac"))
        .unwrap_or(false)
}

fn infer_mode_from_path(relative: &Path) -> Option<ContentMode> {
    let mut inferred = None;
    for component in relative.components() {
        let std::path::Component::Normal(raw) = component else {
            continue;
        };
        let text = raw.to_string_lossy().to_ascii_lowercase();
        for token in text.split(['-', '_', ' ', '.']) {
            let token_mode = parse_mode(token);
            match (inferred, token_mode) {
                (None, Some(mode)) => inferred = Some(mode),
                (Some(existing), Some(mode)) if existing != mode => return None,
                _ => {}
            }
        }
    }
    inferred
}

fn parse_mode(raw: &str) -> Option<ContentMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "voiceover" | "voiceovers" | "vo" | "spoken" | "speech" | "dialogue" | "narration" => {
            Some(ContentMode::Voiceover)
        }
        "singing" | "sung" | "song" | "songs" | "vocal" | "vocals" | "lead" => {
            Some(ContentMode::Singing)
        }
        _ => None,
    }
}

fn mode_label(mode: ContentMode) -> &'static str {
    match mode {
        ContentMode::Voiceover => "voiceover",
        ContentMode::Singing => "singing",
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Deliberately regenerate golden reference files. This is the ONLY sanctioned
/// way to rewrite a golden: it sets `YESVOX_BLESS=1` for a `cargo test` run, so
/// the golden harness (`yesvox_core::golden`) overwrites the `.golden` files
/// instead of asserting against them. The normal gate never sets the variable,
/// so an autonomous loop cannot re-bless a red golden as a side-effect. Review
/// and commit the resulting golden diff as its own deliberate change.
fn bless() -> ExitCode {
    eprintln!("=== cargo xtask bless (YESVOX_BLESS=1) ===");
    eprintln!("Regenerating goldens — this is a deliberate action; commit the diff explicitly.");
    match Command::new("cargo")
        .args(["test", "--all"])
        .env("YESVOX_BLESS", "1")
        .status()
    {
        Ok(status) if status.success() => {
            eprintln!("\nbless: goldens regenerated");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("bless run failed ({status})");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("bless could not run cargo test: {e}");
            ExitCode::FAILURE
        }
    }
}

/// One step of the gate: the command, its args, and whether it is an external
/// cargo subcommand (an installed tool, not a rustup component). External tools
/// get an availability preflight so a missing install fails loudly with a hint.
struct Step {
    args: &'static [&'static str],
    external: bool,
}

fn gate() -> ExitCode {
    let steps = [
        Step {
            args: &["fmt", "--all", "--check"],
            external: false,
        },
        Step {
            args: &[
                "clippy",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
            external: false,
        },
        Step {
            args: &["test", "--workspace", "--exclude", "xtask"],
            external: false,
        },
        Step {
            // Windows keeps the running `target/debug/xtask.exe` locked while
            // `cargo xtask gate` is executing. Test xtask itself in a separate
            // target dir so a changed xtask can still gate itself locally.
            args: &["test", "-p", "xtask", "--target-dir", "target/xtask-gate"],
            external: false,
        },
        Step {
            args: &["deny", "check"],
            external: true,
        },
    ];

    for step in &steps {
        let pretty = format!("cargo {}", step.args.join(" "));
        // Every step is required. An external tool that isn't installed fails the
        // gate with a hint — never a silent skip (#27, finding 8).
        if step.external && !subcommand_available(step.args[0]) {
            eprintln!(
                "gate: `{pretty}` cannot run — cargo-{tool} is required but not installed.\n\
                 Install it with `cargo install cargo-{tool}` (CI enforces this license/advisory gate).",
                tool = step.args[0]
            );
            return ExitCode::FAILURE;
        }
        eprintln!("\n=== {pretty} ===");
        match Command::new("cargo").args(step.args).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("gate failed at `{pretty}` ({status})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("gate could not run `{pretty}`: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    eprintln!("\ngate: green");
    ExitCode::SUCCESS
}

/// Whether `cargo <sub> --version` runs, used to detect an installed external
/// cargo subcommand.
fn subcommand_available(sub: &str) -> bool {
    Command::new("cargo")
        .args([sub, "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod eval_check_tests {
    use super::*;
    use std::fs;

    fn temp_corpus(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("yesvox-eval-check-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create corpus dir");
        dir
    }

    /// Empty (0-byte) files with the right extension. The preflight checks
    /// extension + existence but never decodes, so their emptiness is the proof
    /// that `eval --check` does not render.
    fn touch_wavs(dir: &Path, n: usize) {
        fs::create_dir_all(dir).expect("mk subdir");
        for i in 0..n {
            fs::write(dir.join(format!("stem{i}.wav")), b"").expect("touch wav");
        }
    }

    #[test]
    fn preflight_rejects_a_non_directory() {
        let dir = temp_corpus("nondir");
        let file = dir.join("not-a-dir.wav");
        fs::write(&file, b"").unwrap();
        assert!(preflight(&file).is_err(), "a file is not a corpus dir");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_rejects_an_empty_corpus() {
        let dir = temp_corpus("empty");
        let err = preflight(&dir).unwrap_err();
        assert!(err.contains("no eval stems"), "got: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_lists_loose_files_with_mode_and_preset_without_rendering() {
        let dir = temp_corpus("loose");
        touch_wavs(&dir.join("voiceover"), EVAL_MIN_PER_MODE);
        touch_wavs(&dir.join("singing"), EVAL_MIN_PER_MODE);
        let stems = preflight(&dir).expect("preflight ok on empty (undecoded) files");
        assert_eq!(stems.len(), EVAL_MIN_PER_MODE * 2);
        assert_eq!(
            stems
                .iter()
                .filter(|s| s.mode == ContentMode::Voiceover)
                .count(),
            EVAL_MIN_PER_MODE
        );
        // Each stem carries the preset the eval would actually use for its mode.
        for s in &stems {
            assert_eq!(s.preset, Preset::default_for(s.mode).name);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_reads_a_manifest() {
        let dir = temp_corpus("manifest");
        for i in 0..(EVAL_MIN_PER_MODE * 2) {
            fs::write(dir.join(format!("s{i}.wav")), b"").unwrap();
        }
        let mut manifest = String::from("path\tmode\n");
        for i in 0..(EVAL_MIN_PER_MODE * 2) {
            let mode = if i < EVAL_MIN_PER_MODE {
                "voiceover"
            } else {
                "singing"
            };
            manifest.push_str(&format!("s{i}.wav\t{mode}\n"));
        }
        fs::write(dir.join("manifest.tsv"), manifest).unwrap();
        let stems = preflight(&dir).expect("manifest preflight ok");
        assert_eq!(stems.len(), EVAL_MIN_PER_MODE * 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_reports_a_malformed_manifest_row() {
        let dir = temp_corpus("badrow");
        fs::write(dir.join("a.wav"), b"").unwrap();
        fs::write(dir.join("manifest.tsv"), "path\tmode\na.wav\tsideways\n").unwrap();
        let err = preflight(&dir).unwrap_err();
        assert!(
            err.contains("mode must be voiceover or singing"),
            "got: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_requires_the_minimum_per_mode() {
        let dir = temp_corpus("toofew");
        touch_wavs(&dir.join("voiceover"), EVAL_MIN_PER_MODE);
        touch_wavs(&dir.join("singing"), 2);
        let err = preflight(&dir).unwrap_err();
        assert!(err.contains("at least"), "got: {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
