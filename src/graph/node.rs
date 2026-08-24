//! Node views and traversal iterators for the graph.

use std::{collections::HashSet, marker::PhantomData};

use crate::graph::{Graph, edge::Edge};

pub trait Node<'graph> {
    type Graph: Graph;

    fn new(id: <Self::Graph as Graph>::NodeId, graph: &'graph Self::Graph) -> Self;

    /// Returns this node identifier.
    fn id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns the backing graph reference for this node.
    fn graph(&self) -> &'graph Self::Graph;

    /// Retrieve the ids of all edges incident to this node.
    fn edge_ids(
        &self,
    ) -> &'graph HashSet<<Self::Graph as Graph>::EdgeId, <Self::Graph as Graph>::Hasher>;

    /// Returns the number of incident edges for this node.
    fn edge_count(&self) -> usize;

    /// Returns `true` if this node has no connected edges.
    fn is_leaf(&self) -> bool {
        self.edge_count() == 0
    }

    /// Iterates over all incident edges.
    fn edges(&self) -> Iter<'graph, EdgeMode, Self::Graph> {
        Iter {
            graph: self.graph(),
            node: self.id(),
            iter: self.edge_ids().iter(),
            _mode: PhantomData,
        }
    }

    /// Iterates over child relationships for outgoing edges.
    fn children(&self) -> Iter<'graph, ChildMode, Self::Graph> {
        Iter {
            graph: self.graph(),
            node: self.id(),
            iter: self.edge_ids().iter(),
            _mode: PhantomData,
        }
    }

    /// Iterates over parent relationships for incoming edges.
    fn parents(&self) -> Iter<'graph, ParentMode, Self::Graph> {
        Iter {
            graph: self.graph(),
            node: self.id(),
            iter: self.edge_ids().iter(),
            _mode: PhantomData,
        }
    }
}

pub trait NodeMut<'graph> {
    type Graph: Graph;

    fn new(id: <Self::Graph as Graph>::NodeId, graph: &'graph mut Self::Graph) -> Self;

    /// Returns this node identifier.
    fn id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns the backing graph reference for this node.
    fn graph(&mut self) -> &mut Self::Graph;

    /// Retrieve the ids of all edges incident to this node.
    fn edge_ids(&self) -> &HashSet<<Self::Graph as Graph>::EdgeId, <Self::Graph as Graph>::Hasher>;

    /// Returns the number of incident edges for this node.
    fn edge_count(&self) -> usize;

    /// Registers an edge as incident to this node.
    fn add_edge_id(&mut self, edge: <Self::Graph as Graph>::EdgeId);

    /// Removes an edge from this node's incident edge set.
    fn remove_edge_id(&mut self, edge: <Self::Graph as Graph>::EdgeId);

    /// Returns a "weaker" immutable view of this mutable node handle.
    fn as_ref<'a>(&'a mut self) -> <Self::Graph as Graph>::Node<'a>
    where
        'graph: 'a,
    {
        <Self::Graph as Graph>::Node::new(self.id(), &*self.graph())
    }
}

/// One traversal item containing both edge and opposite-node information.
pub struct EdgeItem<'graph, G: Graph> {
    pub(crate) graph: &'graph G,
    pub(crate) edge: G::EdgeId,
    pub(crate) node: G::NodeId,
}

impl<'graph, G: Graph> EdgeItem<'graph, G> {
    /// Returns the traversed edge identifier.
    pub fn edge_id(&self) -> G::EdgeId {
        self.edge
    }

    /// Returns the opposite-node identifier for this traversal step.
    pub fn node_id(&self) -> G::NodeId {
        self.node
    }

    /// Returns the opposite node handle for this traversal step.
    pub fn node(&self) -> G::Node<'graph> {
        <G as Graph>::Node::new(self.node, self.graph)
    }

    /// Returns the traversed edge handle for this step.
    pub fn edge(&self) -> G::Edge<'graph> {
        <G as Graph>::Edge::new(self.edge, self.graph)
    }
}

/// Iterator mode selecting outgoing child traversal.
pub struct ChildMode;

/// Iterator mode selecting incoming parent traversal.
pub struct ParentMode;

/// Iterator mode selecting all incident edges.
pub struct EdgeMode;

/// Traversal iterator over node relationships.
pub struct Iter<'graph, Mode, G: Graph> {
    graph: &'graph G,
    node: G::NodeId,
    iter: std::collections::hash_set::Iter<'graph, G::EdgeId>,
    _mode: PhantomData<Mode>,
}

impl<'graph, G: Graph> Iterator for Iter<'graph, ChildMode, G> {
    type Item = EdgeItem<'graph, G>;

    fn next(&mut self) -> Option<Self::Item> {
        for &id in self.iter.by_ref() {
            let edge = <G as Graph>::Edge::new(id, self.graph);

            if self.node == edge.from_id() {
                return Some(EdgeItem {
                    graph: self.graph,
                    edge: id,
                    node: edge.to_id(),
                });
            }
        }

        None
    }
}

impl<'graph, G: Graph> Iterator for Iter<'graph, ParentMode, G> {
    type Item = EdgeItem<'graph, G>;

    fn next(&mut self) -> Option<Self::Item> {
        for id in self.iter.by_ref() {
            let edge = <G as Graph>::Edge::new(*id, self.graph);

            if self.node == edge.to_id() {
                return Some(EdgeItem {
                    graph: self.graph,
                    edge: *id,
                    node: edge.from_id(),
                });
            }
        }

        None
    }
}

impl<'graph, G: Graph> Iterator for Iter<'graph, EdgeMode, G> {
    type Item = EdgeItem<'graph, G>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|&id| {
            let edge = <G as Graph>::Edge::new(id, self.graph);
            let other_node = if self.node == edge.from_id() {
                edge.to_id()
            } else {
                edge.from_id()
            };

            EdgeItem {
                graph: self.graph,
                edge: id,
                node: other_node,
            }
        })
    }
}
