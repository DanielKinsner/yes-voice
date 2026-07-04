//! Validate the RAM-preflight working-set constant against a *measured* render
//! peak (#32). A counting global allocator records the peak bytes live during a
//! real render; the preflight estimate (working-set K × buffer bytes) must be a
//! safe upper bound, so the enforcing preflight never under-estimates.
//!
//! This is the whole test binary so the global allocator's peak reflects one
//! render, not other tests running concurrently.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use yesvox_core::denoise::ChunkPolicy;
use yesvox_core::{write_wav, AudioBuffer, SampleEncoding, Session};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static MEASURE_LOCK: Mutex<()> = Mutex::new(());

struct Counting;

// SAFETY: forwards every call to the System allocator; only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn temp(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-mem-{tag}-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn working_set_k_is_a_safe_upper_bound_for_the_measured_render_peak() {
    assert_working_set_k_covers_render(96_000, "short");
}

fn assert_working_set_k_covers_render(frames: usize, tag: &str) {
    let _lock = MEASURE_LOCK.lock().expect("lock memory-budget measurement");
    let path = temp(tag);
    let x: Vec<f32> = (0..frames).map(|i| 0.2 * (i as f32 * 0.02).sin()).collect();
    write_wav(
        &path,
        &AudioBuffer::new(48_000, 16, SampleEncoding::Int, vec![x]),
    )
    .expect("write fixture");

    let mut session = Session::new();
    session.open(&path).expect("open");

    // Baseline includes the decoded input buffer already in memory; measure only
    // what the render adds on top.
    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    session.render().expect("render");
    let render_peak = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);

    // The preflight's estimate for this buffer (working-set K × buffer bytes).
    let estimate = ChunkPolicy::default()
        .preflight(frames, 1, 48_000.0, u64::MAX)
        .estimated_bytes as usize;

    assert!(render_peak > 0, "the render allocated nothing?");
    assert!(
        estimate >= render_peak,
        "working_set_k under-estimates: measured render peak {render_peak} B > estimate {estimate} B — raise ChunkPolicy::working_set_k"
    );
    eprintln!(
        "render peak {} KB, estimate {} KB ({:.1}x headroom)",
        render_peak / 1024,
        estimate / 1024,
        estimate as f64 / render_peak as f64
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn opt_in_long_material_validates_working_set_k() {
    if std::env::var("YESVOX_LONG_MEMORY_BUDGET").as_deref() != Ok("1") {
        eprintln!("skipping long memory-budget check; set YESVOX_LONG_MEMORY_BUDGET=1");
        return;
    }

    // Two minutes mono at 48 kHz: still local-only/opt-in, but large enough to
    // exercise the multi-minute memory shape that the default 2 s fixture cannot.
    assert_working_set_k_covers_render(48_000 * 120, "long");
}
