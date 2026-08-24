//! Graph data structure with typed node and edge identifiers.
//!
//! `Graph` stores node and edge payloads in separate maps and exposes typed
//! reference wrappers (`NodeRef`, `NodeMutRef`, `EdgeRef`, `EdgeMutRef`) to
//! navigate and mutate the structure.

use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::{BuildHasher, Hash};

pub mod analysis;
pub mod edge;
pub mod node;
pub mod owning;
pub mod parse;

pub use edge::{Edge, EdgeMut};
pub use node::{Node, NodeMut};
pub use parse::TestGraph;

/// Recommended fixed-seed hasher for `Graph::Hasher`.
///
/// The graph traits are generic over the hasher (see [`Graph::Hasher`]); this
/// re-export lets consumers select a deterministic, fast hasher for their
/// incident-edge sets without taking a direct `rustc-hash` dependency.
pub use rustc_hash::FxBuildHasher;

/// A typed graph container.
///
/// The graph is parameterized by:
/// - `NodeId`: strongly typed node identifier
/// - `EdgeId`: strongly typed edge identifier
pub trait Graph {
    // Node/edge IDs need only be cheap, hashable, totally-ordered handles — not
    // `Registry` `Identifier`s. This lets a graph use *composite* IDs (e.g.
    // qcode's `BlockId { func, local }`) that index no global arena directly,
    // while `OwningGraph` still pins `Identifier` on its own type parameters.
    type NodeId: Copy + Eq + Hash + Ord + Debug;
    type EdgeId: Copy + Eq + Hash + Ord + Debug;

    /// Hasher backing the graph's incident-edge sets and DFS bookkeeping.
    ///
    /// The trait is generic over the hasher rather than pinning a concrete one:
    /// picking a fixed-seed hasher (e.g. `rustc_hash::FxBuildHasher`) makes
    /// `predecessors()`/`successors()` iteration order deterministic across
    /// runs, whereas the std default (`RandomState`) reseeds per process.
    type Hasher: BuildHasher + Default;

    type Node<'graph>: Node<'graph, Graph = Self>
    where
        Self: 'graph;

    type Edge<'graph>: Edge<'graph, Graph = Self>
    where
        Self: 'graph;

    /// Gets a node in the graph by its identifier, if it exists.
    fn get_node(&self, id: Self::NodeId) -> Option<Self::Node<'_>>;

    fn get_edge(&self, id: Self::EdgeId) -> Option<Self::Edge<'_>>;

    /// Iterates over all nodes in insertion identifier order.
    fn nodes(&self) -> impl Iterator<Item = Self::Node<'_>> + '_;

    /// Iterates over all edges in insertion identifier order.
    fn edges(&self) -> impl Iterator<Item = Self::Edge<'_>> + '_;

    /// A dfs iterator over the graph starting from the root, if present.
    fn dfs(&self, root: Self::NodeId) -> DfsIter<'_, Directed, Self>
    where
        Self: Sized,
    {
        DfsIter {
            graph: self,
            visited: HashSet::default(),
            stack: vec![(None, root)],
            _marker: std::marker::PhantomData,
        }
    }

    /// A dfs iterator over the graph treating edges as undirected.
    fn undirected_dfs(&self, root: Self::NodeId) -> DfsIter<'_, Undirected, Self>
    where
        Self: Sized,
    {
        DfsIter {
            graph: self,
            visited: HashSet::default(),
            stack: vec![(None, root)],
            _marker: std::marker::PhantomData,
        }
    }
}

/// The mutable half of [`Graph`]: node/edge handles that can rewrite the graph
/// structure, plus structural edits.
///
/// Read-only algorithms (dominators, DFS, loop analysis) bound only [`Graph`], so
/// a read-only view (e.g. a checked-out function's CFG behind an immutable
/// reference) can implement `Graph` without providing a mutable surface it does
/// not have. Graphs that own their storage (e.g. [`owning::OwningGraph`]) also
/// implement `GraphMut`.
pub trait GraphMut: Graph {
    type NodeMut<'graph>: NodeMut<'graph, Graph = Self>
    where
        Self: 'graph;

    type EdgeMut<'graph>: EdgeMut<'graph, Graph = Self>
    where
        Self: 'graph;

    fn get_node_mut(&mut self, id: Self::NodeId) -> Option<Self::NodeMut<'_>>;

    fn get_edge_mut(&mut self, id: Self::EdgeId) -> Option<Self::EdgeMut<'_>>;

