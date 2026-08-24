use std::{error::Error, fmt};

use rustc_hash::FxHashMap as HashMap;

use crate::{
    graph::{Graph, GraphMut, edge::Edge, node::Node, owning::OwningGraph},
    registry::Identifier,
};

pub type TestGraph<'a> = OwningGraph<usize, usize, &'a str, &'a str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseErrorKind {
    InvalidNodeDeclaration,
    InvalidEdgeDeclaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError<'src> {
    pub line_number: usize,
    pub line: &'src str,
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ParseErrorKind::InvalidNodeDeclaration => {
                write!(f, "invalid node declaration on line {}", self.line_number)
            }
            ParseErrorKind::InvalidEdgeDeclaration => {
                write!(f, "invalid edge declaration on line {}", self.line_number)
            }
        }
    }
}

impl Error for ParseError<'_> {}

impl<'src, NodeId: Identifier, EdgeId: Identifier>
    OwningGraph<NodeId, EdgeId, &'src str, &'src str>
{
    pub fn parse(source: &'src str) -> Result<Self, ParseError<'src>> {
        let mut graph = Self::default();
        let mut node_ids: HashMap<&'src str, NodeId> = HashMap::default();

        for (line_number, line) in source.lines().enumerate() {
            let line_number = line_number + 1;
            let trimmed = line.trim();

            if trimmed.is_empty() {
                continue;
            }

            if let Some((from_raw, to_raw)) = trimmed.split_once("->") {
                let from_name = parse_identifier(from_raw).map_err(|_| ParseError {
                    line_number,
                    line,
                    kind: ParseErrorKind::InvalidEdgeDeclaration,
                })?;

                let (to_name, explicit_edge_data) =
                    parse_declaration(to_raw).map_err(|_| ParseError {
                        line_number,
                        line,
                        kind: ParseErrorKind::InvalidEdgeDeclaration,
                    })?;

                let from_id = ensure_node(&mut graph, &mut node_ids, from_name, None);
                let to_id = ensure_node(&mut graph, &mut node_ids, to_name, None);
                let edge_data = explicit_edge_data.unwrap_or(trimmed);
                graph.make_edge(from_id, to_id, edge_data);

                continue;
            }

            let (name, explicit_node_data) =
                parse_declaration(trimmed).map_err(|_| ParseError {
                    line_number,
                    line,
                    kind: ParseErrorKind::InvalidNodeDeclaration,
                })?;

            ensure_node(&mut graph, &mut node_ids, name, explicit_node_data);
        }

        Ok(graph)
    }

    pub fn get_node_by_name(&self, name: &str) -> Option<NodeId> {
        self.nodes().find_map(|node| {
            if *node.data() == name {
                Some(node.id())
            } else {
                None
            }
        })
    }

    pub fn get_edge_by_data(&self, data: &str) -> Option<EdgeId> {
        self.edges().find_map(|edge| {
            if *edge.data() == data {
                Some(edge.id())
            } else {
                None
            }
        })
    }
}

fn ensure_node<'src, NodeId: Identifier, EdgeId: Identifier>(
    graph: &mut OwningGraph<NodeId, EdgeId, &'src str, &'src str>,
    node_ids: &mut HashMap<&'src str, NodeId>,
    name: &'src str,
    explicit_data: Option<&'src str>,
) -> NodeId {
    if let Some(&id) = node_ids.get(name) {
        if let Some(data) = explicit_data {
            *graph.get_node_mut(id).unwrap().data() = data;
        }

        return id;
    }

    let data = explicit_data.unwrap_or(name);
    let id = graph.make_node(data);
    node_ids.insert(name, id);
    id
}

fn parse_identifier(input: &str) -> Result<&str, ()> {
    let (name, payload) = parse_declaration(input)?;
    if payload.is_some() {
        return Err(());
    }

    Ok(name)
}

fn parse_declaration(input: &str) -> Result<(&str, Option<&str>), ()> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(());
    }

    let split_index = trimmed
        .find(|c: char| c.is_whitespace() || c == '[')
        .unwrap_or(trimmed.len());

    if split_index == 0 {
        return Err(());
    }

    let name = &trimmed[..split_index];
    let trailing = trimmed[split_index..].trim();

    if trailing.is_empty() {
        return Ok((name, None));
    }

    if !trailing.starts_with('[') || !trailing.ends_with(']') {
        return Err(());
    }

    let payload = trailing[1..trailing.len() - 1].trim();
    if payload.is_empty() {
        return Err(());
    }

    Ok((name, Some(payload)))
}

#[cfg(test)]
mod tests {
    use crate::graph::{Graph, TestGraph};

    #[test]
    fn parse_supports_implicit_and_explicit_node_declarations() {
        let source = "v0\nv1 [Hello World]\nv0 -> v1\nv1 -> v2 [Hello, World]";
        let graph = TestGraph::parse(source).expect("parse should succeed");

        assert_eq!(graph.nodes().count(), 3);
        assert_eq!(graph.edges().count(), 2);

        assert_eq!(*graph.get_node(0).unwrap().data(), "v0");
        assert_eq!(*graph.get_node(1).unwrap().data(), "Hello World");
        assert_eq!(*graph.get_node(2).unwrap().data(), "v2");

        assert_eq!(*graph.get_edge(0).unwrap().data(), "v0 -> v1");
        assert_eq!(*graph.get_edge(1).unwrap().data(), "Hello, World");
    }

    #[test]
    fn parse_declares_missing_nodes_from_edge_declarations() {
        let source = "a -> b";
        let graph = TestGraph::parse(source).expect("parse should succeed");

        assert_eq!(graph.nodes().count(), 2);
        assert_eq!(*graph.get_node(0).unwrap().data(), "a");
        assert_eq!(*graph.get_node(1).unwrap().data(), "b");
        assert_eq!(graph.edges().count(), 1);
    }

    #[test]
    fn parse_updates_existing_node_data_on_explicit_redeclaration() {
        let source = "a -> b\nb [Bee]";
        let graph = TestGraph::parse(source).expect("parse should succeed");

        assert_eq!(graph.nodes().count(), 2);
        assert_eq!(*graph.get_node(1).unwrap().data(), "Bee");
    }

    #[test]
    fn parse_reports_invalid_declarations() {
        let node_error = match TestGraph::parse("v0 [") {
            Ok(_) => panic!("node declaration should fail"),
            Err(error) => error,
        };
        assert_eq!(node_error.line_number, 1);

        let edge_error = match TestGraph::parse("v0 -> [label]") {
            Ok(_) => panic!("edge declaration should fail"),
            Err(error) => error,
        };
        assert_eq!(edge_error.line_number, 1);
    }
}
