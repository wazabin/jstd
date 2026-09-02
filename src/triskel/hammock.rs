//! Dominator-based detection of maximal Ferrante hammocks.
//!
//! A hammock is an induced node region with one entry node and one external
//! exit node.  Every outside edge entering the region targets the entry, and
//! every edge leaving it targets the exit.  For an entry `e`, dominance limits
//! every such region to either `D(e)` or `D(e) - D(x)`.  This implementation
//! summarizes all dominator-subtree boundary edges in one graph-wide pass and
//! emits at most one maximal candidate per entry; it performs no candidate
//! reachability searches.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    graph::{Graph, analysis::compute_dominators, edge::Edge, owning::OwningGraph},
    registry::Identifier,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hammock<NodeId, EdgeId> {
    pub nodes: Vec<NodeId>,
    pub entry_edge: EdgeId,
    pub exit_edge: EdgeId,
}

#[derive(Clone, Copy, Debug, Default)]
enum Target {
    #[default]
    None,
    One(usize),
    Many,
}

impl Target {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, value) | (value, Self::None) => value,
            (Self::One(a), Self::One(b)) if a == b => Self::One(a),
            (Self::Many, _) | (_, Self::Many) | (Self::One(_), Self::One(_)) => Self::Many,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Tag {
    target: Target,
    source_lca: Option<usize>,
    lower_depth: usize,
}

struct TreeIndex {
    parent: Vec<usize>,
    depth: Vec<usize>,
    tin: Vec<usize>,
    tout: Vec<usize>,
    size: Vec<usize>,
    head: Vec<usize>,
    pos: Vec<usize>,
    at_pos: Vec<usize>,
    up: Vec<Vec<usize>>,
}

impl TreeIndex {
    fn dominates(&self, a: usize, b: usize) -> bool {
        self.tin[a] <= self.tin[b] && self.tin[b] < self.tout[a]
    }

    fn lca(&self, mut a: usize, b: usize) -> usize {
        if self.dominates(a, b) {
            return a;
        }
        if self.dominates(b, a) {
            return b;
        }
        for level in (0..self.up.len()).rev() {
            let next = self.up[level][a];
            if !self.dominates(next, b) {
                a = next;
            }
        }
        self.parent[a]
    }
}

fn build_tree_index<S: std::hash::BuildHasher + Default>(
    tree: &crate::graph::analysis::DominatorTree<usize, S>,
    n: usize,
    root: usize,
) -> TreeIndex {
    let mut parent = vec![root; n];
    let mut children = vec![Vec::new(); n];
    for (node, parent_slot) in parent.iter_mut().enumerate() {
        if node == root {
            continue;
        }
        let p = tree
            .immediate_dominator(node)
            .expect("normalized CFG node must be reachable");
        *parent_slot = p;
        children[p].push(node);
    }
    for list in &mut children {
        list.sort_unstable();
    }
    let mut depth = vec![0; n];
    let mut size = vec![0; n];
    let mut heavy = vec![None; n];
    fn sizes(
        v: usize,
        children: &[Vec<usize>],
        depth: &mut [usize],
        size: &mut [usize],
        heavy: &mut [Option<usize>],
    ) {
        size[v] = 1;
        for &child in &children[v] {
            depth[child] = depth[v] + 1;
            sizes(child, children, depth, size, heavy);
            size[v] += size[child];
            if heavy[v].is_none_or(|old| size[child] > size[old]) {
                heavy[v] = Some(child);
            }
        }
    }
    sizes(root, &children, &mut depth, &mut size, &mut heavy);
    let mut tin = vec![0; n];
    let mut tout = vec![0; n];
    let mut head = vec![0; n];
    let mut pos = vec![0; n];
    let mut at_pos = vec![0; n];
    #[allow(clippy::too_many_arguments)]
    fn decompose(
        v: usize,
        h: usize,
        children: &[Vec<usize>],
        heavy: &[Option<usize>],
        head: &mut [usize],
        pos: &mut [usize],
        at_pos: &mut [usize],
        tin: &mut [usize],
        tout: &mut [usize],
        cursor: &mut usize,
    ) {
        head[v] = h;
        pos[v] = *cursor;
        tin[v] = *cursor;
        at_pos[*cursor] = v;
        *cursor += 1;
        if let Some(child) = heavy[v] {
            decompose(
                child, h, children, heavy, head, pos, at_pos, tin, tout, cursor,
            );
        }
        for &child in &children[v] {
            if Some(child) != heavy[v] {
                decompose(
                    child, child, children, heavy, head, pos, at_pos, tin, tout, cursor,
                );
            }
        }
        tout[v] = *cursor;
    }
    decompose(
        root,
        root,
        &children,
        &heavy,
        &mut head,
        &mut pos,
        &mut at_pos,
        &mut tin,
        &mut tout,
        &mut 0,
    );
    let levels = (usize::BITS - n.max(1).leading_zeros()) as usize;
    let mut up = vec![vec![root; n]; levels.max(1)];
    up[0].clone_from_slice(&parent);
    for level in 1..up.len() {
        for node in 0..n {
            up[level][node] = up[level - 1][up[level - 1][node]];
        }
    }
    TreeIndex {
        parent,
        depth,
        tin,
        tout,
        size,
        head,
        pos,
        at_pos,
        up,
    }
}

