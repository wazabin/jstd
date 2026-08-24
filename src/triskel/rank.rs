//! Rank (layer) assignment via the Gansner et al. (TSE93) network simplex.
//!
//! Operates on the internal acyclic [`LayoutGraph`] (back-edges already
//! reversed, self-loops already dropped) and writes a non-negative `rank` into
//! every node. Each edge contributes `weight · slack` to the total cost, where
//! `slack(e) = rank(head) − rank(tail) − minlen(e)`; network simplex minimises
//! the total cost, yielding compact, balanced layerings (unlike plain
//! longest-path, which is merely the feasible starting point).

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::collections::VecDeque;

use crate::{
    graph::{Graph, GraphMut, edge::Edge, node::Node},
    triskel::layout::LayoutGraph,
};

#[derive(Clone, Copy)]
struct RankEdge {
    tail: usize,
    head: usize,
    minlen: i64,
    weight: i64,
}

impl RankEdge {
    fn slack(&self, rank: &[i64]) -> i64 {
        rank[self.head] - rank[self.tail] - self.minlen
    }
}

/// Assigns `rank` to every node of `graph` (min rank normalised to 0).
pub(crate) fn assign_ranks(graph: &mut LayoutGraph) {
    let n = graph.nodes().count();
    if n == 0 {
        return;
    }

    // Internal node ids are a contiguous 0..next_node_id range, but removed
    // nodes can leave gaps; remap to a dense 0..n index space.
    let mut ids: Vec<usize> = graph.nodes().map(|node| node.id()).collect();
    ids.sort_unstable();
    let index: HashMap<usize, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    let mut edges: Vec<RankEdge> = Vec::new();
    let mut edge_refs: Vec<usize> = graph.edges().map(|edge| edge.id()).collect();
    edge_refs.sort_unstable();
    for edge_id in edge_refs {
        let edge = graph.get_edge(edge_id).unwrap();
        let tail = index[&edge.from_id()];
        let head = index[&edge.to_id()];
        if tail == head {
            continue; // defensive: self-loops should already be gone
        }
        edges.push(RankEdge {
            tail,
            head,
            minlen: edge.minlen.max(0),
            weight: edge.weight.max(1),
        });
    }

    let ranks = network_simplex(ids.len(), &edges);

    for (i, id) in ids.iter().enumerate() {
        graph.get_node_mut(*id).unwrap().rank = ranks[i];
    }
}

fn network_simplex(n: usize, edges: &[RankEdge]) -> Vec<i64> {
    let mut rank = init_rank(n, edges);
    if edges.is_empty() {
        return rank; // all zeros: isolated nodes
    }

    let mut tree = feasible_tree(n, edges, &mut rank);

    // Each exchange replaces one negative-cut tree edge with a min-slack edge
    // crossing the same cut. Bounded to guarantee termination on pathological
    // input; real layered graphs converge in far fewer steps.
    let cap = (n * edges.len()).max(64) + 64;
    for _ in 0..cap {
        let cut = cut_values(n, edges, &tree);
        let Some(leave) = tree.iter().copied().filter(|e| cut[e] < 0).min() else {
            break;
        };
        let Some(enter) = enter_edge(n, edges, &tree, leave, &rank) else {
            break;
        };
        tree.remove(&leave);
        tree.insert(enter);
        tighten(n, edges, &tree, &mut rank);
    }

    normalize(&mut rank);
    balance(n, edges, &mut rank);
    normalize(&mut rank);
    rank
}

/// Longest-path feasible ranking: every node sits as early as its predecessors
/// allow. Always feasible because the graph is acyclic.
fn init_rank(n: usize, edges: &[RankEdge]) -> Vec<i64> {
    let mut indeg = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        indeg[e.head] += 1;
        out[e.tail].push(i);
    }

    let mut rank = vec![0i64; n];
    let mut queue: VecDeque<usize> = (0..n).filter(|v| indeg[*v] == 0).collect();
    while let Some(v) = queue.pop_front() {
        for &ei in &out[v] {
            let e = edges[ei];
            if rank[e.head] < rank[e.tail] + e.minlen {
                rank[e.head] = rank[e.tail] + e.minlen;
            }
            indeg[e.head] -= 1;
            if indeg[e.head] == 0 {
                queue.push_back(e.head);
            }
        }
    }
    rank
}

