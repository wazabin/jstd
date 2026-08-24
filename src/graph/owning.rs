use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::RandomState,
    ops::{Deref, DerefMut},
};

use crate::{
    graph::{
        Graph, GraphMut,
        edge::{Edge, EdgeMut},
        node::{Node, NodeMut},
    },
    registry::Identifier,
};

#[derive(Default, Clone)]
pub(crate) struct RawNode<EdgeId: Identifier> {
    pub edges: HashSet<EdgeId>,
}

#[derive(Clone)]
pub(crate) struct RawEdge<NodeId: Identifier> {
    pub from: NodeId,
    pub to: NodeId,
}

impl<NodeId: Identifier + Debug> Debug for RawEdge<NodeId> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawEdge")
            .field("from", &self.from)
            .field("to", &self.to)
            .finish()
    }
}

pub struct NodeRef<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> {
    pub id: NodeId,
    graph: &'graph OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData>
    NodeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    fn inner(&self) -> &'graph RawNode<EdgeId> {
        &self.graph.nodes[&self.id]
    }

    pub fn data(&self) -> &'graph NodeData {
        &self.graph.node_data[&self.id]
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Node<'graph>
    for NodeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    type Graph = OwningGraph<NodeId, EdgeId, NodeData, EdgeData>;

    fn new(id: NodeId, graph: &'graph Self::Graph) -> Self {
        NodeRef { id, graph }
    }

    fn id(&self) -> NodeId {
        self.id
    }

    fn graph(&self) -> &'graph Self::Graph {
        self.graph
    }

    fn edge_ids(&self) -> &'graph HashSet<EdgeId, RandomState> {
        &self.inner().edges
    }

    fn edge_count(&self) -> usize {
        self.inner().edges.len()
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Deref
    for NodeRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    type Target = NodeData;

    fn deref(&self) -> &Self::Target {
        self.data()
    }
}

