//! NO-DOUBLE-REPAIR (plan §3.2, collision rule #2): de-click (which owns
//! `Click` | `MouthClick`) and de-pop (which owns `Plosive`) must never repair
//! the same sample. The shared detector emits disjoint, singly-classified
//! events, so the two modules' ownership partitions can never overlap. This gate
//! locks that property on a signal that carries **both** a sharp click and a
//! low-frequency plosive, so it cannot pass vacuously.

use std::f32::consts::PI;
use yesvox_core::modules::transient::{TransientDetector, TransientKind};

const SR: f32 = 48_000.0;

fn declick_owns(kind: TransientKind) -> bool {
    matches!(kind, TransientKind::Click | TransientKind::MouthClick)
}

/// Voiced 200 Hz body + one fast-onset low-frequency plosive thump + one
/// single-sample click, well separated.
fn click_and_plosive(n: usize) -> Vec<f32> {
    let mut x: Vec<f32> = (0..n)
        .map(|i| 0.30 * (2.0 * PI * 200.0 * i as f32 / SR).sin())
        .collect();

    // Plosive at ~10 000: an 80 Hz burst, ~30 ms, with a ~0.5 ms attack so the
    // onset trips the detector and the body is dominated by sub-150 Hz energy.
    let p0 = 10_000usize;
    let burst = (0.030 * SR) as usize; // ~30 ms
    let attack = (0.0005 * SR) as usize; // ~0.5 ms
    for k in 0..burst {
        if p0 + k >= n {
            break;
        }
        let env = (k as f32 / attack as f32).min(1.0);
        x[p0 + k] += 0.5 * env * (2.0 * PI * 80.0 * k as f32 / SR).sin();
    }

    // Sharp single-sample click, far from the plosive.
    if n > 30_000 {
        x[30_000] += 0.85;
    }
    x
}

#[test]
fn declick_and_depop_never_repair_the_same_sample() {
    let x = click_and_plosive(48_000);
    let events = TransientDetector::default().detect(&x, SR);

    // The signal really does carry both defect classes (non-vacuous).
    assert!(
        events.iter().any(|e| e.kind == TransientKind::Plosive),
        "fixture should produce a plosive (de-pop's job)"
    );
    assert!(
        events.iter().any(|e| declick_owns(e.kind)),
        "fixture should produce a click (de-click's job)"
    );

    // No sample is covered by both a de-click-owned and a de-pop-owned event.
    let n = x.len();
    let mut by_declick = vec![false; n];
    let mut by_depop = vec![false; n];
    for e in &events {
        let covered = if declick_owns(e.kind) {
            &mut by_declick
        } else {
            &mut by_depop
        };
        for s in covered.iter_mut().take(e.end.min(n)).skip(e.start.min(n)) {
            *s = true;
        }
    }
    for i in 0..n {
        assert!(
            !(by_declick[i] && by_depop[i]),
            "sample {i} would be repaired by both de-click and de-pop"
        );
    }
}