/// Grows a spanning tree of tight edges (slack 0). Where it cannot span the
/// graph, shifts the current tree component to tighten the min-slack boundary
/// edge and retries. Returns the spanning tree's edge-index set.
fn feasible_tree(n: usize, edges: &[RankEdge], rank: &mut [i64]) -> HashSet<usize> {
    loop {
        let (nodes, tree) = tight_tree(n, edges, rank);
        if nodes.len() == n {
            return tree;
        }

        // Min-slack edge with exactly one endpoint inside the current tree.
        let mut best: Option<(i64, usize)> = None;
        for (i, e) in edges.iter().enumerate() {
            let tin = nodes.contains(&e.tail);
            let hin = nodes.contains(&e.head);
            if tin == hin {
                continue;
            }
            let slack = e.slack(rank);
            if best.is_none_or(|(s, bi)| slack < s || (slack == s && i < bi)) {
                best = Some((slack, i));
            }
        }

        let (slack, ei) = best.expect("connected graph must have a boundary edge");
        let e = edges[ei];
        let delta = if nodes.contains(&e.head) {
            -slack
        } else {
            slack
        };
        for v in &nodes {
            rank[*v] += delta;
        }
    }
}

/// DFS from node 0 following only tight edges (in either direction); returns the
/// reached node set and the tree edges used to reach them.
fn tight_tree(n: usize, edges: &[RankEdge], rank: &[i64]) -> (HashSet<usize>, HashSet<usize>) {
    let inc = incidence(n, edges);
    let mut nodes = HashSet::default();
    let mut tree = HashSet::default();
    let mut stack = vec![0usize];
    nodes.insert(0usize);
    while let Some(v) = stack.pop() {
        for &ei in &inc[v] {
            let e = edges[ei];
            if e.slack(rank) != 0 {
                continue;
            }
            let other = if e.tail == v { e.head } else { e.tail };
            if nodes.insert(other) {
                tree.insert(ei);
                stack.push(other);
            }
        }
    }
    (nodes, tree)
}

/// For each tree edge, the cut value: (weight of edges crossing the cut in the
/// tail→head direction) − (weight crossing head→tail).
fn cut_values(n: usize, edges: &[RankEdge], tree: &HashSet<usize>) -> HashMap<usize, i64> {
    let mut out = HashMap::default();
    for &te in tree {
        let head_comp = component(n, edges, tree, te, /*from_head=*/ true);
        let mut value = 0i64;
        for f in edges {
            let tail_in_head = head_comp.contains(&f.tail);
            let head_in_head = head_comp.contains(&f.head);
            if tail_in_head == head_in_head {
                continue; // does not cross the cut
            }
            // Cut is oriented along the tree edge e (tail-comp → head-comp).
            if !tail_in_head && head_in_head {
                value += f.weight; // same direction as e
            } else {
                value -= f.weight; // opposite
            }
        }
        out.insert(te, value);
    }
    out
}

/// Nodes reachable from one endpoint of tree edge `skip` within the tree, not
/// crossing `skip`. With `from_head`, starts at the head endpoint.
fn component(
    n: usize,
    edges: &[RankEdge],
    tree: &HashSet<usize>,
    skip: usize,
    from_head: bool,
) -> HashSet<usize> {
    let inc = tree_incidence(n, edges, tree);
    let start = if from_head {
        edges[skip].head
    } else {
        edges[skip].tail
    };
    let mut seen = HashSet::default();
    seen.insert(start);
    let mut stack = vec![start];
    while let Some(v) = stack.pop() {
        for &ei in &inc[v] {
            if ei == skip {
                continue;
            }
            let e = edges[ei];
            let other = if e.tail == v { e.head } else { e.tail };
            if seen.insert(other) {
                stack.push(other);
            }
        }
    }
    seen
}

/// The non-tree edge that should enter when `leave` exits: it must cross the cut
/// in the opposite direction (head-component → tail-component) with min slack.
fn enter_edge(
    n: usize,
    edges: &[RankEdge],
    tree: &HashSet<usize>,
    leave: usize,
    rank: &[i64],
) -> Option<usize> {
    let head_comp = component(n, edges, tree, leave, true);
    let mut best: Option<(i64, usize)> = None;
    for (i, f) in edges.iter().enumerate() {
        if tree.contains(&i) {
            continue;
        }
        // Opposite direction across the cut: tail in head-comp, head in tail-comp.
        if head_comp.contains(&f.tail) && !head_comp.contains(&f.head) {
            let slack = f.slack(rank);
            if best.is_none_or(|(s, bi)| slack < s || (slack == s && i < bi)) {
                best = Some((slack, i));
            }
        }
    }
    best.map(|(_, i)| i)
}

fn incidence(n: usize, edges: &[RankEdge]) -> Vec<Vec<usize>> {
    let mut inc = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        inc[e.tail].push(i);
        inc[e.head].push(i);
    }
    inc
}

