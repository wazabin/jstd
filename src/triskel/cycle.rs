//! Cycle removal via back-edge gadgets.
//!
//! Ranking and ordering require a DAG. Rather than reverse each back-edge (which
//! forces the router to draw it upside-down), we replace it with an *acyclic
//! gadget* that the normal pipeline lays out so the edge leaves its source's
//! bottom and enters its target's top, wrapping cleanly around both.
//!
//! For a back-edge `B -> A` (A is an ancestor, so it ranks above B) we add a
//! virtual node `A'` just above A and `B'` just below B, drop the back-edge, and
//! add three forward edges: `A' -> A`, `B -> B'`, and the long `A' -> B'`. After
//! layout the column `A' -> B'` is a straight vertical run; the router
//! reassembles the polyline `B (bottom) -> B' -> up the column -> A' -> A (top)`.
//! Because the column and the two short attach edges are ordinary edges, the
//! existing ranking/segment/Brandes–Köpf machinery aligns them — no special
//! spacing needed, and any number of back-edges compose.
//!
//! Every gadget edge carries the original edge's `orig` id and a `reversed`
//! marker so the router can find and reassemble its pieces.

use crate::{
    graph::{Graph, edge::Edge, node::Node},
    triskel::layout::{EdgeLayoutData, LayoutGraph, NodeLayoutData},
};

/// Replaces every back-edge with its gadget so `graph` becomes acyclic. The DFS
/// starts at `root` so edge orientation is relative to the designated entry: any
/// edge climbing back to the entry (or to a node above it on the root's spanning
/// tree) is the back-edge, keeping the entry a source — and therefore the top
/// rank. Remaining nodes (those not forward-reachable from the root) are then
/// visited in id order so cycles in any directed region are still broken.
/// Deterministic: the root first, then nodes and out-edges in id order.
pub(crate) fn break_cycles(graph: &mut LayoutGraph, root: usize) {
    let mut ids: Vec<usize> = graph.nodes().map(|n| n.id()).collect();
    ids.sort_unstable();

    // 0 = white (unseen), 1 = gray (on stack), 2 = black (done).
    let mut color = vec![0u8; graph_capacity(graph)];
    let mut back_edges: Vec<usize> = Vec::new();

    // Root first: it must be the spanning-tree root so its predecessors become
    // back-edges rather than the root being ranked as their descendant.
    if color.get(root) == Some(&0) {
        visit(graph, root, &mut color, &mut back_edges);
    }
    for start in ids {
        if color[start] == 0 {
            visit(graph, start, &mut color, &mut back_edges);
        }
    }

    for edge_id in back_edges {
        let edge = graph.get_edge(edge_id).unwrap();
        let source = edge.from_id(); // B: the descendant (lower) endpoint
        let target = edge.to_id(); // A: the ancestor (upper) endpoint
        // Preserve fixed proxy attachments on the real ends of the gadget:
        // `B -> B'` consumes `port_start`, and `A' -> A` consumes `port_end`.
        // The virtual column ignores either value.
        let gadget = EdgeLayoutData {
            reversed: true,
            orig: edge.orig,
            port_start: edge.port_start,
            port_end: edge.port_end,
            ..Default::default()
        };

        let a_prime = graph.make_node(virtual_node()); // above the target
        let b_prime = graph.make_node(virtual_node()); // below the source
        graph.remove_edge(edge_id);
        graph.make_edge(a_prime, target, gadget); // A' -> A  (into A's top)
        graph.make_edge(source, b_prime, gadget); // B -> B'  (out of B's bottom)
        graph.make_edge(a_prime, b_prime, gadget); // A' -> B' (the wrap column)
    }
}

/// A zero-size, non-rendered node used only to anchor a back-edge gadget.
fn virtual_node() -> NodeLayoutData {
    NodeLayoutData {
        width: 0.0,
        height: 0.0,
        is_dummy: true,
        ..Default::default()
    }
}

fn visit(graph: &LayoutGraph, node: usize, color: &mut [u8], back_edges: &mut Vec<usize>) {
    color[node] = 1;

    let mut children: Vec<(usize, usize)> = graph
        .get_node(node)
        .unwrap()
        .children()
        .map(|c| (c.edge_id(), c.node_id()))
        .collect();
    children.sort_unstable();

    for (edge_id, child) in children {
        if child == node {
            continue; // self-loop: dropped earlier, ignore defensively
        }
        match color[child] {
            0 => visit(graph, child, color, back_edges),
            1 => back_edges.push(edge_id), // edge to an ancestor on the stack
            _ => {}
        }
    }

    color[node] = 2;
}

/// Upper bound on node ids so a plain `Vec` can index the color map. Internal
/// ids are dense from 0, but removed nodes can leave gaps; size to max id + 1.
fn graph_capacity(graph: &LayoutGraph) -> usize {
    graph.nodes().map(|n| n.id()).max().map_or(0, |m| m + 1)
}