fn merge_tag(tree: &TreeIndex, dst: &mut Tag, src: Tag) {
    dst.target = dst.target.merge(src.target);
    dst.source_lca = match (dst.source_lca, src.source_lca) {
        (None, value) | (value, None) => value,
        (Some(a), Some(b)) => Some(tree.lca(a, b)),
    };
    dst.lower_depth = dst.lower_depth.max(src.lower_depth);
}

#[allow(clippy::too_many_arguments)]
fn range_update(
    tags: &mut [Tag],
    tree: &TreeIndex,
    node: usize,
    lo: usize,
    hi: usize,
    ql: usize,
    qr: usize,
    tag: Tag,
) {
    if ql >= hi || qr <= lo {
        return;
    }
    if ql <= lo && hi <= qr {
        merge_tag(tree, &mut tags[node], tag);
        return;
    }
    let mid = (lo + hi) / 2;
    range_update(tags, tree, node * 2, lo, mid, ql, qr, tag);
    range_update(tags, tree, node * 2 + 1, mid, hi, ql, qr, tag);
}

fn update_up_exclusive(
    tags: &mut [Tag],
    tree: &TreeIndex,
    mut u: usize,
    ancestor: usize,
    tag: Tag,
    n: usize,
) {
    while tree.head[u] != tree.head[ancestor] {
        range_update(
            tags,
            tree,
            1,
            0,
            n,
            tree.pos[tree.head[u]],
            tree.pos[u] + 1,
            tag,
        );
        u = tree.parent[tree.head[u]];
    }
    if u != ancestor {
        range_update(
            tags,
            tree,
            1,
            0,
            n,
            tree.pos[ancestor] + 1,
            tree.pos[u] + 1,
            tag,
        );
    }
}

fn collect_tags(
    tags: &[Tag],
    tree: &TreeIndex,
    node: usize,
    lo: usize,
    hi: usize,
    inherited: Tag,
    out: &mut [Tag],
) {
    let mut combined = inherited;
    merge_tag(tree, &mut combined, tags[node]);
    if hi - lo == 1 {
        out[tree.at_pos[lo]] = combined;
        return;
    }
    let mid = (lo + hi) / 2;
    collect_tags(tags, tree, node * 2, lo, mid, combined, out);
    collect_tags(tags, tree, node * 2 + 1, mid, hi, combined, out);
}

