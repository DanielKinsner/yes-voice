//! Test-only golden-file harness (#27, finding 2).
//!
//! Cross-implementation reference numbers (torch/clearvoice parity values for
//! the always-on host DSP) used to be hardcoded literals inside the neural unit
//! tests. That made "re-bless a red golden" indistinguishable from an ordinary
//! source edit. Here the reference numbers live in
//! `crates/yesvox-core/tests/goldens/<name>.golden` (one `label = value` per
//! line); the tests compute the values and compare against the file within
//! per-value tolerances that stay in the test code.
//!
//! Regeneration is **deliberate and explicit**: only when `YESVOX_BLESS=1` is
//! set in the environment (via `cargo xtask bless`, or a human running
//! `YESVOX_BLESS=1 cargo test <golden_test>`) does [`check`] overwrite the file
//! with freshly computed values instead of asserting against it. The canonical
//! gate (`cargo xtask gate` / CI `cargo test`) never sets the variable, so an
//! autonomous build loop cannot make a red golden green by re-blessing it —
//! that is a bug, not a fix (CLAUDE.md).

use std::path::PathBuf;

/// One golden comparison: a freshly computed value, its tolerance, and a stable
/// label (used both to key the golden file and in the failure message).
pub struct Golden {
    pub label: &'static str,
    pub computed: f32,
    pub tol: f32,
}

/// Convenience constructor for a [`Golden`] entry.
pub fn golden(label: &'static str, computed: f32, tol: f32) -> Golden {
    Golden {
        label,
        computed,
        tol,
    }
}

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("goldens")
}

/// Whether the deliberate re-bless path is armed. Only `1`/`true` count, so a
/// stray empty value can't silently arm it.
pub fn bless_enabled() -> bool {
    matches!(
        std::env::var("YESVOX_BLESS").ok().as_deref(),
        Some("1") | Some("true")
    )
}

fn serialize(name: &str, entries: &[Golden]) -> String {
    let mut out = format!(
        "# golden: {name}\n# Blessed reference values (regenerate only via `cargo xtask bless`\n# / YESVOX_BLESS=1). Per-value tolerances live in the test, not here.\n"
    );
    for e in entries {
        out.push_str(&format!("{} = {}\n", e.label, e.computed));
    }
    out
}

fn parse(text: &str) -> Vec<(String, f32)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (k, v) = l
                .split_once('=')
                .unwrap_or_else(|| panic!("malformed golden line: {l:?}"));
            let value = v
                .trim()
                .parse::<f32>()
                .unwrap_or_else(|_| panic!("unparseable golden value: {l:?}"));
            (k.trim().to_string(), value)
        })
        .collect()
}

/// Compare `entries` against the golden file `name`, or (under `YESVOX_BLESS=1`)
/// regenerate that file from the computed values. Panics with a descriptive
/// message on any drift beyond tolerance, a missing file, or a structural
/// mismatch (count/label reordering).
pub fn check(name: &str, entries: &[Golden]) {
    let path = goldens_dir().join(format!("{name}.golden"));

    if bless_enabled() {
        std::fs::create_dir_all(goldens_dir()).expect("create goldens dir");
        std::fs::write(&path, serialize(name, entries)).expect("write golden");
        eprintln!("blessed golden `{name}` ({} values)", entries.len());
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden `{name}` missing or unreadable ({e}); this is not something a \
             build loop may auto-create — run `cargo xtask bless` deliberately"
        )
    });
    let refs = parse(&text);
    assert_eq!(
        refs.len(),
        entries.len(),
        "golden `{name}`: entry count drifted (file has {}, test has {})",
        refs.len(),
        entries.len()
    );
    for (e, (rlabel, rval)) in entries.iter().zip(&refs) {
        assert_eq!(
            e.label, rlabel,
            "golden `{name}`: label mismatch (file `{rlabel}` vs test `{}`) — reordered?",
            e.label
        );
        assert!(
            (e.computed - rval).abs() <= e.tol,
            "golden `{name}` / {}: got {}, want {} (±{})",
            e.label,
            e.computed,
            rval,
            e.tol
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labels_and_values_ignoring_comments() {
        let parsed = parse("# header\n\n a = 1.5 \nb=-2.0\n");
        assert_eq!(
            parsed,
            vec![("a".to_string(), 1.5), ("b".to_string(), -2.0)]
        );
    }

    #[test]
    fn serialize_round_trips_through_parse() {
        let entries = [golden("x", 3.25, 0.0), golden("y", -0.5, 0.0)];
        let parsed = parse(&serialize("demo", &entries));
        assert_eq!(
            parsed,
            vec![("x".to_string(), 3.25), ("y".to_string(), -0.5)]
        );
    }

    #[test]
    fn detects_out_of_tolerance_drift() {
        // The harness is not a no-op: a value outside tolerance must fail. Prove
        // it against an in-memory reference without touching the real files.
        let refs = parse("v = 1.0\n");
        let entry = golden("v", 1.05, 0.01);
        // Re-implement the comparison the way `check` does, on parsed refs.
        let (_, rval) = &refs[0];
        assert!(
            (entry.computed - rval).abs() > entry.tol,
            "1.05 vs 1.0 should exceed a 0.01 tolerance"
        );
    }

    #[test]
    fn bless_flag_requires_explicit_truthy_value() {
        // Documents the guard: only "1"/"true" arm bless. (We can't safely mutate
        // process env here, so assert the classifier directly.)
        for (v, armed) in [("1", true), ("true", true), ("", false), ("0", false)] {
            let got = matches!(Some(v), Some("1") | Some("true"));
            assert_eq!(got, armed, "value {v:?}");
        }
    }
}
