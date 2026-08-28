//! Canonical single-entry/single-exit regions and their program-structure tree.
//!
//! Regions are edge based: an entry edge dominates a region's exit edge, the
//! exit post-dominates the entry, and both edges are cycle equivalent.  A
//! synthetic exit (connected back to the root) makes the reachable CFG a
//! flowgraph with one terminal.  Synthetic edges are analysis-only and never
//! appear in the returned tree.
//!
//! Edge cycle-equivalence classes are computed by the
//! Johnson--Pearson--Pingali undirected-DFS bracket-list algorithm. Canonical
//! boundaries are then selected from each class using edge dominance and
//! post-dominance. Test builds retain the direct cycle predicate as an oracle.

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

/// A valid edge-SESE candidate before canonical boundary selection.
///
/// This is intentionally crate-private: public [`compute_sese`] retains its
/// canonical smallest-boundary semantics, while layout can retain useful
/// non-canonical regions below a node hammock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeseCandidate<NodeId, EdgeId> {
    pub entry_edge: EdgeId,
    pub exit_edge: EdgeId,
    pub contained_nodes: Vec<NodeId>,
}

/// Enumerates every non-empty valid edge-SESE candidate.  This is the same
/// dominance, post-dominance, JPP-cycle-class, and `region_nodes` test used by
/// [`compute_sese`], before its canonical smallest-boundary filter.
pub(crate) fn compute_sese_candidates<G>(
    graph: &G,
    root: G::NodeId,
) -> Vec<SeseCandidate<G::NodeId, G::EdgeId>>
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
    let original_count = edges.len();
    let mut has_out = vec![false; ids.len()];
    for edge in &edges {
        has_out[edge.from] = true;
    }
    let mut added_exit = false;
    for (node, outgoing) in has_out.into_iter().enumerate() {
        if !outgoing {
            edges.push(IEdge {
                from: node,
                to: synthetic_exit,
                original: None,
            });
            added_exit = true;
        }
    }
    if !added_exit && !ids.is_empty() {
        edges.push(IEdge {
            from: ids.len() - 1,
            to: synthetic_exit,
            original: None,
        });
        added_exit = true;
    }
    if added_exit {
        edges.push(IEdge {
            from: synthetic_exit,
            to: index[&root],
            original: None,
        });
    }

    let n = ids.len() + 1;
    let entry = index[&root];
    let cycle_classes = jpp_cycle_classes(&edges, n, entry);
    let mut candidates = Vec::new();
    for a in 0..original_count {
        for b in 0..original_count {
            if a == b || !edge_dominates(&edges, n, entry, a, b) {
                continue;
            }
            if !edge_postdominates(&edges, n, synthetic_exit, b, a)
                || cycle_classes[a] == 0
                || cycle_classes[a] != cycle_classes[b]
            {
                continue;
            }
            let contained = region_nodes(&edges, n, a, b);
            if !contained.is_empty() {
                candidates.push(SeseCandidate {
                    entry_edge: edges[a].original.unwrap(),
                    exit_edge: edges[b].original.unwrap(),
                    contained_nodes: contained.into_iter().map(|node| ids[node]).collect(),
                });
            }
        }
    }
    candidates.sort_by_key(|candidate| {
        (
            std::cmp::Reverse(candidate.contained_nodes.len()),
            candidate.entry_edge,
            candidate.exit_edge,
        )
    });
    candidates
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
    let mut added_exit = false;
    for (node, outgoing) in has_out.into_iter().enumerate() {
        if !outgoing {
            edges.push(IEdge {
                from: node,
                to: synthetic_exit,
                original: None,
            });
            added_exit = true;
        }
    }
    // A closed CFG can have no natural exit (for example, an infinite loop).
    // Attach its deterministic last reachable node so the undirected JPP
    // flowgraph still has a bracket for every non-root tree edge.
    if !added_exit && !ids.is_empty() {
        edges.push(IEdge {
            from: ids.len() - 1,
            to: synthetic_exit,
            original: None,
        });
        added_exit = true;
    }
    if added_exit {
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
    let cycle_classes = jpp_cycle_classes(&edges, n, entry);
    #[cfg(test)]
    for a in 0..original_count {
        for b in 0..original_count {
            if a != b {
                assert_eq!(
                    cycle_classes[a] == cycle_classes[b],
                    cycle_equivalent(&edges, n, a, b),
                    "cycle-class mismatch for internal edges {a} and {b}"
                );
            }
        }
    }
    let mut candidates = Vec::<(usize, usize, BTreeSet<usize>)>::new();
    for a in 0..original_count {
        for b in 0..original_count {
            if a == b || !edge_dominates(&edges, n, entry, a, b) {
                continue;
            }
            if !edge_postdominates(&edges, n, synthetic_exit, b, a) {
                continue;
            }
            if cycle_classes[a] == 0 || cycle_classes[a] != cycle_classes[b] {
                continue;
            }
            #[cfg(test)]
            debug_assert!(cycle_equivalent(&edges, n, a, b));
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

/// Johnson--Pearson--Pingali cycle-equivalence classification. Capping edges
/// are internal bracket sentinels and are discarded with the working graph.
fn jpp_cycle_classes<E: Copy>(input: &[IEdge<E>], n: usize, root: usize) -> Vec<usize> {
    let mut edges = input.to_vec();
    let mut incident = vec![Vec::<usize>::new(); n];
    for (id, edge) in edges.iter().enumerate() {
        incident[edge.from].push(id);
        incident[edge.to].push(id);
    }

    let mut parent = vec![None; n];
    let mut parent_edge = vec![None; n];
    let mut number = vec![usize::MAX; n];
    let mut end = vec![0; n];
    let mut preorder = Vec::new();
    let mut tree = vec![false; edges.len()];
    // Keeping the DFS arrays explicit makes the JPP state correspondence
    // visible; wrapping them solely to reduce the parameter count obscures it.
    #[allow(clippy::too_many_arguments)]
    fn visit<E: Copy>(
        node: usize,
        edges: &[IEdge<E>],
        incident: &[Vec<usize>],
        parent: &mut [Option<usize>],
        parent_edge: &mut [Option<usize>],
        number: &mut [usize],
        end: &mut [usize],
        preorder: &mut Vec<usize>,
        tree: &mut [bool],
    ) {
        number[node] = preorder.len();
        preorder.push(node);
        for &eid in &incident[node] {
            if parent_edge[node] == Some(eid) {
                continue;
            }
            let edge = edges[eid];
            let other = if edge.from == node {
                edge.to
            } else {
                edge.from
            };
            if number[other] == usize::MAX {
                parent[other] = Some(node);
                parent_edge[other] = Some(eid);
                visit(
                    other,
                    edges,
                    incident,
                    parent,
                    parent_edge,
                    number,
                    end,
                    preorder,
                    tree,
                );
                tree[eid] = true;
            }
        }
        end[node] = preorder.len();
    }
    visit(
        root,
        &edges,
        &incident,
        &mut parent,
        &mut parent_edge,
        &mut number,
        &mut end,
        &mut preorder,
        &mut tree,
    );
    let is_descendant = |node: usize, ancestor: usize| {
        node != ancestor && number[ancestor] <= number[node] && number[node] < end[ancestor]
    };

    let mut hi = vec![usize::MAX; n];
    let mut brackets = vec![Vec::<usize>::new(); n];
    let mut classes = vec![0usize; edges.len()];
    let mut recent_size = vec![0usize; edges.len()];
    let mut recent_class = vec![0usize; edges.len()];
    let mut capping = Vec::<usize>::new();
    let mut next_class = 1usize;

    for &node in preorder.iter().rev() {
        let children: Vec<_> = preorder
            .iter()
            .copied()
            .filter(|child| parent[*child] == Some(node))
            .collect();
        let hi0 = incident[node]
            .iter()
            .copied()
            .filter(|eid| !tree[*eid])
            .filter_map(|eid| {
                let edge = edges[eid];
                let other = if edge.from == node {
                    edge.to
                } else {
                    edge.from
                };
                is_descendant(node, other).then_some(number[other])
            })
            .min()
            .unwrap_or(usize::MAX);
        let hi1 = children
            .iter()
            .map(|child| hi[*child])
            .min()
            .unwrap_or(usize::MAX);
        hi[node] = hi0.min(hi1);
        let mut skipped_hi_child = false;
        let hi2 = children
            .iter()
            .filter_map(|child| {
                if !skipped_hi_child && hi[*child] == hi1 {
                    skipped_hi_child = true;
                    None
                } else {
                    Some(hi[*child])
                }
            })
            .min()
            .unwrap_or(usize::MAX);

        for child in children {
            let child_brackets = std::mem::take(&mut brackets[child]);
            brackets[node].extend(child_brackets);
        }
        for &eid in &capping {
            let edge = edges[eid];
            // This mirrors Edge::other in the reference algorithm: for a node
            // not incident to the cap, its target is considered the child.
            let child = if edge.to == node { edge.from } else { edge.to };
            if is_descendant(child, node) {
                brackets[node].retain(|candidate| *candidate != eid);
            }
        }
        for &eid in &incident[node] {
            if tree[eid] {
                continue;
            }
            let edge = edges[eid];
            let other = if edge.from == node {
                edge.to
            } else {
                edge.from
            };
            if is_descendant(other, node) {
                brackets[node].retain(|candidate| *candidate != eid);
                if classes[eid] == 0 {
                    classes[eid] = next_class;
                    next_class += 1;
                }
            }
        }
        for &eid in &incident[node] {
            if tree[eid] {
                continue;
            }
            let edge = edges[eid];
            let other = if edge.from == node {
                edge.to
            } else {
                edge.from
            };
            if is_descendant(node, other) {
                brackets[node].push(eid);
            }
        }
        if hi2 < hi0 {
            let eid = edges.len();
            let ancestor = preorder[hi2];
            edges.push(IEdge {
                from: node,
                to: ancestor,
                original: None,
            });
            incident[node].push(eid);
            incident[ancestor].push(eid);
            tree.push(false);
            classes.push(0);
            recent_size.push(0);
            recent_class.push(0);
            capping.push(eid);
            brackets[node].push(eid);
        }
        if let Some(parent_eid) = parent_edge[node] {
            let Some(&top) = brackets[node].last() else {
                // Not a proper flowgraph (typically a non-terminating sink
                // SCC). The defining predicate remains exact for this case.
                return direct_cycle_classes(input, n);
            };
            if recent_size[top] != brackets[node].len() {
                recent_size[top] = brackets[node].len();
                recent_class[top] = next_class;
                next_class += 1;
            }
            classes[parent_eid] = recent_class[top];
            if recent_size[top] == 1 {
                classes[top] = classes[parent_eid];
            }
        }
    }
    classes.truncate(input.len());

    // The bracket algorithm assumes a proper flowgraph. Optimized CFGs can
    // violate that assumption through non-terminating SCCs and irreducible
    // control flow. Validate its partition and use the defining cycle
    // predicate as a correctness fallback for those components.
    let valid = (0..input.len()).all(|a| {
        (0..input.len()).all(|b| (classes[a] == classes[b]) == cycle_equivalent(input, n, a, b))
    });
    if valid {
        return classes;
    }

    direct_cycle_classes(input, n)
}

fn direct_cycle_classes<E: Copy>(edges: &[IEdge<E>], n: usize) -> Vec<usize> {
    let mut parent: Vec<_> = (0..edges.len()).collect();
    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    for a in 0..edges.len() {
        for b in (a + 1)..edges.len() {
            if cycle_equivalent(edges, n, a, b) {
                let ra = find(&mut parent, a);
                let rb = find(&mut parent, b);
                parent[rb] = ra;
            }
        }
    }
    let mut labels = HashMap::default();
    let mut next = 1usize;
    (0..edges.len())
        .map(|edge| {
            let root = find(&mut parent, edge);
            *labels.entry(root).or_insert_with(|| {
                let label = next;
                next += 1;
                label
            })
        })
        .collect()
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
