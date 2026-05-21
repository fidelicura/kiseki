#![allow(missing_docs)]

//! Microbenchmarks for [`kiseki::Trace`].
//!
//! Each scenario is run across a 2x2 matrix of `VALIDATE` and
//! `SANITIZE` settings to quantify the cost of lifecycle tracking
//! and label sanitization independently.

use criterion::{BenchmarkId, Criterion, Throughput};

macro_rules! cfg {
    ($name:ident, $out:ty, $validate:expr, $sanitize:expr) => {
        struct $name;

        impl kiseki::Config for $name {
            type Output = $out;
            type Stage = &'static str;
            const VALIDATE: bool = $validate;
            const SANITIZE: bool = $sanitize;
        }
    };
}

cfg!(VsVec, Vec<u8>, true, true);
cfg!(VnVec, Vec<u8>, true, false);
cfg!(NsVec, Vec<u8>, false, true);
cfg!(NnVec, Vec<u8>, false, false);

cfg!(VsSink, Sink, true, true);
cfg!(VnSink, Sink, true, false);
cfg!(NsSink, Sink, false, true);
cfg!(NnSink, Sink, false, false);

cfg!(VsFile, std::io::BufWriter<std::fs::File>, true, true);
cfg!(VnFile, std::io::BufWriter<std::fs::File>, true, false);
cfg!(NsFile, std::io::BufWriter<std::fs::File>, false, true);
cfg!(NnFile, std::io::BufWriter<std::fs::File>, false, false);

/// Sink that drops every byte. Removes allocation noise from the
/// `Vec<u8>` growth path so benchmarks isolate the encoding cost.
#[derive(Default)]
struct Sink;

