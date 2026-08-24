//! Post-dominator analysis for directed graphs.
//!
//! Definitions used here follow control-flow graph conventions:
//! - A node `p` post-dominates `n` if every path from `n` to any exit passes
//!   through `p`.
//! - Exit nodes post-dominate only themselves.
//!
//! This implementation computes post-dominance sets using a standard
//! iterative data-flow fixed point.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use std::fmt::Debug;
use std::hash::Hash;

use crate::graph::Cfg;

pub fn compute_postdominators<C: Cfg, S: BuildHasher + Default + Clone>(
    graph: &C,
    nodes: &[C::NodeId],
    node_set: &HashSet<C::NodeId, S>,
    exit_set: &HashSet<C::NodeId, S>,
) -> HashMap<C::NodeId, HashSet<C::NodeId, S>, S> {
    let all: HashSet<C::NodeId, S> = nodes.iter().copied().collect();
    let mut pdom: HashMap<C::NodeId, HashSet<C::NodeId, S>, S> = HashMap::default();

    for node in nodes {
        if exit_set.contains(node) {
            pdom.insert(*node, HashSet::from_iter([*node]));
        } else {
            pdom.insert(*node, all.clone());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;

        for node in nodes {
            if exit_set.contains(node) {
                continue;
            }

            let successors: Vec<C::NodeId> = graph
                .successors(*node)
                .filter(|child| node_set.contains(child))
                .collect();

            let mut updated = if successors.is_empty() {
                HashSet::default()
            } else {
                intersect_sets(successors.iter().map(|succ| &pdom[succ]))
            };

            updated.insert(*node);

            if updated != pdom[node] {
                pdom.insert(*node, updated);
                changed = true;
            }
        }
    }

    pdom
}

fn intersect_sets<
    'a,
    NodeId: Copy + Eq + Hash + Debug + 'a,
    S: BuildHasher + Default + Clone + 'a,
>(
    mut sets: impl Iterator<Item = &'a HashSet<NodeId, S>>,
) -> HashSet<NodeId, S> {
    let Some(first) = sets.next() else {
        return HashSet::default();
    };

    let mut acc = first.clone();
    for set in sets {
        acc.retain(|value| set.contains(value));
    }

    acc
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use jstd_derive::Identifier;

    use super::compute_postdominators;
    use crate::graph::{analysis::dominator::reachable_from_root, owning::OwningGraph};

    #[derive(Identifier)]
    struct NodeId(usize);

    #[derive(Identifier)]
    struct EdgeId(usize);

    type TestGraph = OwningGraph<NodeId, EdgeId, (), ()>;

    #[test]
    fn computes_postdominators_with_single_exit() {
        let mut graph = TestGraph::default();

        let (a, b, c, d, e) = {
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
            (a, b, c, d, e)
        };

        let mut nodes = reachable_from_root(&graph, a);
        nodes.sort_by_key(|id| Into::<usize>::into(*id));
        let node_set: HashSet<_> = nodes.iter().copied().collect();
        let exit_set = HashSet::from_iter([e]);

        let pdom = compute_postdominators(&graph, &nodes, &node_set, &exit_set);

        assert_eq!(pdom[&e], HashSet::from_iter([e]));
        assert!(pdom[&d].contains(&d) && pdom[&d].contains(&e));
        assert!(pdom[&a].contains(&a) && pdom[&a].contains(&d) && pdom[&a].contains(&e));
        assert!(!pdom[&a].contains(&b));
        assert!(!pdom[&a].contains(&c));
    }

    #[test]
    fn computes_postdominators_with_multiple_exits() {
        let mut graph = TestGraph::default();

        let (a, _b, c, d) = {
            let a = graph.make_node(());
            let b = graph.make_node(());
            let c = graph.make_node(());
            let d = graph.make_node(());

            graph.make_edge(a, b, ());
            graph.make_edge(a, c, ());
            graph.make_edge(b, d, ());
            (a, b, c, d)
        };

        let mut nodes = reachable_from_root(&graph, a);
        nodes.sort_by_key(|id| Into::<usize>::into(*id));
        let node_set: HashSet<_> = nodes.iter().copied().collect();
        let exit_set = HashSet::from_iter([c, d]);

        let pdom = compute_postdominators(&graph, &nodes, &node_set, &exit_set);

        assert_eq!(pdom[&c], HashSet::from_iter([c]));
        assert_eq!(pdom[&d], HashSet::from_iter([d]));
        assert_eq!(pdom[&a], HashSet::from_iter([a]));
    }
}
