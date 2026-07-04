use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::hint::black_box;
use yesvox_core::{
    AudioBuffer, Chain, DeClick, DeEss, DeEssMode, DeHum, DeReverb, Denoise, SampleEncoding,
    VocalProcessor,
};

const SR: u32 = 48_000;
const MODULE_SECONDS: usize = 10;
const FULL_CHAIN_SECONDS: usize = 30;

fn bench_modules(c: &mut Criterion) {
    let mut group = c.benchmark_group("modules_10s");
    group.sample_size(10);

    let input = fixture(MODULE_SECONDS);
    group.bench_function("declick", |b| {
        b.iter_batched(
            || (DeClick::new(), input.clone()),
            |(mut module, mut buffer)| run_module(&mut module, &mut buffer),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("dehum", |b| {
        b.iter_batched(
            || (DeHum::default(), input.clone()),
            |(mut module, mut buffer)| run_module(&mut module, &mut buffer),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("dereverb", |b| {
        b.iter_batched(
            || (DeReverb::voiceover(), input.clone()),
            |(mut module, mut buffer)| run_module(&mut module, &mut buffer),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("deess_split", |b| {
        b.iter_batched(
            || (DeEss::new(DeEssMode::Split), input.clone()),
            |(mut module, mut buffer)| run_module(&mut module, &mut buffer),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("denoise_light", |b| {
        b.iter_batched(
            || (Denoise::new(1), input.clone()),
            |(mut module, mut buffer)| run_module(&mut module, &mut buffer),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_full_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_chain_30s");
    group.sample_size(10);
    let input = fixture(FULL_CHAIN_SECONDS);

    group.bench_function("voiceover_natural", |b| {
        b.iter_batched(
            || (Chain::voiceover_default(1), input.clone()),
            |(mut chain, mut buffer)| run_module(&mut chain, &mut buffer),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn run_module(module: &mut dyn VocalProcessor, buffer: &mut AudioBuffer) {
    module.prepare(buffer.sample_rate(), buffer.frames(), buffer.layout());
    module.reset();
    module.process(buffer);
    black_box(buffer.channel(0));
}

fn fixture(seconds: usize) -> AudioBuffer {
    let frames = SR as usize * seconds;
    let mut samples = Vec::with_capacity(frames);
    let mut seed = 0x5658_4245u32; // "VXBE"
    for i in 0..frames {
        let t = i as f32 / SR as f32;
        let envelope = 0.6 + 0.25 * (2.0 * std::f32::consts::PI * 1.1 * t).sin().abs();
        let body = 0.25 * (2.0 * std::f32::consts::PI * 180.0 * t).sin();
        let harmonic = 0.08 * (2.0 * std::f32::consts::PI * 360.0 * t).sin();
        let sibilance = 0.035 * (2.0 * std::f32::consts::PI * 7_200.0 * t).sin();
        let hum = 0.008 * (2.0 * std::f32::consts::PI * 60.0 * t).sin();
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.004;
        let click = if i % 151_331 == 0 { 0.35 } else { 0.0 };
        samples.push(
            (envelope * (body + harmonic + sibilance) + hum + noise + click).clamp(-0.98, 0.98),
        );
    }
    AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![samples])
}

criterion_group!(benches, bench_modules, bench_full_chain);
criterion_main!(benches);