fn tree_incidence(n: usize, edges: &[RankEdge], tree: &HashSet<usize>) -> Vec<Vec<usize>> {
    let mut inc = vec![Vec::new(); n];
    for &ei in tree {
        let e = edges[ei];
        inc[e.tail].push(ei);
        inc[e.head].push(ei);
    }
    inc
}

/// Recompute ranks so all tree edges are tight (slack 0), anchored at node 0.
fn tighten(n: usize, edges: &[RankEdge], tree: &HashSet<usize>, rank: &mut [i64]) {
    let inc = tree_incidence(n, edges, tree);
    let mut seen = vec![false; n];
    let mut stack = vec![0usize];
    seen[0] = true;
    while let Some(v) = stack.pop() {
        for &ei in &inc[v] {
            let e = edges[ei];
            let other = if e.tail == v { e.head } else { e.tail };
            if seen[other] {
                continue;
            }
            seen[other] = true;
            rank[other] = if e.tail == v {
                rank[v] + e.minlen // v is tail, other is head
            } else {
                rank[v] - e.minlen // v is head, other is tail
            };
            stack.push(other);
        }
    }
}

fn normalize(rank: &mut [i64]) {
    if let Some(min) = rank.iter().copied().min() {
        for r in rank.iter_mut() {
            *r -= min;
        }
    }
}

/// Centre nodes whose in- and out-weight are equal within their feasible rank
/// window — a cheap version of dot's balancing that reduces lopsided columns.
fn balance(n: usize, edges: &[RankEdge], rank: &mut [i64]) {
    let mut in_w = vec![0i64; n];
    let mut out_w = vec![0i64; n];
    let mut ins: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut outs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        out_w[e.tail] += e.weight;
        in_w[e.head] += e.weight;
        outs[e.tail].push(i);
        ins[e.head].push(i);
    }

    for v in 0..n {
        if in_w[v] != out_w[v] {
            continue;
        }
        let lo = ins[v]
            .iter()
            .map(|&i| rank[edges[i].tail] + edges[i].minlen)
            .max();
        let hi = outs[v]
            .iter()
            .map(|&i| rank[edges[i].head] - edges[i].minlen)
            .min();
        if let (Some(lo), Some(hi)) = (lo, hi)
            && lo <= hi
        {
            rank[v] = (lo + hi) / 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graph::Graph,
        triskel::layout::{EdgeLayoutData, NodeLayoutData},
    };

    fn edge(tail: usize, head: usize, minlen: i64, weight: i64) -> RankEdge {
        RankEdge {
            tail,
            head,
            minlen,
            weight,
        }
    }

    #[test]
    fn network_simplex_returns_normalized_feasible_ranks() {
        let edges = [
            edge(0, 1, 2, 1),
            edge(0, 2, 1, 3),
            edge(1, 3, 1, 2),
            edge(2, 3, 2, 1),
        ];

        let ranks = network_simplex(4, &edges);
        assert_eq!(ranks.iter().min(), Some(&0));
        for edge in edges {
            assert!(edge.slack(&ranks) >= 0, "edge has negative slack");
        }
    }

    #[test]
    fn assign_ranks_handles_empty_isolated_and_weighted_graphs() {
        let mut graph = LayoutGraph::default();
        assign_ranks(&mut graph);

        let isolated = graph.make_node(NodeLayoutData::default());
        assign_ranks(&mut graph);
        assert_eq!(graph.get_node(isolated).unwrap().rank, 0);

        let mut graph = LayoutGraph::default();
        let source = graph.make_node(NodeLayoutData::default());
        let target = graph.make_node(NodeLayoutData::default());
        graph.make_edge(
            source,
            target,
            EdgeLayoutData {
                minlen: 3,
                weight: 2,
                ..Default::default()
            },
        );
        assign_ranks(&mut graph);

        let source_rank = graph.get_node(source).unwrap().rank;
        let target_rank = graph.get_node(target).unwrap().rank;
        assert!(source_rank >= 0);
        assert!(target_rank - source_rank >= 3);
    }

    #[test]
    fn component_and_cut_values_respect_a_removed_tree_edge() {
        let edges = [edge(0, 1, 1, 1), edge(1, 2, 1, 1), edge(0, 2, 1, 4)];
        let tree: HashSet<usize> = [0, 1].into_iter().collect();
        let head_component: HashSet<usize> = [2].into_iter().collect();
        let tail_component: HashSet<usize> = [0, 1].into_iter().collect();

        assert_eq!(component(3, &edges, &tree, 1, true), head_component);
        assert_eq!(component(3, &edges, &tree, 1, false), tail_component);
        assert_eq!(cut_values(3, &edges, &tree).get(&1), Some(&5));
    }
}
