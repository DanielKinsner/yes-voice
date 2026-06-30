//! Versioned, serializable settings (plan §1.1 `ParamTree`).
//!
//! The settings file is versioned (`schema_version`) and **reserves the singing
//! subtree on day one** so the format never breaks when v1.x turns singing on
//! (plan §6 release-gate fallback). v1 ships voiceover-only; the `singing` slot
//! is present, defaulted, and otherwise inert until the singing gate (#16)
//! passes. Unknown future fields decode via `#[serde(default)]`, so an older
//! build can still read a newer file's shared fields.

use crate::audio::SampleEncoding;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Current settings schema version. Bump only on a breaking layout change;
/// additive fields use `#[serde(default)]` and do **not** bump this.
pub const SCHEMA_VERSION: u32 = 1;

/// User-authoritative content mode (plan §5.2). Voiceover vs Singing is an
/// explicit toggle, assisted by — never replaced by — auto-detection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentMode {
    /// Spoken-word restoration. The v1 default.
    #[default]
    Voiceover,
    /// Sung-vocal restoration. Reserved; only selectable once the singing gate
    /// (#16) passes — see [`SingingSettings`].
    Singing,
}

/// The **reserved** singing subtree (plan §6). Present from day one so the
/// schema is stable, but inert in v1: `unlocked` stays `false` until the
/// singing release gate (#16) is passed and the UI is wired (#17).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SingingSettings {
    /// Whether singing mode has been unlocked by the #16 quality gate. Always
    /// `false` in v1 — the slot exists only to keep the schema forward-stable.
    #[serde(default)]
    pub unlocked: bool,
    /// Default tail-preservation strength for sung sustains (plan §4.1). Stored
    /// now so de-reverb defaults don't force a schema bump later.
    #[serde(default = "default_tail_preservation")]
    pub tail_preservation: f32,
}

fn default_tail_preservation() -> f32 {
    0.8
}

impl Default for SingingSettings {
    fn default() -> Self {
        SingingSettings {
            unlocked: false,
            tail_preservation: default_tail_preservation(),
        }
    }
}

/// Export-stage preferences (plan §1.2 / §5.6): bit depth and dither.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    /// Override the export bit depth; `None` honors the source file's depth.
    #[serde(default)]
    pub bit_depth_override: Option<u16>,
    /// Apply TPDF dither on ≤24-bit integer export (plan §1.2). On by default.
    #[serde(default = "default_true")]
    pub dither: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            bit_depth_override: None,
            dither: true,
        }
    }
}

impl ExportSettings {
    /// Resolve these settings to the concrete [`ExportOptions`] used by the
    /// writer, given the source buffer's format.
    pub fn to_options(&self, source_bits: u16, source_encoding: SampleEncoding) -> ExportOptions {
        let bits = self.bit_depth_override.unwrap_or(source_bits);
        ExportOptions {
            bit_depth: bits,
            encoding: source_encoding,
            // Dither only ever applies to ≤24-bit integer export; float export
            // is already lossless, and 32-bit int is below the noise floor.
            dither: self.dither && source_encoding == SampleEncoding::Int && bits <= 24,
        }
    }
}

/// Concrete, resolved export options handed to the writer ([`crate::io`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    /// Target bit depth.
    pub bit_depth: u16,
    /// Target int/float encoding.
    pub encoding: SampleEncoding,
    /// Whether to apply TPDF dither before quantization.
    pub dither: bool,
}

/// The whole versioned settings document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Schema version of this document (see [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Active content mode (voiceover in v1).
    #[serde(default)]
    pub content_mode: ContentMode,
    /// Export preferences.
    #[serde(default)]
    pub export: ExportSettings,
    /// Reserved singing subtree (plan §6); inert in v1.
    #[serde(default)]
    pub singing: SingingSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema_version: SCHEMA_VERSION,
            content_mode: ContentMode::default(),
            export: ExportSettings::default(),
            singing: SingingSettings::default(),
        }
    }
}

/// Errors loading or saving the settings file.
#[derive(Debug)]
pub enum SettingsError {
    /// Filesystem error.
    Io(std::io::Error),
    /// JSON (de)serialization error.
    Json(serde_json::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "settings io error: {e}"),
            SettingsError::Json(e) => write!(f, "settings json error: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<std::io::Error> for SettingsError {
    fn from(e: std::io::Error) -> Self {
        SettingsError::Io(e)
    }
}

impl From<serde_json::Error> for SettingsError {
    fn from(e: serde_json::Error) -> Self {
        SettingsError::Json(e)
    }
}

impl Settings {
    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> Result<String, SettingsError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse from JSON, tolerating missing additive fields via `serde(default)`.
    pub fn from_json(s: &str) -> Result<Self, SettingsError> {
        Ok(serde_json::from_str(s)?)
    }

    /// Load settings from `path`, or return [`Settings::default`] if it does not
    /// exist (first launch).
    pub fn load_or_default(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_json(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::Io(e)),
        }
    }

    /// Save settings to `path` atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let json = self.to_json()?;
        let tmp = settings_temp_path(path);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Sibling temp path for the atomic settings write.
fn settings_temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yesvox-tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_versioned_voiceover_with_reserved_singing_slot() {
        let s = Settings::default();
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        assert_eq!(s.content_mode, ContentMode::Voiceover);
        // The singing slot exists and is reserved/inert in v1.
        assert!(!s.singing.unlocked, "singing must ship locked in v1");
    }

    #[test]
    fn json_round_trips() {
        let s = Settings::default();
        let json = s.to_json().unwrap();
        let back = Settings::from_json(&json).unwrap();
        assert_eq!(s, back);
        // The serialized form actually carries the version and the singing slot.
        assert!(json.contains("\"schema_version\""));
        assert!(json.contains("\"singing\""));
    }

    #[test]
    fn missing_additive_fields_decode_to_defaults() {
        // An older/minimal file carrying only the version still loads, and the
        // reserved singing slot fills in from defaults (forward stability).
        let minimal = r#"{ "schema_version": 1 }"#;
        let s = Settings::from_json(minimal).unwrap();
        assert_eq!(s.content_mode, ContentMode::Voiceover);
        assert!(!s.singing.unlocked);
        assert!(s.export.dither);
    }

    #[test]
    fn export_settings_resolve_dither_only_for_small_int() {
        let e = ExportSettings::default();
        // ≤24-bit int → dither on.
        assert!(e.to_options(16, SampleEncoding::Int).dither);
        assert!(e.to_options(24, SampleEncoding::Int).dither);
        // 32-bit int and float → no dither.
        assert!(!e.to_options(32, SampleEncoding::Int).dither);
        assert!(!e.to_options(32, SampleEncoding::Float).dither);
    }
}
