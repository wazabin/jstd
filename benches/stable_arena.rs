use std::{hint::black_box, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use jstd::{Identifier, registry::Registry, stable_arena::StableArena};

#[derive(Identifier)]
struct Id(usize);

const N: usize = 100_000;

fn access_trace() -> Vec<Id> {
    let mut state = 0x1234_5678_9abc_def0_u64;
    (0..N)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            Id::from(state as usize % N)
        })
        .collect()
}

fn lookup(c: &mut Criterion) {
    let registry: Registry<Id, u64> = (0..N as u64).collect();
    let arena: StableArena<Id, u64> = (0..N as u64).collect();
    let trace = access_trace();
    let mut group = c.benchmark_group("stable_arena_lookup");
    group.bench_function("registry", |b| {
        b.iter(|| {
            let mut sum = 0_u64;
            for &id in &trace {
                sum = sum.wrapping_add(registry[black_box(id)]);
            }
            black_box(sum)
        })
    });
    group.bench_function("stable_arena", |b| {
        b.iter(|| {
            let mut sum = 0_u64;
            for &id in &trace {
                sum = sum.wrapping_add(arena[black_box(id)]);
            }
            black_box(sum)
        })
    });
    group.finish();
}

fn dense_traversal(c: &mut Criterion) {
    let registry: Registry<Id, u64> = (0..N as u64).collect();
    let arena: StableArena<Id, u64> = (0..N as u64).collect();
    let mut group = c.benchmark_group("stable_arena_dense_traversal");
    group.bench_function("registry", |b| {
        b.iter(|| {
            black_box(
                registry
                    .iter()
                    .fold(0_u64, |sum, entry| sum.wrapping_add(**entry)),
            )
        })
    });
    group.bench_function("stable_arena", |b| {
        b.iter(|| {
            black_box(
                arena
                    .iter()
                    .fold(0_u64, |sum, entry| sum.wrapping_add(**entry)),
            )
        })
    });
    group.finish();
}

fn sparse_traversal(c: &mut Criterion) {
    let mut registry: Registry<Id, (bool, u64)> =
        (0..N as u64).map(|value| (true, value)).collect();
    let mut arena: StableArena<Id, u64> = (0..N as u64).collect();
    for raw in 0..N * 9 / 10 {
        registry[Id::from(raw)].0 = false;
        arena.remove(Id::from(raw));
    }

    let mut group = c.benchmark_group("stable_arena_90_percent_deleted_traversal");
    group.bench_function("registry_tombstones", |b| {
        b.iter(|| {
            black_box(
                registry
                    .iter()
                    .filter(|entry| entry.0)
                    .fold(0_u64, |sum, entry| sum.wrapping_add(entry.1)),
            )
        })
    });
    group.bench_function("stable_arena", |b| {
        b.iter(|| {
            black_box(
                arena
                    .iter()
                    .fold(0_u64, |sum, entry| sum.wrapping_add(**entry)),
            )
        })
    });
    group.finish();
}

fn mixed_mutation(c: &mut Criterion) {
    c.bench_function("stable_arena_mixed_push_remove_lookup", |b| {
        b.iter(|| {
            let mut arena = StableArena::<Id, u64>::default();
            let mut ids = Vec::with_capacity(10_000);
            for value in 0..10_000_u64 {
                let id = arena.push(value);
                ids.push(id);
                if value % 3 == 2 {
                    arena.remove(ids[ids.len() - 2]);
                } else {
                    black_box(arena[id]);
                }
            }
            black_box(arena)
        })
    });
}

fn serde_holes(c: &mut Criterion) {
    let registry: Registry<Id, Option<u64>> = (0..N as u64)
        .map(|value| (value >= (N * 9 / 10) as u64).then_some(value))
        .collect();
    let mut arena: StableArena<Id, u64> = (0..N as u64).collect();
    for raw in 0..N * 9 / 10 {
        arena.remove(Id::from(raw));
    }
    let registry_bytes =
        bincode::serde::encode_to_vec(&registry, bincode::config::standard()).unwrap();
    let arena_bytes = bincode::serde::encode_to_vec(&arena, bincode::config::standard()).unwrap();
    println!(
        "hole-heavy serialized bytes: registry_tombstones={}, stable_arena={}",
        registry_bytes.len(),
        arena_bytes.len()
    );

    let mut group = c.benchmark_group("stable_arena_hole_heavy_serde_round_trip");
    group.bench_function("registry_tombstones", |b| {
        b.iter(|| {
            let bytes =
                bincode::serde::encode_to_vec(black_box(&registry), bincode::config::standard())
                    .unwrap();
            let decoded: (Registry<Id, Option<u64>>, usize) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
            black_box(decoded)
        })
    });
    group.bench_function("stable_arena", |b| {
        b.iter(|| {
            let bytes =
                bincode::serde::encode_to_vec(black_box(&arena), bincode::config::standard())
                    .unwrap();
            let decoded: (StableArena<Id, u64>, usize) =
                bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
            black_box(decoded)
        })
    });
    group.finish();
}

fn benches(c: &mut Criterion) {
    lookup(c);
    dense_traversal(c);
    sparse_traversal(c);
    mixed_mutation(c);
    serde_holes(c);
}

criterion_group! {
    name = stable_arena;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));
    targets = benches
}
criterion_main!(stable_arena);
