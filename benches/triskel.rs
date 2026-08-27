//! Scaling benchmarks for the layered Triskel pipeline.
//!
//! The span series keeps one long edge alongside a rank-by-rank backbone. It
//! makes the per-rank segment-slot/cell expansion visible in benchmark output;
//! it is not an O(|V| + |E|) end-to-end memory claim.

use std::{hint::black_box, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use jstd::{
    Identifier,
    graph::owning::OwningGraph,
    triskel::layout::{LayoutBuilder, NodeGeometry},
};

#[derive(Identifier)]
struct NodeId(usize);
#[derive(Identifier)]
struct EdgeId(usize);

type Graph = OwningGraph<NodeId, EdgeId, (), ()>;

fn long_span_graph(span: usize) -> (Graph, NodeId) {
    let mut graph = Graph::default();
    let nodes: Vec<_> = (0..=span).map(|_| graph.make_node(())).collect();
    for pair in nodes.windows(2) {
        graph.make_edge(pair[0], pair[1], ());
    }
    graph.make_edge(nodes[0], nodes[span], ());
    (graph, nodes[0])
}

fn rank_span(c: &mut Criterion) {
    let mut group = c.benchmark_group("triskel_rank_span");
    for span in [8usize, 32, 128] {
        let (graph, root) = long_span_graph(span);
        group.bench_with_input(BenchmarkId::new("layout", span), &span, |bench, _| {
            bench.iter(|| {
                black_box(
                    LayoutBuilder::new(&graph)
                        .root(root)
                        .max_sweeps(4)
                        .geometry(|_| NodeGeometry {
                            width: 40.0,
                            height: 30.0,
                        })
                        .build()
                        .unwrap(),
                )
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = triskel;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(3));
    targets = rank_span
}
criterion_main!(triskel);