pub struct NodeMutRef<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> {
    pub id: NodeId,
    graph: &'graph mut OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData>
    NodeMutRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    pub fn data(&mut self) -> &mut NodeData {
        self.graph.node_data.get_mut(&self.id).unwrap()
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> NodeMut<'graph>
    for NodeMutRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    type Graph = OwningGraph<NodeId, EdgeId, NodeData, EdgeData>;

    fn new(id: NodeId, graph: &'graph mut Self::Graph) -> Self {
        NodeMutRef { id, graph }
    }

    fn id(&self) -> NodeId {
        self.id
    }

    fn graph(&mut self) -> &mut Self::Graph {
        self.graph
    }

    fn edge_ids(&self) -> &HashSet<EdgeId, RandomState> {
        &self.graph.nodes.get(&self.id).unwrap().edges
    }

    fn edge_count(&self) -> usize {
        self.graph.nodes.get(&self.id).unwrap().edges.len()
    }

    fn add_edge_id(&mut self, edge: EdgeId) {
        self.graph
            .nodes
            .get_mut(&self.id)
            .unwrap()
            .edges
            .insert(edge);
    }

    fn remove_edge_id(&mut self, edge: EdgeId) {
        self.graph
            .nodes
            .get_mut(&self.id)
            .unwrap()
            .edges
            .remove(&edge);
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Deref
    for NodeMutRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    type Target = NodeData;

    fn deref(&self) -> &Self::Target {
        self.graph.node_data.get(&self.id).unwrap()
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> DerefMut
    for NodeMutRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.graph.node_data.get_mut(&self.id).unwrap()
    }
}

pub struct EdgeRef<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> {
    pub id: EdgeId,
    graph: &'graph OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
}

impl<'graph, EdgeId: Identifier + Debug, NodeId: Identifier + Debug, NodeData, EdgeData> Debug
    for EdgeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Edge")
            .field("id", &self.id)
            .field("raw", self.inner())
            .finish()
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData>
    EdgeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    fn inner(&self) -> &RawEdge<NodeId> {
        &self.graph.edges[&self.id]
    }

    pub fn data(&self) -> &EdgeData {
        &self.graph.edge_data[&self.id]
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Edge<'graph>
    for EdgeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    type Graph = OwningGraph<NodeId, EdgeId, NodeData, EdgeData>;

    fn new(id: EdgeId, graph: &'graph Self::Graph) -> Self {
        EdgeRef { id, graph }
    }

    fn id(&self) -> EdgeId {
        self.id
    }

    fn graph(&self) -> &'graph Self::Graph {
        self.graph
    }

    fn from_id(&self) -> NodeId {
        self.inner().from
    }

    fn to_id(&self) -> NodeId {
        self.inner().to
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Deref
    for EdgeRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    type Target = EdgeData;

    fn deref(&self) -> &Self::Target {
        self.data()
    }
}

pub struct EdgeMutRef<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> {
    pub id: EdgeId,
    graph: &'graph mut OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData>
    EdgeMutRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    pub fn data(&mut self) -> &mut EdgeData {
        self.graph.edge_data.get_mut(&self.id).unwrap()
    }
}

impl<'graph, NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> EdgeMut<'graph>
    for EdgeMutRef<'graph, NodeId, EdgeId, NodeData, EdgeData>
{
    type Graph = OwningGraph<NodeId, EdgeId, NodeData, EdgeData>;

    fn new(id: <Self::Graph as Graph>::EdgeId, graph: &'graph mut Self::Graph) -> Self {
        EdgeMutRef { id, graph }
    }

    fn id(&self) -> <Self::Graph as Graph>::EdgeId {
        self.id
    }

    fn graph(&mut self) -> &mut Self::Graph {
        self.graph
    }

    fn from_id(&self) -> <Self::Graph as Graph>::NodeId {
        self.graph.get_edge(self.id).unwrap().from_id()
    }

    fn to_id(&self) -> <Self::Graph as Graph>::NodeId {
        self.graph.get_edge(self.id).unwrap().to_id()
    }

    fn set_from(&mut self, node: <Self::Graph as Graph>::NodeId) {
        self.graph.edges.get_mut(&self.id).unwrap().from = node;
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Deref
    for EdgeMutRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    type Target = EdgeData;

    fn deref(&self) -> &Self::Target {
        &self.graph.edge_data[&self.id]
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> DerefMut
    for EdgeMutRef<'_, NodeId, EdgeId, NodeData, EdgeData>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.graph.edge_data.get_mut(&self.id).unwrap()
    }
}

/// An example Graph implementation with `usize` identifiers and simple payloads, used in tests.
pub struct OwningGraph<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> {
    node_data: HashMap<NodeId, NodeData>,
    edge_data: HashMap<EdgeId, EdgeData>,

    nodes: HashMap<NodeId, RawNode<EdgeId>>,
    edges: HashMap<EdgeId, RawEdge<NodeId>>,

    next_node_id: usize,
    next_edge_id: usize,
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Default
    for OwningGraph<NodeId, EdgeId, NodeData, EdgeData>
{
    fn default() -> Self {
        Self {
            node_data: HashMap::new(),
            edge_data: HashMap::new(),
            nodes: HashMap::new(),
            edges: HashMap::new(),
            next_node_id: 0,
            next_edge_id: 0,
        }
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> Graph
    for OwningGraph<NodeId, EdgeId, NodeData, EdgeData>
{
    type NodeId = NodeId;
    type EdgeId = EdgeId;

    // The generic demo/test graph keeps the std default hasher; consumers that
    // need deterministic edge iteration (e.g. qcode's `Context`) pick a
    // fixed-seed hasher via their own `Graph::Hasher`.
    type Hasher = RandomState;

    type Node<'a>
        = NodeRef<'a, NodeId, EdgeId, NodeData, EdgeData>
    where
        Self: 'a;

    type Edge<'a>
        = EdgeRef<'a, NodeId, EdgeId, NodeData, EdgeData>
    where
        Self: 'a;

    fn get_node(&self, id: Self::NodeId) -> Option<Self::Node<'_>> {
        if self.node_data.contains_key(&id) {
            Some(NodeRef { id, graph: self })
        } else {
            None
        }
    }

    fn get_edge(&self, id: Self::EdgeId) -> Option<Self::Edge<'_>> {
        if self.edge_data.contains_key(&id) {
            Some(EdgeRef { id, graph: self })
        } else {
            None
        }
    }

    fn nodes(&self) -> impl Iterator<Item = Self::Node<'_>> + '_ {
        self.node_data.keys().map(|id| self.get_node(*id).unwrap())
    }

    fn edges(&self) -> impl Iterator<Item = Self::Edge<'_>> + '_ {
        self.edge_data.keys().map(|id| self.get_edge(*id).unwrap())
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData> GraphMut
    for OwningGraph<NodeId, EdgeId, NodeData, EdgeData>
{
    type NodeMut<'a>
        = NodeMutRef<'a, NodeId, EdgeId, NodeData, EdgeData>
    where
        Self: 'a;

    type EdgeMut<'a>
        = EdgeMutRef<'a, NodeId, EdgeId, NodeData, EdgeData>
    where
        Self: 'a;

    fn get_node_mut(&mut self, id: Self::NodeId) -> Option<Self::NodeMut<'_>> {
        if self.node_data.contains_key(&id) {
            Some(NodeMutRef { id, graph: self })
        } else {
            None
        }
    }

    fn get_edge_mut(&mut self, id: Self::EdgeId) -> Option<Self::EdgeMut<'_>> {
        if self.edge_data.contains_key(&id) {
            Some(EdgeMutRef { id, graph: self })
        } else {
            None
        }
    }
}

impl<NodeId: Identifier, EdgeId: Identifier, NodeData, EdgeData>
    OwningGraph<NodeId, EdgeId, NodeData, EdgeData>
{
    /// Creates a new node
    /// Creates a new node with payload `data` and returns its identifier.
    pub fn make_node(&mut self, data: NodeData) -> NodeId {
        let id = NodeId::from(self.next_node_id);
        self.next_node_id += 1;
        self.nodes.insert(
            id,
            RawNode {
                edges: HashSet::new(),
            },
        );
        self.node_data.insert(id, data);
        id
    }

    /// Creates a new edge from `from` to `to` with payload `data`.
    ///
    /// # Panics
    /// Panics if either endpoint does not exist.
    pub fn make_edge(&mut self, from: NodeId, to: NodeId, data: EdgeData) -> EdgeId {
        let id = EdgeId::from(self.next_edge_id);
        self.next_edge_id += 1;
        self.edges.insert(id, RawEdge { from, to });
        self.edge_data.insert(id, data);
        self.nodes.get_mut(&from).unwrap().edges.insert(id);
        self.nodes.get_mut(&to).unwrap().edges.insert(id);
        id
    }

    /// Removes an edge by identifier.
    ///
    /// # Panics
    /// Panics if the edge does not exist.
    pub fn remove_edge(&mut self, id: EdgeId) {
        let edge = self.edges.remove(&id).unwrap();
        self.edge_data.remove(&id);

        self.nodes.get_mut(&edge.from).unwrap().edges.remove(&id);

        self.nodes.get_mut(&edge.to).unwrap().edges.remove(&id);
    }

    /// Edits an edge's endpoints
    pub fn edit_edge(&mut self, id: EdgeId, new_from: NodeId, new_to: NodeId) {
        let edge = self.edges.get_mut(&id).unwrap();

        // Remove from old endpoints
        self.nodes.get_mut(&edge.from).unwrap().edges.remove(&id);
        self.nodes.get_mut(&edge.to).unwrap().edges.remove(&id);

        // Update edge endpoints
        edge.from = new_from;
        edge.to = new_to;

        // Add to new endpoints
        self.nodes.get_mut(&new_from).unwrap().edges.insert(id);
        self.nodes.get_mut(&new_to).unwrap().edges.insert(id);
    }

    /// Removes a node and all of its incident edges.
    ///
    /// # Panics
    /// Panics if the node does not exist.
    pub fn remove_node(&mut self, id: NodeId) {
        // Remove edges while the node is still present: `remove_edge` updates
        // both endpoint incident-edge sets, including this node's.
        let edges: Vec<EdgeId> = self.nodes[&id].edges.iter().copied().collect();
        for edge_id in edges {
            self.remove_edge(edge_id);
        }
        self.nodes.remove(&id).unwrap();
        self.node_data.remove(&id);
    }

    /// Returns an iterator over the immediate successor blocks —
    /// blocks that this block may transfer control to.
    pub fn reinterpret<NewNodeData, NewEdgeData, Fn, Fe>(
        &self,
        mut node_data_map: Fn,
        mut edge_data_map: Fe,
    ) -> OwningGraph<NodeId, EdgeId, NewNodeData, NewEdgeData>
    where
        Fn: FnMut(&NodeData) -> NewNodeData,
        Fe: FnMut(&EdgeData) -> NewEdgeData,
    {
        OwningGraph {
            node_data: self
                .node_data
                .iter()
                .map(|(id, data)| (*id, node_data_map(data)))
                .collect(),
            edge_data: self
                .edge_data
                .iter()
                .map(|(id, data)| (*id, edge_data_map(data)))
                .collect(),
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            next_node_id: self.next_node_id,
            next_edge_id: self.next_edge_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{
        graph::{
            Graph, GraphMut,
            edge::{Edge, EdgeMut},
            node::{Node, NodeMut},
            owning::OwningGraph,
        },
        registry::Identifier,
    };

    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct TestNodeId(usize);

    impl From<usize> for TestNodeId {
        fn from(value: usize) -> Self {
            Self(value)
        }
    }

    impl From<TestNodeId> for usize {
        fn from(value: TestNodeId) -> Self {
            value.0
        }
    }

    impl Identifier for TestNodeId {}

    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct TestEdgeId(usize);

    impl From<usize> for TestEdgeId {
        fn from(value: usize) -> Self {
            Self(value)
        }
    }

    impl From<TestEdgeId> for usize {
        fn from(value: TestEdgeId) -> Self {
            value.0
        }
    }

    impl Identifier for TestEdgeId {}

    type TestGraph = OwningGraph<TestNodeId, TestEdgeId, &'static str, i32>;

    #[test]
    fn editor_creates_nodes_edges_and_root() {
        let mut graph = TestGraph::default();

        let (root_id, child_id, edge_id) = {
            let root_id = graph.make_node("root");
            let child_id = graph.make_node("child");
            let edge_id = graph.make_edge(root_id, child_id, 7);
            (root_id, child_id, edge_id)
        };

        let root = graph.get_node(root_id).unwrap();
        assert_eq!(root.id(), root_id);
        assert_eq!(*root.data(), "root");

        let child = graph.get_node(child_id).unwrap();
        assert_eq!(*child.data(), "child");
        assert!(child.parents().any(|item| item.node_id() == root_id));

        let edge = graph.get_edge(edge_id).unwrap();
        assert_eq!(edge.id(), edge_id);
        assert_eq!(edge.from().id(), root_id);
        assert_eq!(edge.to().id(), child_id);
        assert_eq!(*edge.data(), 7);
        assert!(!edge.is_loop());
    }

    #[test]
    fn node_and_edge_iterators_cover_all_items() {
        let mut graph = TestGraph::default();

        {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            graph.make_edge(a, b, 1);
            graph.make_edge(b, c, 2);
        }

        let node_values: HashSet<_> = graph.nodes().map(|node| *node.data()).collect();
        let edge_values: HashSet<_> = graph.edges().map(|edge| *edge.data()).collect();

        assert_eq!(node_values, HashSet::from(["a", "b", "c"]));
        assert_eq!(edge_values, HashSet::from([1, 2]));
    }

    #[test]
    fn mutable_handles_can_update_payloads() {
        let mut graph = TestGraph::default();

        let (node_id, edge_id) = {
            let n0 = graph.make_node("initial");
            let n1 = graph.make_node("other");
            let e0 = graph.make_edge(n0, n1, 10);
            (n0, e0)
        };

        *graph.get_node_mut(node_id).unwrap().data() = "updated";
        *graph.get_edge_mut(edge_id).unwrap().data() = 99;

        assert_eq!(*graph.get_node(node_id).unwrap().data(), "updated");
        assert_eq!(*graph.get_edge(edge_id).unwrap().data(), 99);
    }

    #[test]
    fn remove_edge_updates_connected_nodes() {
        let mut graph = TestGraph::default();

        let (a, b, edge_id) = {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let edge_id = graph.make_edge(a, b, 3);
            (a, b, edge_id)
        };

        assert_eq!(graph.get_node(a).unwrap().edge_count(), 1);
        assert_eq!(graph.get_node(b).unwrap().edge_count(), 1);

        graph.remove_edge(edge_id);

        assert_eq!(graph.get_node(a).unwrap().edge_count(), 0);
        assert_eq!(graph.get_node(b).unwrap().edge_count(), 0);
        assert_eq!(graph.edges().count(), 0);
    }

    #[test]
    fn make_edge_does_not_overwrite_existing_after_remove() {
        let mut graph = TestGraph::default();

        let (a, b, c, d) = {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            let d = graph.make_node("d");
            (a, b, c, d)
        };

        let e0 = graph.make_edge(a, b, 10);
        let _e1 = graph.make_edge(b, c, 20);

        graph.remove_edge(e0);

        let _e2 = graph.make_edge(c, d, 30);

        let edge_values: HashSet<_> = graph.edges().map(|edge| *edge.data()).collect();
        assert_eq!(edge_values, HashSet::from([20, 30]));
        assert_eq!(graph.edges().count(), 2);
    }

    #[test]
    fn merge_nodes_rehomes_outgoing_edges_to_keep() {
        // A -e0-> B -e1-> C
        // merge_nodes(keep=A, remove=B, direct=e0)
        // Expected: A -e1-> C, B has no edges, e0 is gone from both
        let mut graph = TestGraph::default();

        let (a, b, c) = {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            (a, b, c)
        };
        let e0 = graph.make_edge(a, b, 1);
        let e1 = graph.make_edge(b, c, 2);

        graph.merge_nodes(a, b, e0);

        // e0 removed from A and B
        assert!(!graph.get_node(a).unwrap().edge_ids().contains(&e0));
        assert!(!graph.get_node(b).unwrap().edge_ids().contains(&e0));

        // e1 rehomed: now originates from A
        assert_eq!(graph.get_edge(e1).unwrap().from_id(), a);
        assert!(graph.get_node(a).unwrap().edge_ids().contains(&e1));
        assert!(!graph.get_node(b).unwrap().edge_ids().contains(&e1));

        // B has no incident edges
        assert_eq!(graph.get_node(b).unwrap().edge_count(), 0);

        // A has exactly one outgoing edge (e1) to C
        let a_children: Vec<_> = graph.get_node(a).unwrap().children().collect();
        assert_eq!(a_children.len(), 1);
        assert_eq!(a_children[0].node_id(), c);
        assert_eq!(a_children[0].edge_id(), e1);
    }

    #[test]
    fn merge_nodes_handles_multiple_outgoing_edges_on_removed_node() {
        // A -e0-> B, B -e1-> C, B -e2-> D
        // merge_nodes(keep=A, remove=B, direct=e0)
        // Expected: A -e1-> C, A -e2-> D, B has no edges
        let mut graph = TestGraph::default();

        let (a, b, c, d) = {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            let d = graph.make_node("d");
            (a, b, c, d)
        };
        let e0 = graph.make_edge(a, b, 10);
        let e1 = graph.make_edge(b, c, 20);
        let e2 = graph.make_edge(b, d, 30);

        graph.merge_nodes(a, b, e0);

        assert_eq!(graph.get_node(b).unwrap().edge_count(), 0);

        let a_successors: HashSet<_> = graph
            .get_node(a)
            .unwrap()
            .children()
            .map(|item| item.node_id())
            .collect();
        assert_eq!(a_successors, HashSet::from([c, d]));

        assert_eq!(graph.get_edge(e1).unwrap().from_id(), a);
        assert_eq!(graph.get_edge(e2).unwrap().from_id(), a);
    }

    #[test]
    fn merge_nodes_on_tail_node_leaves_keep_with_no_outgoing_edges() {
        // A -e0-> B (B has no outgoing edges — it is a tail)
        // merge_nodes(keep=A, remove=B, direct=e0)
        // Expected: A has no outgoing edges, B has no incident edges
        let mut graph = TestGraph::default();

        let (a, b) = {
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            (a, b)
        };
        let e0 = graph.make_edge(a, b, 1);

        graph.merge_nodes(a, b, e0);

        assert_eq!(graph.get_node(a).unwrap().children().count(), 0);
        assert_eq!(graph.get_node(b).unwrap().edge_count(), 0);
    }

    #[test]
    fn merge_nodes_does_not_touch_incoming_edges_of_keep() {
        // P -ep-> A -e0-> B -e1-> C
        // merge_nodes(keep=A, remove=B, direct=e0)
        // Expected: P -ep-> A -e1-> C (ep still points to A)
        let mut graph = TestGraph::default();

        let (p, a, b, c) = {
            let p = graph.make_node("p");
            let a = graph.make_node("a");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            (p, a, b, c)
        };
        let ep = graph.make_edge(p, a, 0);
        let e0 = graph.make_edge(a, b, 1);
        graph.make_edge(b, c, 2);

        graph.merge_nodes(a, b, e0);

        // ep unchanged: still from P to A
        assert_eq!(graph.get_edge(ep).unwrap().from_id(), p);
        assert_eq!(graph.get_edge(ep).unwrap().to_id(), a);
        assert!(graph.get_node(a).unwrap().edge_ids().contains(&ep));

        // A now has ep (incoming) and e1 (outgoing)
        assert_eq!(graph.get_node(a).unwrap().edge_count(), 2);
    }

    #[test]
    fn directed_dfs_visits_reachable_nodes() {
        let mut graph = TestGraph::default();

        let (root, b, c, isolated) = {
            let root = graph.make_node("root");
            let b = graph.make_node("b");
            let c = graph.make_node("c");
            let isolated = graph.make_node("isolated");

            graph.make_edge(root, b, 1);
            graph.make_edge(b, c, 2);

            (root, b, c, isolated)
        };

        let mut seen = HashSet::new();
        let mut root_count = 0;

        for (edge, node) in graph.dfs(root) {
            if node.id() == root {
                root_count += 1;
                assert!(edge.is_none());
            }
            seen.insert(node.id());
        }

        assert_eq!(root_count, 1);
        assert!(seen.contains(&root));
        assert!(seen.contains(&b));
        assert!(seen.contains(&c));
        assert!(!seen.contains(&isolated));
    }

    #[test]
    fn undirected_dfs_can_reach_incoming_neighbors() {
        let mut graph = TestGraph::default();

        let (root, parent) = {
            let root = graph.make_node("root");
            let parent = graph.make_node("parent");

            graph.make_edge(parent, root, 5);

            (root, parent)
        };

        let directed_seen: HashSet<_> = graph.dfs(root).map(|(_, node)| node.id()).collect();

        let undirected_seen: HashSet<_> = graph
            .undirected_dfs(root)
            .map(|(_, node)| node.id())
            .collect();

        assert!(directed_seen.contains(&root));
        assert!(!directed_seen.contains(&parent));
        assert!(undirected_seen.contains(&root));
        assert!(undirected_seen.contains(&parent));
    }

    #[test]
    fn trait_views_traverse_and_mutate_edges_without_losing_connectivity() {
        let mut graph = TestGraph::default();
        let a = graph.make_node("a");
        let b = graph.make_node("b");
        let loop_node = graph.make_node("loop");
        let edge = graph.make_edge(a, b, 1);
        let loop_edge = graph.make_edge(loop_node, loop_node, 2);

        let a_ref = graph.get_node(a).unwrap();
        assert!(!a_ref.is_leaf());
        assert_eq!(a_ref.edges().count(), 1);
        let step = a_ref.children().next().unwrap();
        assert_eq!(step.edge().id(), edge);
        assert_eq!(step.node().id(), b);
        assert!(graph.get_edge(loop_edge).unwrap().is_loop());

        {
            let mut a_mut = graph.get_node_mut(a).unwrap();
            assert_eq!(a_mut.edge_count(), 1);
            a_mut.remove_edge_id(edge);
            assert!(a_mut.as_ref().is_leaf());
            a_mut.add_edge_id(edge);
            assert_eq!(a_mut.as_ref().edge_count(), 1);
        }

        {
            let mut edge_mut = graph.get_edge_mut(edge).unwrap();
            assert_eq!(edge_mut.as_ref().to_id(), b);
            assert_eq!(edge_mut.from().id(), a);
            assert_eq!(edge_mut.to().id(), b);
            *edge_mut.data() = 9;
            edge_mut.set_from(a);
        }
        assert_eq!(*graph.get_edge(edge).unwrap().data(), 9);
    }

    #[test]
    fn editing_reinterpreting_and_removing_nodes_preserves_graph_invariants() {
        let mut graph = TestGraph::default();
        let a = graph.make_node("a");
        let b = graph.make_node("b");
        let c = graph.make_node("c");
        let edge = graph.make_edge(a, b, 4);

        graph.edit_edge(edge, b, c);
        let edited = graph.get_edge(edge).unwrap();
        assert_eq!(edited.from_id(), b);
        assert_eq!(edited.to_id(), c);
        assert_eq!(graph.get_node(a).unwrap().edge_count(), 0);
        assert_eq!(graph.get_node(b).unwrap().edge_count(), 1);
        assert_eq!(graph.get_node(c).unwrap().edge_count(), 1);

        let reinterpreted = graph.reinterpret(|node| node.len(), |edge| edge * 10);
        assert_eq!(*reinterpreted.get_node(a).unwrap().data(), 1);
        assert_eq!(*reinterpreted.get_edge(edge).unwrap().data(), 40);

        graph.remove_node(b);
        assert!(graph.get_node(b).is_none());
        assert!(graph.get_edge(edge).is_none());
        assert_eq!(graph.get_node(c).unwrap().edge_count(), 0);
    }
}
