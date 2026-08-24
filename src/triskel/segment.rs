//! Eiglsperger long-edge representation.
//!
//! A long edge spanning ranks `r0..r1` is *not* split into one dummy per rank.
//! Instead it becomes at most two dummy vertices plus a single spanning edge:
//!
//! * span 2 (`r1 == r0+2`): one **r-vertex** at `r0+1`; the edge becomes
//!   `from → r → to`. (An ordinary dummy — there is no pass-through rank.)
//! * span ≥ 3: a **p-vertex** at `r0+1` and a **q-vertex** at `r1-1`; the
//!   original edge is re-pointed to the spanning **segment** `p → q`, with new
//!   edges `from → p` and `q → to`. Ranks strictly between `p` and `q` carry the
//!   segment only as a container element (no vertex) — that is the Eiglsperger
//!   memory win: O(1) graph objects per long edge regardless of its rank span.
//!
//! All edges of one original edge share its `orig` id and `reversed` flag so the
//! router can reassemble the polyline.

use rustc_hash::FxHashSet as HashSet;

use crate::{
    graph::{Graph, edge::Edge},
    triskel::layout::{EdgeLayoutData, LayoutGraph, NodeLayoutData, layer_of},
};

/// The p/q dummy classification needed by the container ordering pass.
pub(crate) struct SegmentInfo {
    pub pvertices: HashSet<usize>,
    pub qvertices: HashSet<usize>,
}

pub(crate) fn build_segments(graph: &mut LayoutGraph) -> SegmentInfo {
    let mut pvertices = HashSet::default();
    let mut qvertices = HashSet::default();

    let mut edge_ids: Vec<usize> = graph.edges().map(|e| e.id()).collect();
    edge_ids.sort_unstable();

    for edge_id in edge_ids {
        let edge = graph.get_edge(edge_id).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        let data = *edge.data();

        let r0 = layer_of(graph, from);
        let r1 = layer_of(graph, to);
        if r1 <= r0 + 1 {
            continue; // adjacent ranks
        }

        let span_data = EdgeLayoutData {
            reversed: data.reversed,
            orig: data.orig,
            ..Default::default()
        };

        if r1 == r0 + 2 {
            let r = graph.make_node(dummy_at(r0 + 1));
            graph.remove_edge(edge_id);
            graph.make_edge(from, r, span_data);
            graph.make_edge(r, to, span_data);
        } else {
            let p = graph.make_node(dummy_at(r0 + 1));
            let q = graph.make_node(dummy_at(r1 - 1));
            // Re-point the original edge to be the spanning p → q segment; it
            // keeps its orig/reversed data.
            graph.edit_edge(edge_id, p, q);
            graph.make_edge(from, p, span_data);
            graph.make_edge(q, to, span_data);
            pvertices.insert(p);
            qvertices.insert(q);
        }
    }

    SegmentInfo {
        pvertices,
        qvertices,
    }
}

fn dummy_at(rank: usize) -> NodeLayoutData {
    NodeLayoutData {
        width: 0.0,
        height: 0.0,
        rank: rank as i64,
        is_dummy: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triskel::rank;

    /// The Eiglsperger win: a long edge spanning many ranks costs O(1) dummies
    /// (a p- and a q-vertex), not one per intermediate rank.
    #[test]
    fn long_edge_uses_two_dummies_regardless_of_span() {
        let mut g = LayoutGraph::default();
        let n: Vec<usize> = (0..6)
            .map(|_| {
                g.make_node(NodeLayoutData {
                    width: 10.0,
                    height: 10.0,
                    ..Default::default()
                })
            })
            .collect();
        for w in n.windows(2) {
            g.make_edge(w[0], w[1], EdgeLayoutData::default());
        }
        // Edge spanning all 5 rank-steps (ranks 0..5).
        g.make_edge(n[0], n[5], EdgeLayoutData::default());

        rank::assign_ranks(&mut g);
        let info = build_segments(&mut g);

        let dummies = g.nodes().filter(|node| node.is_dummy).count();
        assert_eq!(dummies, 2, "span-5 long edge must use exactly 2 dummies");
        assert_eq!(info.pvertices.len(), 1);
        assert_eq!(info.qvertices.len(), 1);
    }
}
