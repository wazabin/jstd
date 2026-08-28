//! Canonical single-entry/single-exit regions and their program-structure tree.
//!
//! Regions are edge based: an entry edge dominates a region's exit edge, the
//! exit post-dominates the entry, and both edges are cycle equivalent.  A
//! synthetic exit (connected back to the root) makes the reachable CFG a
//! flowgraph with one terminal.  Synthetic edges are analysis-only and never
//! appear in the returned tree.
//!
//! This implementation deliberately favours a small, auditable formulation of
//! the Johnson--Pearson--Pingali definitions.  It computes edge dominance,
//! post-dominance and cycle equivalence directly, then extracts the canonical
//! (laminar) regions.  The asymptotically faster bracket-list algorithm can be
//! substituted without changing the public result type.

use std::collections::{BTreeSet, VecDeque};

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::graph::{Graph, edge::Edge, node::Node};

/// One canonical edge-based SESE region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeseRegion<NodeId, EdgeId> {
    /// Parent region; `None` only for the synthetic root region.
    pub parent: Option<usize>,
    /// Immediate child regions, in deterministic entry-edge order.
    pub children: Vec<usize>,
    /// Boundary edge entering this region (`None` for the root region).
    pub entry_edge: Option<EdgeId>,
    /// Boundary edge leaving this region (`None` for the root region).
    pub exit_edge: Option<EdgeId>,
    /// Nodes owned directly by this region (children are not repeated here).
    pub nodes: Vec<NodeId>,
    /// All original nodes contained by the region, including descendants.
    pub contained_nodes: Vec<NodeId>,
}

/// Program-structure tree for the nodes reachable from a CFG entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeseTree<NodeId, EdgeId> {
    pub regions: Vec<SeseRegion<NodeId, EdgeId>>,
}

impl<NodeId, EdgeId> SeseTree<NodeId, EdgeId> {
    pub fn root(&self) -> &SeseRegion<NodeId, EdgeId> {
        &self.regions[0]
    }

    pub fn has_nontrivial_regions(&self) -> bool {
        self.regions.len() > 1
    }
}

#[derive(Clone, Copy)]
struct IEdge<E> {
    from: usize,
    to: usize,
    original: Option<E>,
}