struct MinTree {
    n: usize,
    values: Vec<usize>,
}
impl MinTree {
    fn new(values: &[usize]) -> Self {
        let n = values.len().next_power_of_two();
        let mut data = vec![usize::MAX; n * 2];
        data[n..n + values.len()].copy_from_slice(values);
        for i in (1..n).rev() {
            data[i] = data[i * 2].min(data[i * 2 + 1]);
        }
        Self { n, values: data }
    }
    fn rightmost_at_most(&self, ql: usize, qr: usize, limit: usize) -> Option<usize> {
        fn find(
            data: &[usize],
            node: usize,
            lo: usize,
            hi: usize,
            ql: usize,
            qr: usize,
            limit: usize,
        ) -> Option<usize> {
            if ql >= hi || qr <= lo || data[node] > limit {
                return None;
            }
            if hi - lo == 1 {
                return Some(lo);
            }
            let mid = (lo + hi) / 2;
            find(data, node * 2 + 1, mid, hi, ql, qr, limit)
                .or_else(|| find(data, node * 2, lo, mid, ql, qr, limit))
        }
        find(&self.values, 1, 0, self.n, ql, qr, limit)
    }
}

fn deepest_good(
    tree: &TreeIndex,
    min_tree: &MinTree,
    entry: usize,
    mut limit: usize,
) -> Option<usize> {
    while tree.head[limit] != tree.head[entry] {
        let head = tree.head[limit];
        if let Some(pos) =
            min_tree.rightmost_at_most(tree.pos[head], tree.pos[limit] + 1, tree.depth[entry])
        {
            return Some(tree.at_pos[pos]);
        }
        limit = tree.parent[head];
    }
    (tree.pos[entry] < tree.pos[limit])
        .then(|| {
            min_tree.rightmost_at_most(tree.pos[entry] + 1, tree.pos[limit] + 1, tree.depth[entry])
        })
        .flatten()
        .map(|pos| tree.at_pos[pos])
}

#[derive(Clone, Copy)]
struct Candidate {
    entry: usize,
    exit: usize,
    removed: Option<usize>,
    size: usize,
}

