//! ONNX Runtime adapter implementing [`MaskModel`] (behind the `neural` feature).
//!
//! This is the thin leaf that runs the real MossFormer2_SE_48K graph: it feeds
//! the 180-dim Kaldi features (`[1, T, 180]`) the front-end produces and reads
//! back the 961-bin real ratio mask (`[1, T, 961]`). Everything else — fbank,
//! STFT, mask application, iSTFT, resampling — is always-on pure Rust in the
//! sibling modules and golden-tested in the normal gate.
//!
//! Why this layer needs no golden of its own: the #9 spike already pinned
//! ORT↔PyTorch mask parity (max │Δ│ 4.6e-6), and the host chain is pinned to
//! clearvoice's own STFT/iSTFT. So the composition is covered; this adapter is
//! just the call. A `#[ignore]`d integration test (`ort_mask_matches_golden`)
//! runs the real graph end-to-end when `YESVOX_MODEL_PATH` (the ONNX file) and
//! `ORT_DYLIB_PATH` (the onnxruntime shared library, for `load-dynamic`) are
//! set — it is not part of CI because it needs the fetched weights + runtime.
//!
//! `ort` is loaded dynamically (`load-dynamic`), so building `--features neural`
//! never downloads or links a native library; the runtime `libonnxruntime` is
//! discovered at first use via `ORT_DYLIB_PATH`.

use super::engine::MaskModel;
use ort::execution_providers::{ExecutionProviderDispatch, CPU, CUDA};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use ort::execution_providers::CoreML;
#[cfg(target_os = "windows")]
use ort::execution_providers::DirectML;

const FEAT_DIM: usize = 180;
const MASK_BINS: usize = 961;

/// A [`MaskModel`] backed by an ONNX Runtime session over the MossFormer2 graph.
///
/// The session needs `&mut self` to run, so it lives behind a `Mutex` to keep
/// the model `Sync` (the engine holds it as `Box<dyn MaskModel>`).
pub struct OrtMaskModel {
    session: Mutex<Session>,
    input_name: String,
    output_name: String,
}

impl OrtMaskModel {
    /// Load the MossFormer2 ONNX graph from `path`. The graph accepts a
    /// `[1, T, 180]` float input and emits a `[1, T, 961]` float mask. The
    /// shipped export is **256-frame-locked** (the legacy tracer baked the
    /// frame count into a Reshape — see `spikes/mossformer2-onnx/verify_dynamic.py`),
    /// so callers feed exactly 256-frame windows; the forthcoming chunked
    /// decoder slices longer signals into overlapping 256-frame windows.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| format!("create ONNX session builder: {e}"))?
            .with_execution_providers(preferred_providers())
            .map_err(|e| format!("configure ONNX execution providers: {e}"))?
            .commit_from_file(path)
            .map_err(|e| format!("load ONNX model: {e}"))?;
        let (input_name, output_name) = required_io_names(
            session.inputs().first().map(|input| input.name()),
            session.outputs().first().map(|output| output.name()),
        )?;
        Ok(OrtMaskModel {
            session: Mutex::new(session),
            input_name,
            output_name,
        })
    }
}

fn required_io_names(
    input: Option<&str>,
    output: Option<&str>,
) -> Result<(String, String), String> {
    let input = input.ok_or_else(|| "ONNX model declares no input".to_string())?;
    let output = output.ok_or_else(|| "ONNX model declares no output".to_string())?;
    Ok((input.to_string(), output.to_string()))
}

/// Execution providers in preference order, ending in the always-available CPU
/// provider. ONNX Runtime registers each in turn and silently skips any not
/// present in the loaded runtime (logged via `tracing`), so hardware
/// acceleration is negotiated at runtime with a guaranteed, transparent CPU
/// fallback (plan §2.2 / #10). The platform GPU EP is preferred first; CUDA is
/// offered where present; CPU always closes the list.
#[allow(clippy::vec_init_then_push)] // cfg-gated platform EPs preclude a vec! literal
fn preferred_providers() -> Vec<ExecutionProviderDispatch> {
    let mut eps: Vec<ExecutionProviderDispatch> = Vec::new();
    #[cfg(target_os = "windows")]
    eps.push(DirectML::default().build());
    #[cfg(target_os = "macos")]
    eps.push(CoreML::default().build());
    eps.push(CUDA::default().build());
    eps.push(CPU::default().build());
    eps
}