/// Identifies canonical SESE regions in the directed subgraph reachable from
/// `root`. Multiple exits are joined to an internal synthetic exit. Self-loops
/// do not form region boundaries.
pub fn compute_sese<G>(graph: &G, root: G::NodeId) -> SeseTree<G::NodeId, G::EdgeId>
where
    G: Graph,
    G::NodeId: Ord,
    G::EdgeId: Ord,
{
    let mut ids = Vec::new();
    let mut seen: HashSet<G::NodeId> = HashSet::default();
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        ids.push(id);
        let mut succ: Vec<_> = graph
            .get_node(id)
            .into_iter()
            .flat_map(|node| node.children().map(|edge| edge.node_id()))
            .collect();
        succ.sort();
        queue.extend(succ);
    }
    ids.sort();

    let index: HashMap<_, _> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let synthetic_exit = ids.len();
    let mut edges = Vec::new();
    let mut original_edges: Vec<_> = graph
        .edges()
        .filter_map(|edge| {
            let from = edge.from_id();
            let to = edge.to_id();
            (from != to && index.contains_key(&from) && index.contains_key(&to)).then_some((
                edge.id(),
                from,
                to,
            ))
        })
        .collect();
    original_edges.sort_by_key(|(id, _, _)| *id);
    for (id, from, to) in original_edges {
        edges.push(IEdge {
            from: index[&from],
            to: index[&to],
            original: Some(id),
        });
    }

    let mut has_out = vec![false; ids.len()];
    for edge in &edges {
        has_out[edge.from] = true;
    }
    for (node, outgoing) in has_out.into_iter().enumerate() {
        if !outgoing {
            edges.push(IEdge {
                from: node,
                to: synthetic_exit,
                original: None,
            });
        }
    }
    // A component with no natural exit is already closed; the back edge below
    // is unnecessary, and there can be no terminal-based canonical region.
    if edges.iter().any(|edge| edge.to == synthetic_exit) {
        edges.push(IEdge {
            from: synthetic_exit,
            to: index[&root],
            original: None,
        });
    }

    let n = ids.len() + 1;
    let entry = index[&root];
    let original_count = edges
        .iter()
        .take_while(|edge| edge.original.is_some())
        .count();
    let mut candidates = Vec::<(usize, usize, BTreeSet<usize>)>::new();
    for a in 0..original_count {
        for b in 0..original_count {
            if a == b || !edge_dominates(&edges, n, entry, a, b) {
                continue;
            }
            if !edge_postdominates(&edges, n, synthetic_exit, b, a) {
                continue;
            }
            if !cycle_equivalent(&edges, n, a, b) {
                continue;
            }
            let contained = region_nodes(&edges, n, a, b);
            if !contained.is_empty() {
                candidates.push((a, b, contained));
            }
        }
    }

    // A canonical region is the smallest region for which an edge is an entry
    // or exit boundary. A boundary may therefore be the exit of one canonical
    // region and the entry of the next; consuming boundaries greedily would
    // incorrectly merge sequential structured constructs.
    candidates.sort_by_key(|(a, b, nodes)| (nodes.len(), *a, *b));
    let mut smallest_for_boundary: HashMap<usize, usize> = HashMap::default();
    for (index, (a, b, _)) in candidates.iter().enumerate() {
        smallest_for_boundary.entry(*a).or_insert(index);
        smallest_for_boundary.entry(*b).or_insert(index);
    }
    let mut selected: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter(|(index, (a, b, _))| {
            smallest_for_boundary.get(a) == Some(index)
                || smallest_for_boundary.get(b) == Some(index)
        })
        .map(|(_, candidate)| candidate.clone())
        .collect();
    // Defensive laminarity filter for malformed/irreducible flowgraphs. JPP
    // canonical regions are laminar; crossing candidates belong to the root.
    selected.retain(|(_, _, candidate)| {
        !candidates.iter().any(|(_, _, other)| {
            let intersects = candidate.iter().any(|node| other.contains(node));
            intersects && !candidate.is_subset(other) && !other.is_subset(candidate)
        })
    });
    selected.sort_by_key(|(a, b, nodes)| (std::cmp::Reverse(nodes.len()), *a, *b));

    let mut regions = vec![SeseRegion {
        parent: None,
        children: Vec::new(),
        entry_edge: None,
        exit_edge: None,
        nodes: Vec::new(),
        contained_nodes: ids.clone(),
    }];
    let mut sets = vec![(0..ids.len()).collect::<BTreeSet<_>>()];
    for (a, b, nodes) in selected {
        let parent = sets
            .iter()
            .enumerate()
            .filter(|(_, set)| nodes.is_subset(set))
            .min_by_key(|(_, set)| set.len())
            .map(|(i, _)| i)
            .unwrap_or(0);
        let id = regions.len();
        regions.push(SeseRegion {
            parent: Some(parent),
            children: Vec::new(),
            entry_edge: edges[a].original,
            exit_edge: edges[b].original,
            nodes: Vec::new(),
            contained_nodes: nodes.iter().map(|i| ids[*i]).collect(),
        });
        sets.push(nodes);
        regions[parent].children.push(id);
    }
    for region in &mut regions {
        // Region ids are assigned in deterministic boundary-edge order.
        region.children.sort_unstable();
    }
    // Assign each node to its deepest containing region.
    for (node_index, node) in ids.iter().copied().enumerate() {
        let owner = sets
            .iter()
            .enumerate()
            .filter(|(_, set)| set.contains(&node_index))
            .min_by_key(|(_, set)| set.len())
            .map(|(id, _)| id)
            .unwrap_or(0);
        regions[owner].nodes.push(node);
    }

    SeseTree { regions }
}

