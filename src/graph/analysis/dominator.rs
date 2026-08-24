//! Dominator analysis for directed graphs.
//!
//! Definitions used here follow control-flow graph conventions:
//! - A node `d` dominates `n` if every path from the graph root to `n` passes
//!   through `d`.
//! - The root dominates only itself.
//!
//! The main entry point is [`compute_dominators`], which runs the
//! Lengauer–Tarjan algorithm and returns a [`DominatorTree`]. Full dominator
//! sets and the children map are computed lazily on first access.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash};

use crate::graph::{Cfg, FxBuildHasher};

pub fn reachable_from_root<C: Cfg>(graph: &C, root: C::NodeId) -> Vec<C::NodeId> {
    let mut visited: HashSet<C::NodeId, C::Hasher> = HashSet::default();
    let mut order = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if !visited.insert(node) {
            continue;
        }
        order.push(node);
        for succ in graph.successors(node) {
            if !visited.contains(&succ) {
                stack.push(succ);
            }
        }
    }
    order
}

// ---------------------------------------------------------------------------
// DominatorTree
// ---------------------------------------------------------------------------

/// Dominator tree for a reachable subgraph.
///
/// The immediate-dominator map is computed eagerly by [`compute_dominators`].
/// Full dominator sets and the children map are computed lazily on first
/// access and then cached.
///
/// The tree is generic over the hasher `S` used for its internal, node-keyed
/// maps — pinned to the source graph's [`Graph::Hasher`] by
/// [`compute_dominators`] — rather than hardcoding a concrete one. It defaults
/// to [`FxBuildHasher`] so the common `DominatorTree<NodeId>` spelling keeps
/// the fast, deterministic hasher that graph consumers (e.g. qcode's `Context`)
/// select.
pub struct DominatorTree<N: Copy + Hash + Eq, S: BuildHasher + Default = FxBuildHasher> {
    root: N,
    idom: HashMap<N, N, S>,
    preds: HashMap<N, Vec<N>, S>,
    dominator_sets: OnceCell<HashMap<N, HashSet<N, S>, S>>,
    children: OnceCell<HashMap<N, Vec<N>, S>>,
    frontier: OnceCell<HashMap<N, HashSet<N, S>, S>>,
}

impl<N: Copy + Hash + Eq, S: BuildHasher + Default> DominatorTree<N, S> {
    /// The root of the dominator tree (the CFG entry node).
    pub fn root(&self) -> N {
        self.root
    }

    /// Returns the immediate dominator of `node`, or `None` if `node` is the
    /// root or was not in the analyzed subgraph.
    pub fn immediate_dominator(&self, node: N) -> Option<N> {
        self.idom.get(&node).copied()
    }

