//! Scaling benchmarks for the layered Triskel pipeline.
//!
//! Graph objects use O(|V| + |E|) storage plus O(long edges) p/q endpoints.
//! Ordering/coordinates materialise O(|V| + S) transient slots/cells, where S
//! is the sum of active long-segment spans. Boundary crossing counting is
//! quadratic in the number of boundary connections in this implementation;
//! transpose repeatedly evaluates that objective and is consequently intended
//! for small/medium ranks. Orthogonal routing is linear in waypoints plus the
//! per-channel interval-overlap lane assignment (quadratic in contested jogs).

use std::{hint::black_box, time::Duration};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use jstd::{
    Identifier,
    graph::owning::OwningGraph,
    triskel::{
        layout::{LayoutBuilder, NodeGeometry},
        router::EdgeStyle,
    },
};

#[derive(Identifier)]
struct NodeId(usize);
#[derive(Identifier)]
struct EdgeId(usize);
type Graph = OwningGraph<NodeId, EdgeId, (), ()>;

fn finish(graph: Graph, root: NodeId) -> (Graph, NodeId) {
    (graph, root)
}
fn chain_with_spans(span: usize, spans: usize) -> (Graph, NodeId) {
    let mut graph = Graph::default();
    let nodes: Vec<_> = (0..=span).map(|_| graph.make_node(())).collect();
    for pair in nodes.windows(2) {
        graph.make_edge(pair[0], pair[1], ());
    }
    for i in 0..spans {
        graph.make_edge(
            nodes[i % (span / 2).max(1)],
            nodes[span - i % (span / 2).max(1)],
            (),
        );
    }
    finish(graph, nodes[0])
}
fn bipartite(width: usize, dense: bool) -> (Graph, NodeId) {
    let mut graph = Graph::default();
    let top: Vec<_> = (0..width).map(|_| graph.make_node(())).collect();
    let bottom: Vec<_> = (0..width).map(|_| graph.make_node(())).collect();
    for (i, &a) in top.iter().enumerate() {
        for (j, &b) in bottom.iter().enumerate() {
            if dense || j == width - 1 - i {
                graph.make_edge(a, b, ());
            }
        }
    }
    finish(graph, top[0])
}
fn bench_graph(c: &mut Criterion, name: &str, graph: &(Graph, NodeId), style: EdgeStyle) {
    c.bench_function(name, |b| {
        b.iter(|| {
            black_box(
                LayoutBuilder::new(&graph.0)
                    .root(graph.1)
                    .edge_style(style)
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
fn suites(c: &mut Criterion) {
    let mut group = c.benchmark_group("triskel_layout");
    for span in [8usize, 32, 64] {
        let graph = chain_with_spans(span, 1);
        group.bench_with_input(BenchmarkId::new("rank_span", span), &graph, |b, g| {
            b.iter(|| {
                black_box(
                    LayoutBuilder::new(&g.0)
                        .root(g.1)
                        .max_sweeps(4)
                        .build()
                        .unwrap(),
                )
            })
        });
    }
    group.finish();
    for (name, graph, style) in [
        (
            "wide_bipartite",
            bipartite(12, false),
            EdgeStyle::Orthogonal,
        ),
        ("dense_crossing", bipartite(10, true), EdgeStyle::Orthogonal),
        (
            "simultaneous_long_segments",
            chain_with_spans(24, 10),
            EdgeStyle::Orthogonal,
        ),
        (
            "nested_long_segments",
            chain_with_spans(32, 14),
            EdgeStyle::Straight,
        ),
        (
            "transpose_contention",
            bipartite(16, true),
            EdgeStyle::Straight,
        ),
        (
            "orthogonal_lane_contention",
            bipartite(14, true),
            EdgeStyle::Orthogonal,
        ),
    ] {
        bench_graph(c, name, &graph, style);
    }
}
criterion_group! { name = triskel; config = Criterion::default().sample_size(20).measurement_time(Duration::from_secs(3)); targets = suites }
criterion_main!(triskel);