fn reachable<E: Copy>(
    edges: &[IEdge<E>],
    n: usize,
    start: usize,
    skip: Option<usize>,
    reverse: bool,
) -> Vec<bool> {
    let mut seen = vec![false; n];
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        if seen[node] {
            continue;
        }
        seen[node] = true;
        for (i, edge) in edges.iter().enumerate() {
            if skip == Some(i) {
                continue;
            }
            let (from, to) = if reverse {
                (edge.to, edge.from)
            } else {
                (edge.from, edge.to)
            };
            if from == node && !seen[to] {
                stack.push(to);
            }
        }
    }
    seen
}

fn edge_dominates<E: Copy>(edges: &[IEdge<E>], n: usize, root: usize, a: usize, b: usize) -> bool {
    !reachable(edges, n, root, Some(a), false)[edges[b].from]
}

fn edge_postdominates<E: Copy>(
    edges: &[IEdge<E>],
    n: usize,
    exit: usize,
    b: usize,
    a: usize,
) -> bool {
    !reachable(edges, n, exit, Some(b), true)[edges[a].to]
}

fn cycle_equivalent<E: Copy>(edges: &[IEdge<E>], n: usize, a: usize, b: usize) -> bool {
    let a_cycle_without_b = reachable(edges, n, edges[a].to, Some(b), false)[edges[a].from];
    let b_cycle_without_a = reachable(edges, n, edges[b].to, Some(a), false)[edges[b].from];
    !a_cycle_without_b && !b_cycle_without_a
}

fn region_nodes<E: Copy>(
    edges: &[IEdge<E>],
    n: usize,
    entry: usize,
    exit: usize,
) -> BTreeSet<usize> {
    let forward = reachable(edges, n, edges[entry].to, Some(exit), false);
    let backward = reachable(edges, n, edges[exit].from, Some(entry), true);
    (0..n.saturating_sub(1))
        .filter(|node| forward[*node] && backward[*node])
        .collect()
}

#[cfg(test)]
mod tests {
    use jstd_derive::Identifier;
    use proptest::prelude::*;

    use super::*;
    use crate::graph::owning::OwningGraph;

    #[derive(Identifier)]
    struct NodeId(usize);
    #[derive(Identifier)]
    struct EdgeId(usize);
    type G = OwningGraph<NodeId, EdgeId, (), ()>;

    #[test]
    fn finds_diamond_between_canonical_boundary_edges() {
        let mut graph = G::default();
        let s = graph.make_node(());
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        let t = graph.make_node(());
        let entry = graph.make_edge(s, a, ());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());
        let exit = graph.make_edge(d, t, ());