    /// Returns the children of `node` in the dominator tree — the nodes for
    /// which `node` is the immediate dominator.
    pub fn children_of(&self, node: N) -> &[N] {
        self.children_map()
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns `true` if `dominator` dominates `node`.
    ///
    /// Every node dominates itself. Triggers lazy set computation on first
    /// call.
    pub fn dominates(&self, dominator: N, node: N) -> bool {
        self.dominator_sets_map()
            .get(&node)
            .is_some_and(|s| s.contains(&dominator))
    }

    /// Returns the set of all nodes that dominate `node`, or `None` if `node`
    /// was not in the analyzed subgraph.
    ///
    /// Triggers lazy set computation on first call.
    pub fn dominator_set(&self, node: N) -> Option<&HashSet<N, S>> {
        self.dominator_sets_map().get(&node)
    }

    fn children_map(&self) -> &HashMap<N, Vec<N>, S> {
        self.children.get_or_init(|| {
            let mut map: HashMap<N, Vec<N>, S> = HashMap::default();
            for (&n, &parent) in &self.idom {
                map.entry(parent).or_default().push(n);
            }
            map
        })
    }

    /// Returns the dominator frontier for every node in the analyzed subgraph.
    ///
    /// The dominator frontier of `n` is the set of nodes `y` such that `n`
    /// dominates a predecessor of `y` but does not strictly dominate `y`.
    /// Frontiers are used by SSA construction to determine phi-node placement.
    ///
    /// Computed lazily on first call using the Cytron et al. algorithm.
    pub fn dominator_frontier(&self) -> &HashMap<N, HashSet<N, S>, S> {
        self.frontier.get_or_init(|| {
            let mut all_nodes: HashSet<N, S> = self.idom.keys().copied().collect();
            all_nodes.insert(self.root);

            let mut frontier: HashMap<N, HashSet<N, S>, S> =
                all_nodes.iter().map(|&n| (n, HashSet::default())).collect();

            for (&n, preds) in &self.preds {
                let idom_n = match self.idom.get(&n) {
                    Some(&d) => d,
                    None => continue,
                };

                if preds.len() < 2 {
                    continue;
                }

                for &p in preds {
                    let mut runner = p;
                    while runner != idom_n {
                        frontier.entry(runner).or_default().insert(n);
                        match self.idom.get(&runner) {
                            Some(&parent) => runner = parent,
                            None => break,
                        }
                    }
                }
            }

            frontier
        })
    }

    fn dominator_sets_map(&self) -> &HashMap<N, HashSet<N, S>, S> {
        self.dominator_sets
            .get_or_init(|| compute_dominator_sets(&self.idom, self.root))
    }
}

// ---------------------------------------------------------------------------
// compute_dominators
// ---------------------------------------------------------------------------

/// Computes the dominator tree for all nodes reachable from `root`.
///
/// Runs the Lengauer–Tarjan algorithm (O(n α(n))). Returns a [`DominatorTree`]
/// whose immediate-dominator map is available immediately; full dominator sets
/// and the children map are available lazily via [`DominatorTree::dominator_set`]
/// and [`DominatorTree::children_of`].
pub fn compute_dominators<C: Cfg>(
    graph: &C,
    root: C::NodeId,
) -> DominatorTree<C::NodeId, C::Hasher> {
    let nodes = reachable_from_root(graph, root);
    let node_set: HashSet<C::NodeId, C::Hasher> = nodes.iter().copied().collect();

    let result = run_lengauer_tarjan(graph, &node_set, root);

    DominatorTree {
        root,
        idom: result.idom,
        preds: result.preds,
        dominator_sets: OnceCell::new(),
        children: OnceCell::new(),
        frontier: OnceCell::new(),
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Materializes full dominator sets from the immediate-dominator map.
///
/// For each node `n`, `dom[n]` is the set of every node on the path from the
/// root to `n` in the dominator tree, including `n` itself and the root.
fn compute_dominator_sets<N: Copy + Hash + Eq, S: BuildHasher + Default>(
    idom: &HashMap<N, N, S>,
    root: N,
) -> HashMap<N, HashSet<N, S>, S> {
    let mut all_nodes: HashSet<N, S> = idom.keys().copied().collect();
    all_nodes.insert(root);

    let mut out = HashMap::default();
    for n in all_nodes {
        let mut set = HashSet::default();
        let mut cursor = n;
        loop {
            set.insert(cursor);
            match idom.get(&cursor) {
                Some(&parent) => cursor = parent,
                None => break,
            }
        }
        out.insert(n, set);
    }
    out
}

struct TarjanResult<C: Cfg> {
    idom: HashMap<C::NodeId, C::NodeId, C::Hasher>,
    preds: HashMap<C::NodeId, Vec<C::NodeId>, C::Hasher>,
}

impl<C: Cfg> Default for TarjanResult<C> {
    fn default() -> Self {
        Self {
            idom: HashMap::default(),
            preds: HashMap::default(),
        }
    }
}

/// Runs the Lengauer–Tarjan algorithm and returns the idom map and predecessor
/// map for the reachable subgraph.
fn run_lengauer_tarjan<C: Cfg>(
    graph: &C,
    node_set: &HashSet<C::NodeId, C::Hasher>,
    root: C::NodeId,
) -> TarjanResult<C> {
    let mut state = LtState::new(graph, node_set);
    state.dfs(root, 0);

    let n = state.last_index();
    if n == 0 {
        return TarjanResult::default();
    }

    for w in (2..=n).rev() {
        let preds = state.pred[w].clone();
        for v in preds {
            let u = state.eval(v);
            if state.semi[u] < state.semi[w] {
                state.semi[w] = state.semi[u];
            }
        }

        let semi_w = state.semi[w];
        state.bucket[semi_w].push(w);

        let p = state.parent[w];
        state.link(p, w);

        let bucket_parent = std::mem::take(&mut state.bucket[p]);
        for v in bucket_parent {
            let u = state.eval(v);
            if state.semi[u] < state.semi[v] {
                state.idom[v] = u;
            } else {
                state.idom[v] = p;
            }
        }
    }

    for w in 2..=n {
        if state.idom[w] != state.semi[w] {
            state.idom[w] = state.idom[state.idom[w]];
        }
    }

    // Convert DFS-index structures to NodeId maps.
    let mut result = TarjanResult::default();

    let root_node = state.vertex[1].expect("root must have DFS index 1");
    result.preds.entry(root_node).or_default();

    for w in 2..=n {
        let node = state.vertex[w].expect("DFS index must map to a node");
        let idom_idx = state.idom[w];
        let idom_node = state.vertex[idom_idx].expect("idom index must map to a node");
        result.idom.insert(node, idom_node);

        let pred_nodes: Vec<C::NodeId> = state.pred[w]
            .iter()
            .map(|&idx| state.vertex[idx].expect("pred index must map to a node"))
            .collect();
        result.preds.insert(node, pred_nodes);
    }

    result
}

// ---------------------------------------------------------------------------
// Lengauer–Tarjan internal state
// ---------------------------------------------------------------------------

struct LtState<'graph, C: Cfg> {
    graph: &'graph C,
    node_set: &'graph HashSet<C::NodeId, C::Hasher>,

    number: HashMap<C::NodeId, usize, C::Hasher>,
    vertex: Vec<Option<C::NodeId>>, // 1-based DFS index -> node
    parent: Vec<usize>,
    semi: Vec<usize>,
    idom: Vec<usize>,
    ancestor: Vec<usize>,
    label: Vec<usize>,
    bucket: Vec<Vec<usize>>,
    pred: Vec<Vec<usize>>,
}

impl<'graph, C: Cfg> LtState<'graph, C> {
    fn new(graph: &'graph C, node_set: &'graph HashSet<C::NodeId, C::Hasher>) -> Self {
        Self {
            graph,
            node_set,
            number: HashMap::default(),
            vertex: vec![None],
            parent: vec![0],
            semi: vec![0],
            idom: vec![0],
            ancestor: vec![0],
            label: vec![0],
            bucket: vec![Vec::new()],
            pred: vec![Vec::new()],
        }
    }

    fn last_index(&self) -> usize {
        self.vertex.len().saturating_sub(1)
    }

    fn push_vertex(&mut self, node: C::NodeId, parent: usize) -> usize {
        let idx = self.vertex.len();
        self.number.insert(node, idx);
        self.vertex.push(Some(node));
        self.parent.push(parent);
        self.semi.push(idx);
        self.idom.push(0);
        self.ancestor.push(0);
        self.label.push(idx);
        self.bucket.push(Vec::new());
        self.pred.push(Vec::new());
        idx
    }

    fn dfs(&mut self, node: C::NodeId, parent: usize) {
        if self.number.contains_key(&node) || !self.node_set.contains(&node) {
            return;
        }

        let node_idx = self.push_vertex(node, parent);

        let succs: Vec<C::NodeId> = self.graph.successors(node).collect();
        for succ in succs {
            if !self.node_set.contains(&succ) {
                continue;
            }

            if !self.number.contains_key(&succ) {
                self.dfs(succ, node_idx);
            }

            if let Some(&succ_idx) = self.number.get(&succ) {
                self.pred[succ_idx].push(node_idx);
            }
        }
    }

    fn link(&mut self, parent: usize, child: usize) {
        self.ancestor[child] = parent;
    }

    fn compress(&mut self, v: usize) {
        let a = self.ancestor[v];
        if a != 0 {
            let aa = self.ancestor[a];
            if aa != 0 {
                self.compress(a);

                if self.semi[self.label[a]] < self.semi[self.label[v]] {
                    self.label[v] = self.label[a];
                }

                self.ancestor[v] = self.ancestor[a];
            }
        }
    }

    fn eval(&mut self, v: usize) -> usize {
        if self.ancestor[v] == 0 {
            return self.label[v];
        }

        self.compress(v);
        let a = self.ancestor[v];
        if a != 0 && self.semi[self.label[a]] < self.semi[self.label[v]] {
            self.label[v] = self.label[a];
        }

        self.label[v]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use jstd_derive::Identifier;

    use super::{compute_dominators, reachable_from_root};
    use crate::graph::owning::OwningGraph;

    #[derive(Identifier)]
    struct NodeId(usize);

    #[derive(Identifier)]
    struct EdgeId(usize);

    type TestGraph = OwningGraph<NodeId, EdgeId, (), ()>;

    #[test]
    fn computes_reachability_from_root() {
        let mut graph = TestGraph::default();

        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        let x = graph.make_node(());

        graph.make_edge(a, b, ());
        graph.make_edge(b, c, ());
        graph.make_edge(c, d, ());

        let reachable: HashSet<_> = reachable_from_root(&graph, a).into_iter().collect();

        assert!(reachable.contains(&a));
        assert!(reachable.contains(&b));
        assert!(reachable.contains(&c));
        assert!(reachable.contains(&d));
        assert!(!reachable.contains(&x));
    }

    #[test]
    fn computes_dominators_on_diamond() {
        // a -> b, a -> c, b -> d, c -> d, d -> e
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        let e = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());
        graph.make_edge(d, e, ());

        let tree = compute_dominators(&graph, a);

        assert_eq!(tree.dominator_set(a), Some(&HashSet::from_iter([a])));
        assert!(tree.dominates(a, b) && tree.dominates(b, b));
        assert!(tree.dominates(a, c) && tree.dominates(c, c));
        assert!(tree.dominates(a, d));
        assert!(!tree.dominates(b, d));
        assert!(!tree.dominates(c, d));
        assert!(tree.dominates(a, e) && tree.dominates(d, e) && tree.dominates(e, e));
    }

    #[test]
    fn immediate_dominator_on_chain() {
        // a -> b -> c -> d
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(b, c, ());
        graph.make_edge(c, d, ());

        let tree = compute_dominators(&graph, a);

        assert_eq!(tree.immediate_dominator(b), Some(a));
        assert_eq!(tree.immediate_dominator(c), Some(b));
        assert_eq!(tree.immediate_dominator(d), Some(c));
        assert_eq!(tree.immediate_dominator(a), None);
    }

    #[test]
    fn immediate_dominator_on_diamond() {
        // a -> b, a -> c, b -> d, c -> d
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());

        let tree = compute_dominators(&graph, a);

        assert_eq!(tree.immediate_dominator(b), Some(a));
        assert_eq!(tree.immediate_dominator(c), Some(a));
        assert_eq!(tree.immediate_dominator(d), Some(a));
        assert_eq!(tree.immediate_dominator(a), None);
    }

    #[test]
    fn dominator_frontier_on_diamond() {
        // a -> b, a -> c, b -> d, c -> d
        // idom: b=a, c=a, d=a
        // DF(a)={}, DF(b)={d}, DF(c)={d}, DF(d)={}
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());

        let tree = compute_dominators(&graph, a);
        let df = tree.dominator_frontier();

        assert_eq!(df[&a], HashSet::default());
        assert_eq!(df[&b], HashSet::from_iter([d]));
        assert_eq!(df[&c], HashSet::from_iter([d]));
        assert_eq!(df[&d], HashSet::default());
    }

    #[test]
    fn dominator_frontier_on_loop() {
        // a -> b -> c -> b (back-edge), b -> d
        // idom: b=a, c=b, d=b
        // DF(b)={b} (b is its own frontier due to the back-edge), DF(c)={b}, DF(d)={}
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(b, c, ());
        graph.make_edge(c, b, ());
        graph.make_edge(b, d, ());

        let tree = compute_dominators(&graph, a);
        let df = tree.dominator_frontier();

        assert_eq!(df[&a], HashSet::default());
        assert_eq!(df[&b], HashSet::from_iter([b]));
        assert_eq!(df[&c], HashSet::from_iter([b]));
        assert_eq!(df[&d], HashSet::default());
    }

    #[test]
    fn dominator_tree_children_on_diamond() {
        // a -> b, a -> c, b -> d, c -> d  =>  idom(b)=a, idom(c)=a, idom(d)=a
        let mut graph = TestGraph::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());

        let tree = compute_dominators(&graph, a);

        let mut a_children: Vec<_> = tree.children_of(a).to_vec();
        a_children.sort_by_key(|id| Into::<usize>::into(*id));
        assert_eq!(a_children, vec![b, c, d]);
        assert!(tree.children_of(d).is_empty());
    }
}