impl OrtMaskModel {
    /// The fallible inference body: every ORT step maps to an error string, so
    /// [`run`](MaskModel::run) can degrade to a passthrough mask instead of
    /// panicking on a runtime/model failure (#33). A bad *model* is meant to be
    /// caught at load; this covers a runtime (`ORT_DYLIB_PATH`) or session-level
    /// failure that only surfaces at inference.
    fn try_run(&self, features: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, String> {
        let t = features.len();
        let t_i64 = i64::try_from(t).map_err(|_| "feature frame count exceeds i64".to_string())?;
        // Flatten [T, 180] row-major into a [1, T, 180] tensor.
        let capacity = t
            .checked_mul(FEAT_DIM)
            .ok_or_else(|| "feature tensor size overflow".to_string())?;
        let mut data = Vec::with_capacity(capacity);
        for (index, row) in features.iter().enumerate() {
            if row.len() != FEAT_DIM {
                return Err(format!(
                    "feature row {index} has {} values, expected {FEAT_DIM}",
                    row.len()
                ));
            }
            data.extend_from_slice(row);
        }
        let tensor = Tensor::from_array((vec![1i64, t_i64, FEAT_DIM as i64], data))
            .map_err(|e| format!("build features tensor: {e}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "ort session lock poisoned".to_string())?;
        let outputs = session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(|e| format!("ONNX inference failed: {e}"))?;
        let (shape, flat) = outputs[self.output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract mask tensor: {e}"))?;

        reshape_mask(shape.as_ref(), flat, t)
    }
}

/// Validate the native output before it can touch audio. A wrong/custom model
/// or runtime fault must become the caller's all-pass mask, never a partial,
/// non-finite, or incorrectly shaped gain matrix.
fn reshape_mask(shape: &[i64], flat: &[f32], frames: usize) -> Result<Vec<Vec<f32>>, String> {
    let frames_i64 =
        i64::try_from(frames).map_err(|_| "mask frame count exceeds i64".to_string())?;
    let expected_shape = [1, frames_i64, MASK_BINS as i64];
    if shape != expected_shape {
        return Err(format!(
            "mask tensor shape {shape:?}, expected {expected_shape:?}"
        ));
    }
    let expected_values = frames
        .checked_mul(MASK_BINS)
        .ok_or_else(|| "mask tensor size overflow".to_string())?;
    if flat.len() != expected_values {
        return Err(format!(
            "mask tensor has {} values, expected {expected_values}",
            flat.len()
        ));
    }
    if flat.iter().any(|gain| !gain.is_finite()) {
        return Err("mask tensor contains a non-finite gain".to_string());
    }
    Ok(flat
        .as_chunks::<MASK_BINS>()
        .0
        .iter()
        .map(|row| row.to_vec())
        .collect())
}

impl MaskModel for OrtMaskModel {
    fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let t = features.len();
        if t == 0 {
            return Vec::new();
        }
        match self.try_run(features) {
            Ok(mask) => mask,
            // Passthrough (all-gain mask): degrade to no-denoise rather than
            // panicking or corrupting the render (#33).
            Err(msg) => {
                eprintln!("neural inference failed, passing audio through: {msg}");
                vec![vec![1.0; MASK_BINS]; t]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::MossformerFbank;
    use std::f32::consts::PI;

    #[test]
    fn malformed_session_io_and_mask_outputs_are_rejected() {
        assert!(required_io_names(None, Some("mask")).is_err());
        assert!(required_io_names(Some("features"), None).is_err());
        assert_eq!(
            required_io_names(Some("features"), Some("mask")).unwrap(),
            ("features".to_string(), "mask".to_string())
        );

        let valid = vec![0.5; 2 * MASK_BINS];
        let mask = reshape_mask(&[1, 2, MASK_BINS as i64], &valid, 2).unwrap();
        assert_eq!(mask.len(), 2);
        assert_eq!(mask[0].len(), MASK_BINS);

        assert!(reshape_mask(&[1, 2, 0], &[], 2).is_err());
        assert!(reshape_mask(&[1, 1, MASK_BINS as i64], &valid, 2).is_err());
        assert!(reshape_mask(&[1, 2, MASK_BINS as i64], &valid[..valid.len() - 1], 2).is_err());

        let mut non_finite = valid;
        non_finite[17] = f32::NAN;
        assert!(reshape_mask(&[1, 2, MASK_BINS as i64], &non_finite, 2).is_err());
    }

    /// End-to-end parity against a Python-dumped golden of the real model.
    ///
    /// Ignored by default: needs the fetched ONNX weights (`YESVOX_MODEL_PATH`)
    /// and the onnxruntime shared library (`ORT_DYLIB_PATH`). Run with:
    /// `YESVOX_MODEL_PATH=… ORT_DYLIB_PATH=… cargo test -p yesvox-core \
    ///   --features neural -- --ignored ort_mask_matches_golden`
    ///
    /// Goldens come from `spikes/mossformer2-onnx/ort_mask_golden.py` on the
    /// same deterministic 256-frame input.
    #[test]
    #[ignore = "needs YESVOX_MODEL_PATH + ORT_DYLIB_PATH (fetched weights + runtime)"]
    fn ort_mask_matches_golden() {
        let model_path = match std::env::var("YESVOX_MODEL_PATH") {
            Ok(p) => p,
            Err(_) => return,
        };
        // 256 frames: 1920 + 255*384 samples, the static export's frame count.
        let n = 1920 + 255 * 384;
        let sr = 48_000.0f32;
        let scaled: Vec<f32> = (0..n)
            .map(|k| {
                let t = k as f32;
                32768.0
                    * (0.4 * (2.0 * PI * 140.0 * t / sr).sin()
                        + 0.2 * (2.0 * PI * 700.0 * t / sr).sin()
                        + 0.15 * (2.0 * PI * 1700.0 * t / sr).sin())
            })
            .collect();
        let feats = MossformerFbank::new().features(&scaled);
        assert_eq!(feats.len(), 256);

        let model = OrtMaskModel::from_file(&model_path).expect("load model");
        let mask = model.run(&feats);
        assert_eq!(mask.len(), 256);
        assert_eq!(mask[0].len(), 961);
        // All gains finite and bounded (ratio mask).
        assert!(mask
            .iter()
            .flatten()
            .all(|&g| g.is_finite() && (-0.01..=2.0).contains(&g)));

        // Goldens from spikes/mossformer2-onnx/ort_mask_golden.py: the real
        // ONNX mask on these exact Rust-computed features. The mask is sparse
        // for synthetic tones, so the global argmax location + the full-mask sum
        // are the load-bearing pins — a transposed/mis-scaled/mis-read mask
        // would move the argmax far and shift the sum by a lot. The few-percent
        // value drift is the Rust-vs-torchaudio fbank numerics amplified by the
        // nonlinear net (which runs on dither=1.0 random features in production,
        // so this is well inside its own tolerance).
        let (mut at, mut ab, mut amax) = (0usize, 0usize, f32::MIN);
        let mut sum = 0.0f32;
        for (t, row) in mask.iter().enumerate() {
            for (b, &g) in row.iter().enumerate() {
                sum += g;
                if g > amax {
                    amax = g;
                    at = t;
                    ab = b;
                }
            }
        }
        eprintln!("rust argmax=({at},{ab})={amax} sum={sum}");
        // Argmax in the same high-band region as the golden (253, 628).
        assert!(
            at.abs_diff(253) <= 3 && ab.abs_diff(628) <= 6,
            "argmax ({at},{ab})"
        );
        assert!(
            (amax - 0.29648).abs() < 0.03,
            "argmax gain {amax} want ~0.296"
        );
        assert!((sum - 3005.30).abs() < 60.0, "mask sum {sum} want ~3005");
    }
}