    /// Merges `remove` into `keep` at the graph-structural level.
    ///
    /// - Removes `direct_edge` (the edge from `keep` to `remove`) from both
    ///   nodes' incident edge sets.
    /// - Rehomes every outgoing edge of `remove` so that it originates from
    ///   `keep` instead, updating both the edge record and the nodes' edge sets.
    ///
    /// The caller is responsible for any payload-level cleanup (e.g. removing
    /// `remove` from a parent list, transferring instruction lists, etc.).
    fn merge_nodes(&mut self, keep: Self::NodeId, remove: Self::NodeId, direct_edge: Self::EdgeId)
    where
        Self: Sized,
    {
        // 1. Remove the direct edge from both nodes' incident sets.
        self.get_node_mut(keep).unwrap().remove_edge_id(direct_edge);
        self.get_node_mut(remove)
            .unwrap()
            .remove_edge_id(direct_edge);

        // 2. Collect outgoing edges of `remove` (releasing the immutable borrow).
        let outgoing: Vec<Self::EdgeId> = self
            .get_node(remove)
            .unwrap()
            .children()
            .map(|item| item.edge_id())
            .collect();

        // 3. Rehome each outgoing edge from `remove` to `keep`.
        for eid in outgoing {
            self.get_edge_mut(eid).unwrap().set_from(keep);
            self.get_node_mut(keep).unwrap().add_edge_id(eid);
            self.get_node_mut(remove).unwrap().remove_edge_id(eid);
        }
    }
}

/// A minimal control-flow-graph view: the successor relation on node ids.
///
/// Dominator and post-dominator analysis need only two things from a graph: the
/// successors of a node id, and a deterministic [`Hasher`](Cfg::Hasher) for their
/// internal node-keyed maps. Bounding those algorithms on `Cfg` instead of the
/// full [`Graph`] lets a caller expose just a successor relation — e.g. a
/// checked-out function's CFG read through a `Copy` host handle, which cannot
/// hand out the `&'graph Self::Graph` that [`Node`] requires.
///
/// Every [`Graph`] is a `Cfg` via the blanket impl below, so existing graph
/// consumers keep working unchanged.
pub trait Cfg {
    type NodeId: Copy + Eq + Hash + Ord + Debug;

    /// Hasher backing the analyses' node-keyed maps. Picking a fixed-seed hasher
    /// (e.g. [`FxBuildHasher`]) keeps [`successors`](Cfg::successors) iteration —
    /// and thus the derived dominator structures — deterministic across runs,
    /// whereas the std default (`RandomState`) reseeds per process.
    type Hasher: BuildHasher + Default;

    /// The successors of `n` (the targets of its outgoing edges).
    fn successors(&self, n: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_;
}

impl<G: Graph> Cfg for G {
    type NodeId = G::NodeId;
    type Hasher = G::Hasher;

    fn successors(&self, n: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_ {
        let succs: Vec<Self::NodeId> = self
            .get_node(n)
            .map(|nref| nref.children().map(|e| e.node_id()).collect())
            .unwrap_or_default();
        succs.into_iter()
    }
}

pub struct Directed;

pub struct Undirected;

/// Depth-first traversal iterator over graph nodes.
///
/// The iterator yields tuples of:
/// - the incoming tree edge used to discover the node (`None` for root),
/// - the discovered node reference.
///
/// `Mode` controls how neighbors are explored:
/// - [`Directed`]: follows only outgoing edges
/// - [`Undirected`]: treats all incident edges as traversable
pub struct DfsIter<'graph, Mode, G: Graph + Sized> {
    graph: &'graph G,
    visited: HashSet<G::NodeId, G::Hasher>,
    stack: Vec<(Option<G::EdgeId>, G::NodeId)>,
    _marker: std::marker::PhantomData<Mode>,
}

impl<'graph, G: Graph + Sized> Iterator for DfsIter<'graph, Directed, G> {
    type Item = (Option<G::Edge<'graph>>, G::Node<'graph>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((edge_id, node_id)) = self.stack.pop() {
            if !self.visited.insert(node_id) {
                continue;
            }

            let node_ref = self.graph.get_node(node_id).unwrap();

            for edge in node_ref.children() {
                let child_id = edge.node_id();
                if !self.visited.contains(&child_id) {
                    self.stack.push((Some(edge.edge_id()), child_id));
                }
            }

            return Some((edge_id.and_then(|id| self.graph.get_edge(id)), node_ref));
        }
        None
    }
}

impl<'graph, G: Graph + Sized> Iterator for DfsIter<'graph, Undirected, G> {
    type Item = (Option<G::Edge<'graph>>, G::Node<'graph>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((edge_id, node_id)) = self.stack.pop() {
            if !self.visited.insert(node_id) {
                continue;
            }

            let node_ref = self.graph.get_node(node_id).unwrap();

            for edge in node_ref.edges() {
                let child_id = edge.node_id();
                if !self.visited.contains(&child_id) {
                    self.stack.push((Some(edge.edge_id()), child_id));
                }
            }

            return Some((edge_id.and_then(|id| self.graph.get_edge(id)), node_ref));
        }
        None
    }
}