impl std::io::Write for Sink {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        criterion::black_box(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        criterion::black_box(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const STAGES: [&str; 5] = ["F", "D", "X", "M", "W"];
const LABEL: &str = "0x1000: add r1, r2, r3";

/// Emit `n` instructions through a 5-stage pipeline, one instruction
/// per cycle, with one label per instruction. Returns the produced
/// byte count for throughput reporting.
fn pipeline_into_vec<C>(n: u32, output: C::Output) -> usize
where
    C: kiseki::Config<Stage = &'static str>,
    C::Output: AsRef<[u8]>,
{
    let mut trace = kiseki::Trace::<C>::new(0, output).unwrap();
    for i in 0..n {
        let id = trace.start(i, 0).unwrap();
        trace.label(id, LABEL, kiseki::Level::Pane).unwrap();
        for &stage in &STAGES {
            trace.stage(id, 0, &stage, false).unwrap();
            trace.advance(1).unwrap();
        }
        trace.retire(id).unwrap();
    }
    let bytes = trace.finish().unwrap();
    bytes.as_ref().len()
}

/// Same as [`pipeline_into_vec`] but drives a non-`AsRef<[u8]>` sink
/// so the caller doesn't pay for the allocation. Used by sink
/// benches.
fn pipeline_into_sink<C>(n: u32)
where
    C: kiseki::Config<Stage = &'static str, Output = Sink>,
{
    let mut trace = kiseki::Trace::<C>::new(0, Sink).unwrap();
    for i in 0..n {
        let id = trace.start(i, 0).unwrap();
        trace.label(id, LABEL, kiseki::Level::Pane).unwrap();
        for &stage in &STAGES {
            trace.stage(id, 0, &stage, false).unwrap();
            trace.advance(1).unwrap();
        }
        trace.retire(id).unwrap();
    }
    let _ = trace.finish().unwrap();
}

/// Names match the matrix: `validate_sanitize`, `validate_only`,
/// `sanitize_only`, `neither`.
const ARMS: [&str; 4] = [
    "validate_sanitize",
    "validate_only",
    "sanitize_only",
    "neither",
];

const NUMBER: &[u32; 2] = &[1_000, 100_000];

fn bench_pipeline_vec(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_to_vec");
    for &n in NUMBER {
        let bytes = pipeline_into_vec::<VsVec>(n, Vec::with_capacity(64 * n as usize));
        group.throughput(Throughput::Bytes(bytes as u64));

        group.bench_with_input(BenchmarkId::new(ARMS[0], n), &n, |b, &n| {
            b.iter(|| {
                let out = Vec::with_capacity(64 * n as usize);
                criterion::black_box(pipeline_into_vec::<VsVec>(criterion::black_box(n), out));
            });
        });
        group.bench_with_input(BenchmarkId::new(ARMS[1], n), &n, |b, &n| {
            b.iter(|| {
                let out = Vec::with_capacity(64 * n as usize);
                criterion::black_box(pipeline_into_vec::<VnVec>(criterion::black_box(n), out));
            });
        });
        group.bench_with_input(BenchmarkId::new(ARMS[2], n), &n, |b, &n| {
            b.iter(|| {
                let out = Vec::with_capacity(64 * n as usize);
                criterion::black_box(pipeline_into_vec::<NsVec>(criterion::black_box(n), out));
            });
        });
        group.bench_with_input(BenchmarkId::new(ARMS[3], n), &n, |b, &n| {
            b.iter(|| {
                let out = Vec::with_capacity(64 * n as usize);
                criterion::black_box(pipeline_into_vec::<NnVec>(criterion::black_box(n), out));
            });
        });
    }
    group.finish();
}

fn bench_pipeline_sink(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_to_sink");
    for &n in NUMBER {
        group.bench_with_input(BenchmarkId::new(ARMS[0], n), &n, |b, &n| {
            b.iter(|| pipeline_into_sink::<VsSink>(criterion::black_box(n)));
        });
        group.bench_with_input(BenchmarkId::new(ARMS[1], n), &n, |b, &n| {
            b.iter(|| pipeline_into_sink::<VnSink>(criterion::black_box(n)));
        });
        group.bench_with_input(BenchmarkId::new(ARMS[2], n), &n, |b, &n| {
            b.iter(|| pipeline_into_sink::<NsSink>(criterion::black_box(n)));
        });
        group.bench_with_input(BenchmarkId::new(ARMS[3], n), &n, |b, &n| {
            b.iter(|| pipeline_into_sink::<NnSink>(criterion::black_box(n)));
        });
    }
    group.finish();
}

/// Same as [`pipeline_into_vec`] but drives a `BufWriter<File>` so
/// the benchmark exercises a realistic on-disk sink. The caller
/// supplies a freshly opened, truncated file so each iteration starts
/// from a known state and amortizes the open/create cost outside the
/// hot loop. The buffer is explicitly flushed by [`Trace::finish`]'s
/// drop of the writer; we additionally call `flush` to surface any
/// IO failure as a panic instead of silently dropping bytes.
/// File handle paired with its on-disk path. The path is unlinked
/// when this guard is dropped so benchmark iterations do not leak
/// temp files into `/tmp`.
struct TmpFile {
    file: Option<std::fs::File>,
    path: std::path::PathBuf,
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn pipeline_into_file<C>(n: u32, mut tmp: TmpFile)
where
    C: kiseki::Config<Stage = &'static str, Output = std::io::BufWriter<std::fs::File>>,
{
    let file = tmp.file.take().unwrap();
    let writer = std::io::BufWriter::new(file);
    let mut trace = kiseki::Trace::<C>::new(0, writer).unwrap();
    for i in 0..n {
        let id = trace.start(i, 0).unwrap();
        trace.label(id, LABEL, kiseki::Level::Pane).unwrap();
        for &stage in &STAGES {
            trace.stage(id, 0, &stage, false).unwrap();
            trace.advance(1).unwrap();
        }
        trace.retire(id).unwrap();
    }
    let mut writer = trace.finish().unwrap();
    std::io::Write::flush(&mut writer).unwrap();
}

fn fresh_tmp_file(tag: &str) -> TmpFile {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "kiseki-bench-{tag}-{}-{seq}.kanata",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    TmpFile {
        file: Some(file),
        path,
    }
}

fn bench_pipeline_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_to_file");
    for &n in NUMBER {
        let bytes = pipeline_into_vec::<VsVec>(n, Vec::with_capacity(64 * n as usize));
        group.throughput(Throughput::Bytes(bytes as u64));

        group.bench_with_input(BenchmarkId::new(ARMS[0], n), &n, |b, &n| {
            b.iter_batched(
                || fresh_tmp_file("vs"),
                |f| pipeline_into_file::<VsFile>(criterion::black_box(n), f),
                criterion::BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new(ARMS[1], n), &n, |b, &n| {
            b.iter_batched(
                || fresh_tmp_file("vn"),
                |f| pipeline_into_file::<VnFile>(criterion::black_box(n), f),
                criterion::BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new(ARMS[2], n), &n, |b, &n| {
            b.iter_batched(
                || fresh_tmp_file("ns"),
                |f| pipeline_into_file::<NsFile>(criterion::black_box(n), f),
                criterion::BatchSize::SmallInput,
            );
        });
        group.bench_with_input(BenchmarkId::new(ARMS[3], n), &n, |b, &n| {
            b.iter_batched(
                || fresh_tmp_file("nn"),
                |f| pipeline_into_file::<NnFile>(criterion::black_box(n), f),
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion::criterion_group!(
    benches,
    bench_pipeline_vec,
    bench_pipeline_sink,
    bench_pipeline_file,
);
criterion::criterion_main!(benches);
