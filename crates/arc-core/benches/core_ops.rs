use arc_core::store::cas::ObjectStore;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

fn bench_cas_blob_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("arc_core_cas");
    let sizes = [64usize, 1024, 16 * 1024];

    for size in sizes {
        let fixture = vec![0xAB; size];
        let temp = tempfile::tempdir().expect("tempdir");
        let store = ObjectStore::new(temp.path());

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("write_blob", size), &fixture, |b, input| {
            b.iter(|| {
                let hash = store.write_blob(input).expect("write blob");
                black_box(hash);
            });
        });

        let hash = store.write_blob(&fixture).expect("seed write blob");
        group.bench_with_input(BenchmarkId::new("read_blob", size), &hash, |b, h| {
            b.iter(|| {
                let bytes = store.read_blob(h).expect("read blob");
                black_box(bytes.len());
            });
        });
    }

    group.finish();
}

criterion_group!(core_ops, bench_cas_blob_roundtrip);
criterion_main!(core_ops);
