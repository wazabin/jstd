//! Edge views for immutable and mutable graph access.
use crate::graph::{
    Graph, GraphMut,
    node::{Node, NodeMut},
};

/// Typed edge handle trait for immutable graph references.
pub trait Edge<'graph> {
    type Graph: Graph;

    fn new(id: <Self::Graph as Graph>::EdgeId, graph: &'graph Self::Graph) -> Self;

    /// Returns this edge identifier.
    fn id(&self) -> <Self::Graph as Graph>::EdgeId;

    /// Returns the backing graph reference for this edge.
    fn graph(&self) -> &'graph Self::Graph;

    /// Returns the source node's id for this edge.
    #[allow(clippy::wrong_self_convention)]
    fn from_id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns the destination node's id for this edge.
    fn to_id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns the source node for this edge
    fn from(&self) -> <Self::Graph as Graph>::Node<'graph> {
        let id = self.from_id();
        <Self::Graph as Graph>::Node::new(id, self.graph())
    }

    /// Returns the destination node for this edge
    fn to(&self) -> <Self::Graph as Graph>::Node<'graph> {
        let id = self.to_id();
        <Self::Graph as Graph>::Node::new(id, self.graph())
    }

    /// Returns `true` when this edge starts and ends at the same node.
    fn is_loop(&self) -> bool {
        self.from_id() == self.to_id()
    }
}

pub trait EdgeMut<'graph> {
    type Graph: GraphMut;

    fn new(id: <Self::Graph as Graph>::EdgeId, graph: &'graph mut Self::Graph) -> Self;

    /// Returns this edge identifier.
    fn id(&self) -> <Self::Graph as Graph>::EdgeId;

    /// Returns the backing graph reference for this edge.
    fn graph(&mut self) -> &mut Self::Graph;

    /// Returns the source node's id for this edge.
    #[allow(clippy::wrong_self_convention)]
    fn from_id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns mutable access to the source node for this edge.
    fn from(&mut self) -> <Self::Graph as GraphMut>::NodeMut<'_> {
        let id = self.from_id();
        <Self::Graph as GraphMut>::NodeMut::new(id, self.graph())
    }

    /// Returns the destination node's id for this edge.
    fn to_id(&self) -> <Self::Graph as Graph>::NodeId;

    /// Returns mutable access to the destination node for this edge.
    fn to(&mut self) -> <Self::Graph as GraphMut>::NodeMut<'_> {
        let id = self.to_id();
        <Self::Graph as GraphMut>::NodeMut::new(id, self.graph())
    }

    /// Redirects this edge to originate from `node` instead of its current source.
    fn set_from(&mut self, node: <Self::Graph as Graph>::NodeId);

    /// Returns an immutable view of this edge handle.
    fn as_ref(&mut self) -> <Self::Graph as Graph>::Edge<'_> {
        <Self::Graph as Graph>::Edge::new(self.id(), &*self.graph())
    }
}