/// Detect maximal, pairwise-disjoint hammocks. Components without a genuine
/// sink, nodes unreachable from `root`, or nodes unable to reach a sink are
/// deliberately left to canonical SESE/flat layout rather than normalized by
/// inventing semantics for a nonterminating CFG.
pub(super) fn compute_hammocks<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    root: NodeId,
) -> Vec<Hammock<NodeId, EdgeId>>
where
    NodeId: Identifier + std::fmt::Debug + Ord,
    EdgeId: Identifier + std::fmt::Debug + Ord,
{
    let original_n = component.len();
    if original_n < 3 {
        return Vec::new();
    }
    let mut nodes = component.to_vec();
    nodes.sort();
    let index: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, &node)| (node, i))
        .collect();
    let root = index[&root];
    let set: HashSet<_> = nodes.iter().copied().collect();
    let mut edges = Vec::new();
    let mut out_degree = vec![0usize; original_n];
    let mut original_edges: Vec<_> = graph
        .edges()
        .filter(|e| set.contains(&e.from_id()) && set.contains(&e.to_id()))
        .map(|e| (e.id(), index[&e.from_id()], index[&e.to_id()]))
        .collect();
    original_edges.sort_by_key(|edge| edge.0);
    for &(_, u, v) in &original_edges {
        edges.push((u, v));
        if u != v {
            out_degree[u] += 1;
        }
    }
    let sinks: Vec<_> = (0..original_n).filter(|&v| out_degree[v] == 0).collect();
    if sinks.is_empty() {
        return Vec::new();
    }
    let sink = if sinks.len() == 1 {
        sinks[0]
    } else {
        original_n
    };
    let n = original_n + usize::from(sinks.len() > 1);
    if sinks.len() > 1 {
        for &v in &sinks {
            edges.push((v, sink));
        }
    }
    let mut normalized = OwningGraph::<usize, usize, (), ()>::default();
    for _ in 0..n {
        normalized.make_node(());
    }
    for &(u, v) in &edges {
        normalized.make_edge(u, v, ());
    }
    let dominators = compute_dominators(&normalized, root);
    if (0..n).any(|v| v != root && dominators.immediate_dominator(v).is_none()) {
        return Vec::new();
    }
    // Co-reachability is part of the detector's contract.
    let mut reverse = vec![Vec::new(); n];
    for &(u, v) in &edges {
        reverse[v].push(u);
    }
    let mut seen = vec![false; n];
    let mut stack = vec![sink];
    while let Some(v) = stack.pop() {
        if seen[v] {
            continue;
        }
        seen[v] = true;
        stack.extend(reverse[v].iter().copied());
    }
    if seen.iter().any(|seen| !seen) {
        return Vec::new();
    }

    let tree = build_tree_index(&dominators, n, root);
    let mut range_tags = vec![Tag::default(); n * 4 + 4];
    for &(u, v) in &edges {
        let a = tree.lca(u, v);
        if a == u {
            continue;
        }
        let lower_depth = tree.depth[a] + usize::from(a != v);
        update_up_exclusive(
            &mut range_tags,
            &tree,
            u,
            a,
            Tag {
                target: Target::One(v),
                source_lca: Some(u),
                lower_depth,
            },
            n,
        );
    }
    let mut summaries = vec![Tag::default(); n];
    collect_tags(&range_tags, &tree, 1, 0, n, Tag::default(), &mut summaries);
    let lower_by_pos: Vec<_> = tree
        .at_pos
        .iter()
        .map(|&v| summaries[v].lower_depth)
        .collect();
    let min_tree = MinTree::new(&lower_by_pos);
    let mut candidates = Vec::new();
    for (entry, summary) in summaries.iter().enumerate().take(original_n) {
        // A layout hammock needs a real original boundary edge. The CFG root
        // has no outside predecessor and is represented by the tree root.
        if entry == root {
            continue;
        }
        if let Target::One(exit) = summary.target {
            if exit < original_n && !tree.dominates(entry, sink) && tree.size[entry] >= 2 {
                candidates.push(Candidate {
                    entry,
                    exit,
                    removed: None,
                    size: tree.size[entry],
                });
                continue;
            }
        }
        let escape = summary.source_lca;
        let mut limit = match escape {
            Some(l) if l != entry => l,
            Some(_) => continue,
            None if tree.dominates(entry, sink) => sink,
            None => continue,
        };
        if tree.dominates(entry, sink) && escape.is_some() {
            limit = tree.lca(limit, sink);
        }
        if limit == entry {
            continue;
        }
        let Some(exit) = deepest_good(&tree, &min_tree, entry, limit) else {
            continue;
        };
        if exit >= original_n {
            continue;
        }
        let size = tree.size[entry] - tree.size[exit];
        if size >= 2 {
            candidates.push(Candidate {
                entry,
                exit,
                removed: Some(exit),
                size,
            });
        }
    }
    candidates.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.size),
            c.entry,
            c.exit,
            c.removed.is_some(),
        )
    });
    // A subtree difference occupies at most two Euler intervals. Selection can
    // therefore reject overlaps without materializing candidate node sets.
    let intervals = |candidate: Candidate| {
        let mut value = vec![(tree.tin[candidate.entry], tree.tout[candidate.entry])];
        if let Some(exit) = candidate.removed {
            value = vec![
                (tree.tin[candidate.entry], tree.tin[exit]),
                (tree.tout[exit], tree.tout[candidate.entry]),
            ];
            value.retain(|(lo, hi)| lo < hi);
        }
        value
    };
    let mut claimed = vec![0usize; n + 1];
    let prefix = |values: &[usize], mut end: usize| {
        let mut sum = 0;
        while end > 0 {
            sum += values[end];
            end &= end - 1;
        }
        sum
    };
    let mut selected = Vec::new();
    for candidate in candidates {
        let ranges = intervals(candidate);
        if ranges
            .iter()
            .any(|&(lo, hi)| prefix(&claimed, hi) != prefix(&claimed, lo))
        {
            continue;
        }
        // Accepted regions are disjoint, so point updates touch at most n
        // positions over the complete selection pass.
        for &(lo, hi) in &ranges {
            for pos in lo..hi {
                let mut index = pos + 1;
                while index <= n {
                    claimed[index] += 1;
                    index += index & index.wrapping_neg();
                }
            }
        }
        selected.push(candidate);
    }

    let mut owner = vec![None; original_n];
    let mut region_nodes = Vec::with_capacity(selected.len());
    let mut entry_owner = vec![None; original_n];
    for (id, &candidate) in selected.iter().enumerate() {
        entry_owner[candidate.entry] = Some(id);
        let mut members: Vec<_> = intervals(candidate)
            .into_iter()
            .flat_map(|(lo, hi)| tree.at_pos[lo..hi].iter().copied())
            .filter(|&v| v < original_n)
            .collect();
        members.sort_unstable();
        for &node in &members {
            owner[node] = Some(id);
        }
        region_nodes.push(members);
    }
    // Recover stable original boundary-edge IDs in one final edge pass.
    let mut entry_edges = vec![None; selected.len()];
    let mut exit_edges = vec![None; selected.len()];
    for &(edge, from, to) in &original_edges {
        if let Some(id) = entry_owner[to]
            && owner[from] != Some(id)
            && entry_edges[id].is_none()
        {
            entry_edges[id] = Some(edge);
        }
        if let Some(id) = owner[from]
            && selected[id].exit == to
            && exit_edges[id].is_none()
        {
            exit_edges[id] = Some(edge);
        }
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(id, _)| {
            Some(Hammock {
                nodes: region_nodes[id].iter().copied().map(|v| nodes[v]).collect(),
                entry_edge: entry_edges[id]?,
                exit_edge: exit_edges[id]?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use jstd_derive::Identifier;

    use super::{Hammock, compute_hammocks};
    use crate::graph::{Graph, edge::Edge, owning::OwningGraph};

    #[derive(Identifier)]
    struct N(usize);
    #[derive(Identifier)]
    struct E(usize);
    type G = OwningGraph<N, E, (), ()>;

    fn add(g: &mut G, from: N, to: N) {
        g.make_edge(from, to, ());
    }

    fn nodes(hammock: &Hammock<N, E>) -> Vec<usize> {
        hammock.nodes.iter().copied().map(usize::from).collect()
    }

    fn verify_boundaries(g: &G, hammocks: &[Hammock<N, E>]) {
        for hammock in hammocks {
            let inside: std::collections::HashSet<_> = hammock.nodes.iter().copied().collect();
            let entry = g.get_edge(hammock.entry_edge).unwrap().to_id();
            let exit = g.get_edge(hammock.exit_edge).unwrap().to_id();
            assert!(inside.contains(&entry));
            assert!(!inside.contains(&exit));
            for edge in g.edges() {
                let from = inside.contains(&edge.from_id());
                let to = inside.contains(&edge.to_id());
                if !from && to {
                    assert_eq!(edge.to_id(), entry, "side entry");
                }
                if from && !to {
                    assert_eq!(edge.to_id(), exit, "side exit");
                }
            }
        }
    }

    #[test]
    fn detects_multiple_exit_sources_with_one_target() {
        let mut g = G::default();
        let n: Vec<_> = (0..7).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[1], n[3]);
        add(&mut g, n[2], n[4]);
        add(&mut g, n[3], n[4]);
        add(&mut g, n[2], n[5]);
        add(&mut g, n[4], n[5]);
        add(&mut g, n[0], n[5]);
        add(&mut g, n[5], n[6]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert!(
            result.iter().any(|h| nodes(h) == vec![1, 2, 3, 4]),
            "{:?}",
            result.iter().map(nodes).collect::<Vec<_>>()
        );
        verify_boundaries(&g, &result);
    }

    #[test]
    fn detects_type_a_exit_with_unrelated_predecessor() {
        let mut g = G::default();
        let n: Vec<_> = (0..7).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[0], n[2]);
        add(&mut g, n[1], n[3]);
        add(&mut g, n[3], n[4]);
        add(&mut g, n[4], n[5]);
        add(&mut g, n[2], n[5]);
        add(&mut g, n[5], n[6]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert!(result.iter().any(|h| nodes(h) == vec![1, 3, 4]));
        verify_boundaries(&g, &result);
    }

    #[test]
    fn detects_loop_hammock() {
        let mut g = G::default();
        let n: Vec<_> = (0..6).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[2], n[3]);
        add(&mut g, n[3], n[2]);
        add(&mut g, n[2], n[4]);
        add(&mut g, n[3], n[4]);
        add(&mut g, n[0], n[4]);
        add(&mut g, n[4], n[5]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert!(
            result.iter().any(|h| nodes(h) == vec![1, 2, 3]),
            "{:?}",
            result.iter().map(nodes).collect::<Vec<_>>()
        );
        verify_boundaries(&g, &result);
    }

    #[test]
    fn chooses_outer_nested_hammock_deterministically() {
        let mut g = G::default();
        let n: Vec<_> = (0..7).map(|_| g.make_node(())).collect();
        for pair in n.windows(2) {
            add(&mut g, pair[0], pair[1]);
        }
        let first = compute_hammocks(&g, &n, n[0]);
        let second = compute_hammocks(&g, &n, n[0]);
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert_eq!(nodes(&first[0]), vec![1, 2, 3, 4, 5]);
        verify_boundaries(&g, &first);
    }

    #[test]
    fn supports_parallel_edges_and_self_loops() {
        let mut g = G::default();
        let n: Vec<_> = (0..5).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[2], n[3]);
        add(&mut g, n[3], n[4]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert_eq!(nodes(&result[0]), vec![1, 2, 3]);
        verify_boundaries(&g, &result);
    }

    #[test]
    fn rejects_side_entry_into_internal_node() {
        let mut g = G::default();
        let n: Vec<_> = (0..7).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[0], n[2]);
        add(&mut g, n[1], n[3]);
        add(&mut g, n[3], n[4]);
        add(&mut g, n[2], n[4]);
        add(&mut g, n[4], n[5]);
        add(&mut g, n[5], n[6]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert!(
            !result
                .iter()
                .any(|h| h.nodes.contains(&n[3]) && h.nodes.contains(&n[4]))
        );
        verify_boundaries(&g, &result);
    }

    #[test]
    fn rejects_different_external_exit_targets() {
        let mut g = G::default();
        let n: Vec<_> = (0..7).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[1], n[3]);
        add(&mut g, n[2], n[4]);
        add(&mut g, n[3], n[5]);
        add(&mut g, n[4], n[6]);
        add(&mut g, n[5], n[6]);
        let result = compute_hammocks(&g, &n, n[0]);
        assert!(
            !result.iter().any(|h| nodes(h) == vec![1, 2, 3]),
            "{:?}",
            result.iter().map(nodes).collect::<Vec<_>>()
        );
        verify_boundaries(&g, &result);
    }

    #[test]
    fn rejects_nonterminating_component_without_sink() {
        let mut g = G::default();
        let n: Vec<_> = (0..3).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[2], n[1]);
        assert!(compute_hammocks(&g, &n, n[0]).is_empty());
    }

    #[test]
    fn rejects_nodes_unreachable_from_selected_root() {
        let mut g = G::default();
        let n: Vec<_> = (0..4).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[2], n[1]);
        add(&mut g, n[1], n[3]);
        assert!(compute_hammocks(&g, &n, n[0]).is_empty());
    }

    #[test]
    fn multiple_returns_do_not_expose_synthetic_exit() {
        let mut g = G::default();
        let n: Vec<_> = (0..6).map(|_| g.make_node(())).collect();
        add(&mut g, n[0], n[1]);
        add(&mut g, n[1], n[2]);
        add(&mut g, n[1], n[3]);
        add(&mut g, n[2], n[4]);
        add(&mut g, n[3], n[5]);
        let result = compute_hammocks(&g, &n, n[0]);
        verify_boundaries(&g, &result);
        for hammock in result {
            assert!(g.get_edge(hammock.entry_edge).is_some());
            assert!(g.get_edge(hammock.exit_edge).is_some());
        }
    }
}