        let tree = compute_sese(&graph, s);
        let region = tree
            .regions
            .iter()
            .find(|region| region.entry_edge == Some(entry))
            .expect("diamond region");
        assert_eq!(region.exit_edge, Some(exit));
        assert_eq!(region.contained_nodes, vec![a, b, c, d]);
    }

    #[test]
    fn sequential_regions_may_share_a_boundary_edge() {
        let mut graph = G::default();
        let n: Vec<_> = (0..10).map(|_| graph.make_node(())).collect();
        let first_entry = graph.make_edge(n[0], n[1], ());
        graph.make_edge(n[1], n[2], ());
        graph.make_edge(n[1], n[3], ());
        graph.make_edge(n[2], n[4], ());
        graph.make_edge(n[3], n[4], ());
        let shared = graph.make_edge(n[4], n[5], ());
        graph.make_edge(n[5], n[6], ());
        graph.make_edge(n[5], n[7], ());
        graph.make_edge(n[6], n[8], ());
        graph.make_edge(n[7], n[8], ());
        graph.make_edge(n[8], n[9], ());

        let tree = compute_sese(&graph, n[0]);
        assert!(tree.regions.iter().any(|region| {
            region.entry_edge == Some(first_entry) && region.exit_edge == Some(shared)
        }));
        assert!(
            tree.regions
                .iter()
                .any(|region| region.entry_edge == Some(shared))
        );
    }

    #[test]
    fn multiple_exits_use_an_internal_terminal_without_exposing_it() {
        let mut graph = G::default();
        let root = graph.make_node(());
        let left = graph.make_node(());
        let right = graph.make_node(());
        graph.make_edge(root, left, ());
        graph.make_edge(root, right, ());

        let tree = compute_sese(&graph, root);
        let mut owned: Vec<_> = tree
            .regions
            .iter()
            .flat_map(|region| region.nodes.iter().copied())
            .collect();
        owned.sort();
        assert_eq!(owned, vec![root, left, right]);
        assert!(tree.regions.iter().all(|region| {
            region.entry_edge.is_some() == region.exit_edge.is_some() || region.parent.is_none()
        }));
    }

    #[test]
    fn loop_body_is_a_canonical_region() {
        let mut graph = G::default();
        let s = graph.make_node(());
        let header = graph.make_node(());
        let body = graph.make_node(());
        let after = graph.make_node(());
        let t = graph.make_node(());
        let entry = graph.make_edge(s, header, ());
        graph.make_edge(header, body, ());
        graph.make_edge(body, header, ());
        let exit = graph.make_edge(header, after, ());
        graph.make_edge(after, t, ());

        let tree = compute_sese(&graph, s);
        assert!(tree.regions.iter().any(|region| {
            region.entry_edge == Some(entry)
                && region.exit_edge == Some(exit)
                && region.contained_nodes.contains(&header)
                && region.contained_nodes.contains(&body)
        }));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn generated_program_structure_trees_are_laminar_and_total(
            node_count in 2usize..=10,
            extra_edges in proptest::collection::vec((0usize..16, 0usize..16), 0..24),
        ) {
            let mut graph = G::default();
            let nodes: Vec<_> = (0..node_count).map(|_| graph.make_node(())).collect();
            // A backbone makes every generated node reachable while the extra
            // edges supply branches, loops, and irreducible cross-links.
            for pair in nodes.windows(2) {
                graph.make_edge(pair[0], pair[1], ());
            }
            for (from, to) in extra_edges {
                graph.make_edge(nodes[from % node_count], nodes[to % node_count], ());
            }

            let tree = compute_sese(&graph, nodes[0]);
            let mut owned: Vec<_> = tree.regions.iter()
                .flat_map(|region| region.nodes.iter().copied())
                .collect();
            owned.sort();
            prop_assert_eq!(&owned, &nodes);
            for (id, region) in tree.regions.iter().enumerate() {
                for &child in &region.children {
                    prop_assert_eq!(tree.regions[child].parent, Some(id));
                    prop_assert!(tree.regions[child].contained_nodes.iter()
                        .all(|node| region.contained_nodes.contains(node)));
                }
            }
            for (i, lhs) in tree.regions.iter().enumerate().skip(1) {
                for rhs in tree.regions.iter().skip(i + 1) {
                    let intersects = lhs.contained_nodes.iter()
                        .any(|node| rhs.contained_nodes.contains(node));
                    prop_assert!(!intersects
                        || lhs.contained_nodes.iter().all(|node| rhs.contained_nodes.contains(node))
                        || rhs.contained_nodes.iter().all(|node| lhs.contained_nodes.contains(node)));
                }
            }
        }
    }

    #[test]
    fn nested_regions_are_laminar_and_assign_every_node_once() {
        let mut graph = G::default();
        let nodes: Vec<_> = (0..8).map(|_| graph.make_node(())).collect();
        for pair in nodes.windows(2) {
            graph.make_edge(pair[0], pair[1], ());
        }
        graph.make_edge(nodes[1], nodes[4], ());
        graph.make_edge(nodes[2], nodes[3], ());
        graph.make_edge(nodes[4], nodes[6], ());

        let tree = compute_sese(&graph, nodes[0]);
        let owned: Vec<_> = tree
            .regions
            .iter()
            .flat_map(|region| region.nodes.iter().copied())
            .collect();
        let mut unique = owned.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(owned.len(), nodes.len());
        assert_eq!(unique, nodes);
        for (id, region) in tree.regions.iter().enumerate().skip(1) {
            let parent = region.parent.unwrap();
            assert!(parent < id);
            assert!(
                region
                    .contained_nodes
                    .iter()
                    .all(|node| { tree.regions[parent].contained_nodes.contains(node) })
            );
        }
    }
}
