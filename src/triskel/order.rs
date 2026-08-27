//! Within-rank ordering via the Eiglsperger container method.
//!
//! Long edges live as segments inside *containers* — a contiguous run of
//! co-travelling segments between two vertices in a rank. p-vertices are
//! absorbed into the preceding container; q-vertices are extracted back out at
//! their rank. Crossings are counted treating a whole container as one block.
//! The current active-entry scan is not an end-to-end linear-time guarantee.
//!
//! Output: every vertex's `order`, the per-rank materialised-vertex layers, the
//! per-rank slot sequence (vertices and segment lanes, left to right) used by
//! the coordinate phase, and the set of segments crossed by a normal edge
//! (type-1 conflicts) used by Brandes–Köpf.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    graph::{Graph, GraphMut, edge::Edge, node::Node},
    triskel::{
        layout::{LayoutGraph, layer_of},
        segment::SegmentInfo,
    },
};

/// One left-to-right element of a rank: a real/dummy vertex, or a segment lane
/// passing through (identified by its q-vertex id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    Vertex(usize),
    Segment(usize),
}

pub(crate) struct Ordering {
    pub layers: Vec<Vec<usize>>,
    pub slots: Vec<Vec<Slot>>,
}

#[derive(Clone, Debug)]
enum Item {
    Vertex(usize),
    Container(Vec<usize>, usize),
}

impl Item {
    fn size(&self) -> usize {
        match self {
            Item::Vertex(_) => 1,
            Item::Container(c, _) => c.len(),
        }
    }
}

#[derive(Clone, Default)]
struct Layer {
    items: Vec<Item>,
}

impl Layer {
    /// Ensure vertex/container alternation: starts and ends with a container and
    /// never has two consecutive vertices or containers.
    fn normalize(&mut self) {
        let mut out: Vec<Item> = vec![Item::Container(Vec::new(), 0)];
        for item in self.items.drain(..) {
            match (out.last_mut().unwrap(), item) {
                (Item::Container(lhs, _), Item::Container(rhs, _)) => lhs.extend(rhs),
                (Item::Vertex(_), Item::Vertex(id)) => {
                    out.push(Item::Container(Vec::new(), 0));
                    out.push(Item::Vertex(id));
                }
                (_, item) => out.push(item),
            }
        }
        if !matches!(out.last(), Some(Item::Container(_, _))) {
            out.push(Item::Container(Vec::new(), 0));
        }
        self.items = out;
    }
}

struct Context<'g> {
    graph: &'g mut LayoutGraph,
    pvertices: HashSet<usize>,
    qvertices: HashSet<usize>,
    wishes: HashMap<usize, f64>,
    reversed: bool,
}

impl<'g> Context<'g> {
    fn is_p(&self, id: usize) -> bool {
        self.pvertices.contains(&id)
    }
    fn is_q(&self, id: usize) -> bool {
        self.qvertices.contains(&id)
    }

    /// The q-vertex on the far end of a p-vertex's segment.
    fn q_of(&self, id: usize) -> usize {
        let node = self.graph.get_node(id).unwrap();
        if self.reversed {
            node.parents().next().unwrap().node_id()
        } else {
            node.children().next().unwrap().node_id()
        }
    }

    fn order(&mut self, layering: &mut [Vec<usize>], rounds: usize) {
        let mut best_cross = usize::MAX;
        let mut best_order: HashMap<usize, usize> = HashMap::default();

        for _ in 0..rounds {
            let cross = self.round(layering);
            if cross < best_cross {
                best_cross = cross;
                best_order = self.graph.nodes().map(|n| (n.id(), n.order)).collect();
            }
            layering.reverse();
            self.reversed = !self.reversed;
            std::mem::swap(&mut self.pvertices, &mut self.qvertices);
        }

        for (id, order) in best_order {
            self.graph.get_node_mut(id).unwrap().order = order;
        }
    }

    fn round(&mut self, layering: &mut [Vec<usize>]) -> usize {
        let mut crossings = 0;
        let mut layer = Layer::default();
        if let Some(first) = layering.first_mut() {
            first.sort_by_key(|id| self.graph.get_node(*id).unwrap().order);
            for &id in first.iter() {
                layer.items.push(Item::Vertex(id));
            }
            layer.normalize();
        }

        for child in layering.iter().skip(1) {
            self.absorb_p(&mut layer);
            self.assign_container_positions(&mut layer);
            self.compute_wishes(child);
            layer = self.merge(layer, child);
            self.extract_q(&mut layer, child);
            layer.normalize();
            crossings += self.count_crossings(&layer);
        }
        self.assign_container_positions(&mut layer);
        crossings
    }

    /// step1: convert each p-vertex into a segment in the preceding container.
    fn absorb_p(&self, layer: &mut Layer) {
        let mut out: Vec<Item> = Vec::with_capacity(layer.items.len());
        for item in &layer.items {
            match item {
                Item::Container(c, parent) => {
                    if let Some(Item::Container(prev, _)) = out.last_mut() {
                        prev.extend(c.iter().copied());
                    } else {
                        out.push(Item::Container(c.clone(), *parent));
                    }
                }
                &Item::Vertex(id) => {
                    if self.is_p(id) {
                        let Some(Item::Container(c, _)) = out.last_mut() else {
                            panic!("expected a container before a p-vertex");
                        };
                        c.push(self.q_of(id));
                    } else {
                        out.push(Item::Vertex(id));
                    }
                }
            }
        }
        layer.items = out;
    }

    /// step2: assign a running position to every item; write vertex orders.
    fn assign_container_positions(&mut self, layer: &mut Layer) {
        let mut pos = 0;
        for item in &mut layer.items {
            match item {
                Item::Container(_, parent) => *parent = pos,
                Item::Vertex(id) => self.graph.get_node_mut(*id).unwrap().order = pos,
            }
            pos += item.size();
        }
    }

    /// step2': desired position (median of neighbour positions) per child vertex.
    fn compute_wishes(&mut self, child: &[usize]) {
        for &id in child {
            if self.is_q(id) {
                continue;
            }
            let node = self.graph.get_node(id).unwrap();
            let mut neighbors: Vec<usize> = if self.reversed {
                node.children()
                    .map(|c| self.graph.get_node(c.node_id()).unwrap().order)
                    .collect()
            } else {
                node.parents()
                    .map(|p| self.graph.get_node(p.node_id()).unwrap().order)
                    .collect()
            };
            neighbors.sort_unstable();
            let wish = if neighbors.is_empty() {
                node.order as f64
            } else if neighbors.len() % 2 == 0 {
                let m = neighbors.len() / 2;
                (neighbors[m - 1] + neighbors[m]) as f64 / 2.0
            } else {
                neighbors[neighbors.len() / 2] as f64
            };
            self.wishes.insert(id, wish);
        }
    }

    /// step3: merge child vertices into the parent's containers by position.
    fn merge(&self, mut parent: Layer, child: &[usize]) -> Layer {
        let mut nodes: Vec<usize> = child.iter().copied().filter(|id| !self.is_q(*id)).collect();
        nodes.sort_by(|a, b| {
            let pa = self.wishes.get(a).copied().unwrap_or(0.0);
            let pb = self.wishes.get(b).copied().unwrap_or(0.0);
            pa.total_cmp(&pb).then_with(|| a.cmp(b))
        });

        let mut set_idx = 0;
        let mut node_idx = 0;
        let mut res = Layer::default();

        loop {
            let node_pos = nodes
                .get(node_idx)
                .map(|id| (*id, self.wishes.get(id).copied().unwrap_or(0.0)));
            let set_pos = if let Some(Item::Container(set, pos)) = parent.items.get(set_idx) {
                Some((set.clone(), *pos as f64))
            } else {
                None
            };

            match (set_pos, node_pos) {
                (Some((set, _)), _) if set.is_empty() => set_idx += 2,
                (Some((_, spos)), Some((node, npos))) if npos <= spos => {
                    res.items.push(Item::Vertex(node));
                    node_idx += 1;
                }
                (None, Some((node, _))) => {
                    res.items.push(Item::Vertex(node));
                    node_idx += 1;
                }
                (Some((set, spos)), Some((_, npos))) if npos >= spos + (set.len() - 1) as f64 => {
                    res.items.push(Item::Container(set, spos as usize));
                    set_idx += 2;
                }
                (Some((set, spos)), None) => {
                    res.items.push(Item::Container(set, spos as usize));
                    set_idx += 2;
                }
                (Some((set, spos)), Some((node, npos))) => {
                    let k = (npos - spos) as usize;
                    let (left, right) = (set[..k].to_vec(), set[k..].to_vec());
                    res.items.push(Item::Container(left, spos as usize));
                    res.items.push(Item::Vertex(node));
                    node_idx += 1;
                    parent.items[set_idx] = Item::Container(right, spos as usize + k);
                }
                (None, None) => break,
            }
        }
        res
    }

    /// step4: pull each q-vertex out of the container holding its segment.
    fn extract_q(&self, layer: &mut Layer, nodes: &[usize]) {
        for &id in nodes {
            if !self.is_q(id) {
                continue;
            }
            let mut split = None;
            for (idx, item) in layer.items.iter().enumerate() {
                if let Item::Container(c, spos) = item
                    && let Some(at) = c.iter().position(|s| *s == id)
                {
                    let left = c[..at].to_vec();
                    let right = c[at + 1..].to_vec();
                    split = Some((idx, *spos, left, right));
                    break;
                }
            }
            let Some((idx, spos, left, right)) = split else {
                panic!("q-vertex {id} segment not found in any container");
            };
            let left_size = left.len();
            layer.items[idx] = Item::Container(left, spos);
            layer.items.insert(idx + 1, Item::Vertex(id));
            layer
                .items
                .insert(idx + 2, Item::Container(right, spos + left_size + 1));
        }
    }

    /// step5: count crossings, treating each container as a block of `size`
    /// parallel segments.
    fn count_crossings(&self, layer: &Layer) -> usize {
        let mut crossings = 0;
        // (position, weight)
        let mut active: Vec<(usize, usize)> = Vec::new();

        for item in &layer.items {
            match item {
                Item::Vertex(node) => {
                    // In an upward sweep, `layer` is above the previously
                    // processed rank, so its relevant neighbours are children,
                    // not parents.
                    let mut neighbors: Vec<usize> = if self.reversed {
                        self.graph
                            .get_node(*node)
                            .unwrap()
                            .children()
                            .map(|c| self.graph.get_node(c.node_id()).unwrap().order)
                            .collect()
                    } else {
                        self.graph
                            .get_node(*node)
                            .unwrap()
                            .parents()
                            .map(|p| self.graph.get_node(p.node_id()).unwrap().order)
                            .collect()
                    };
                    neighbors.sort_unstable();
                    for parent_pos in neighbors {
                        for &(pos, weight) in &active {
                            if pos > parent_pos {
                                crossings += weight;
                            }
                        }
                        active.push((parent_pos, 1));
                    }
                }
                Item::Container(c, _) if c.is_empty() => {}
                Item::Container(c, pos) => {
                    let size = c.len();
                    crossings += active
                        .iter()
                        .filter(|&&(p, _)| p > *pos)
                        .map(|&(_, w)| w)
                        .sum::<usize>()
                        * size;
                    active.push((*pos, size));
                }
            }
        }
        crossings
    }
}

pub(crate) fn order(
    graph: &mut LayoutGraph,
    seg: &SegmentInfo,
    root: usize,
    max_sweeps: usize,
) -> Ordering {
    let max_rank = graph
        .nodes()
        .map(|n| layer_of(graph, n.id()))
        .max()
        .unwrap_or(0);
    let mut layering: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    let mut ids: Vec<usize> = graph.nodes().map(|n| n.id()).collect();
    ids.sort_unstable();
    for &id in &ids {
        layering[layer_of(graph, id)].push(id);
    }

    seed_order(graph, root, &ids, &mut layering);

    {
        let mut ctx = Context {
            graph: &mut *graph,
            pvertices: seg.pvertices.clone(),
            qvertices: seg.qvertices.clone(),
            wishes: HashMap::default(),
            reversed: false,
        };
        ctx.order(&mut layering, max_sweeps.max(1));
    }

    // Rebuild deterministic top-down layers + slots from the sweep result,
    // then refine slots against both boundaries of every affected rank.
    let layers = build_layers(graph, max_rank);
    let mut slots = build_slots(graph, seg, &layers);
    transpose_slots(graph, seg, &mut slots, false);
    let layers = layers_from_slots(graph, &slots);
    debug_assert_slot_contract(graph, seg, &layers, &slots);

    Ordering { layers, slots }
}

/// Total adjacent-boundary crossing objective for materialised slots.  A long
/// p→q segment contributes one connection at every boundary it spans, so this
/// is the geometric objective affected by swapping a slot in an interior rank.
fn slot_crossings(graph: &LayoutGraph, seg: &SegmentInfo, slots: &[Vec<Slot>]) -> usize {
    let mut pos: Vec<HashMap<Slot, usize>> = vec![HashMap::default(); slots.len()];
    for (rank, row) in slots.iter().enumerate() {
        for (index, &slot) in row.iter().enumerate() {
            pos[rank].insert(slot, index);
        }
    }
    let mut by_boundary: Vec<Vec<(usize, usize)>> = vec![Vec::new(); slots.len().saturating_sub(1)];
    let mut edge_ids: Vec<_> = graph.edges().map(|edge| edge.id()).collect();
    edge_ids.sort_unstable();
    for eid in edge_ids {
        let edge = graph.get_edge(eid).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        let from_rank = layer_of(graph, from);
        let to_rank = layer_of(graph, to);
        if to_rank == from_rank + 1 {
            by_boundary[from_rank].push((
                pos[from_rank][&Slot::Vertex(from)],
                pos[to_rank][&Slot::Vertex(to)],
            ));
        }
    }
    // The only non-adjacent internal edges are p→q segment edges.  Materialise
    // their boundary crossings without materialising graph vertices.
    let mut qids: Vec<_> = seg.qvertices.iter().copied().collect();
    qids.sort_unstable();
    for q in qids {
        let p = graph
            .get_node(q)
            .unwrap()
            .parents()
            .next()
            .unwrap()
            .node_id();
        let start = layer_of(graph, p);
        let end = layer_of(graph, q);
        for rank in start..end {
            let left = if rank == start {
                Slot::Vertex(p)
            } else {
                Slot::Segment(q)
            };
            let right = if rank + 1 == end {
                Slot::Vertex(q)
            } else {
                Slot::Segment(q)
            };
            by_boundary[rank].push((pos[rank][&left], pos[rank + 1][&right]));
        }
    }
    by_boundary
        .iter()
        .map(|edges| {
            edges
                .iter()
                .enumerate()
                .map(|(i, &(a0, b0))| {
                    edges[i + 1..]
                        .iter()
                        .filter(|&&(a1, b1)| (a0 < a1 && b0 > b1) || (a0 > a1 && b0 < b1))
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

/// Checks the hard equality/order system before coordinate assignment. Equality
/// groups are p/lane/q chains; every adjacent slot creates a strictly-positive
/// left→right difference constraint. A directed cycle is therefore exactly a
/// mandatory-coordinate infeasibility.
fn mandatory_slot_constraints_feasible(
    graph: &LayoutGraph,
    seg: &SegmentInfo,
    slots: &[Vec<Slot>],
) -> bool {
    let mut cells = HashMap::default();
    let mut n = 0usize;
    for (rank, row) in slots.iter().enumerate() {
        for &slot in row {
            cells.insert((rank, slot), n);
            n += 1;
        }
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }
    let mut unite = |a: usize, b: usize| {
        let a = find(&mut parent, a);
        let b = find(&mut parent, b);
        if a != b {
            parent[b] = a;
        }
    };
    let mut qids: Vec<_> = seg.qvertices.iter().copied().collect();
    qids.sort_unstable();
    for q in qids {
        let p = graph
            .get_node(q)
            .unwrap()
            .parents()
            .next()
            .unwrap()
            .node_id();
        let start = layer_of(graph, p);
        let end = layer_of(graph, q);
        let first = cells[&(start, Slot::Vertex(p))];
        for rank in start + 1..end {
            unite(first, cells[&(rank, Slot::Segment(q))]);
        }
        unite(first, cells[&(end, Slot::Vertex(q))]);
    }
    let groups: Vec<_> = (0..n).map(|i| find(&mut parent, i)).collect();
    let mut outgoing = vec![Vec::new(); n];
    let mut indegree = vec![0usize; n];
    for (rank, row) in slots.iter().enumerate() {
        for pair in row.windows(2) {
            let a = groups[cells[&(rank, pair[0])]];
            let b = groups[cells[&(rank, pair[1])]];
            if a == b {
                return false;
            }
            if !outgoing[a].contains(&b) {
                outgoing[a].push(b);
                indegree[b] += 1;
            }
        }
    }
    let mut ready: Vec<_> = (0..n)
        .filter(|&i| i == groups[i] && indegree[i] == 0)
        .collect();
    let mut visited = 0;
    while let Some(i) = ready.pop() {
        visited += 1;
        for &next in &outgoing[i] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(next);
            }
        }
    }
    visited == (0..n).filter(|&i| i == groups[i]).count()
}

/// Deterministic local-swap pass. Each candidate is scored against **all** rank
/// boundaries, not merely the boundary last visited by the sweep.  A swap that
/// would make p/lane/q equality incompatible with slot separation is rejected.
fn transpose_slots(graph: &LayoutGraph, seg: &SegmentInfo, slots: &mut [Vec<Slot>], reverse: bool) {
    loop {
        let before = slot_crossings(graph, seg, slots);
        let ranks: Vec<_> = if reverse {
            (0..slots.len()).rev().collect()
        } else {
            (0..slots.len()).collect()
        };
        let mut accepted = false;
        'scan: for rank in ranks {
            let indices: Vec<_> = if reverse {
                (0..slots[rank].len().saturating_sub(1)).rev().collect()
            } else {
                (0..slots[rank].len().saturating_sub(1)).collect()
            };
            for index in indices {
                slots[rank].swap(index, index + 1);
                let feasible = mandatory_slot_constraints_feasible(graph, seg, slots);
                let after = feasible.then(|| slot_crossings(graph, seg, slots));
                if after.is_some_and(|score| score < before) {
                    accepted = true;
                    break 'scan;
                }
                slots[rank].swap(index, index + 1);
            }
        }
        if !accepted {
            break;
        }
    }
}

/// Synchronises vertex order with the final slot order.  Slot positions (rather
/// than stale `NodeLayoutData::order`) are the authority after transpose.
fn layers_from_slots(graph: &mut LayoutGraph, slots: &[Vec<Slot>]) -> Vec<Vec<usize>> {
    let mut layers = vec![Vec::new(); slots.len()];
    for (rank, row) in slots.iter().enumerate() {
        for (order, slot) in row.iter().enumerate() {
            if let Slot::Vertex(id) = *slot {
                graph.get_node_mut(id).unwrap().order = order;
                layers[rank].push(id);
            }
        }
    }
    layers
}

/// Phase boundary contract: materialised vertices occur once at their own rank;
/// each p/q segment occupies exactly its intermediate ranks and nowhere else.
fn debug_assert_slot_contract(
    graph: &LayoutGraph,
    seg: &SegmentInfo,
    layers: &[Vec<usize>],
    slots: &[Vec<Slot>],
) {
    debug_assert_eq!(layers.len(), slots.len());
    let mut vertices = HashSet::default();
    let mut lanes: HashMap<usize, Vec<usize>> = HashMap::default();
    for (rank, row) in slots.iter().enumerate() {
        for slot in row {
            match *slot {
                Slot::Vertex(id) => {
                    debug_assert_eq!(layer_of(graph, id), rank);
                    debug_assert!(vertices.insert(id), "vertex {id} appears in multiple slots");
                }
                Slot::Segment(q) => lanes.entry(q).or_default().push(rank),
            }
        }
    }
    let expected_vertices: HashSet<usize> = graph.nodes().map(|node| node.id()).collect();
    debug_assert_eq!(vertices, expected_vertices, "slot vertex set is incomplete");

    for &q in &seg.qvertices {
        let p = graph
            .get_node(q)
            .unwrap()
            .parents()
            .next()
            .unwrap()
            .node_id();
        let expected: Vec<usize> = ((layer_of(graph, p) + 1)..layer_of(graph, q)).collect();
        debug_assert_eq!(
            lanes.remove(&q).unwrap_or_default(),
            expected,
            "invalid lane interval for q={q}"
        );
    }
    debug_assert!(lanes.is_empty(), "slot contains an unknown segment");
}

/// Seed each rank's order from a DFS preorder so crossing reduction starts from
/// a sensible, deterministic embedding.
fn seed_order(graph: &mut LayoutGraph, root: usize, ids: &[usize], layering: &mut [Vec<usize>]) {
    let cap = ids.iter().copied().max().unwrap_or(0) + 1;
    let mut pre = vec![usize::MAX; cap];
    let mut counter = 0usize;
    let mut seen = vec![false; cap];
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node >= cap || seen[node] {
            continue;
        }
        seen[node] = true;
        pre[node] = counter;
        counter += 1;
        let mut children: Vec<usize> = graph
            .get_node(node)
            .unwrap()
            .children()
            .map(|c| c.node_id())
            .collect();
        children.sort_unstable_by(|a, b| b.cmp(a));
        for c in children {
            if !seen[c] {
                stack.push(c);
            }
        }
    }
    for &id in ids {
        if pre[id] == usize::MAX {
            pre[id] = counter;
            counter += 1;
        }
    }
    for layer in layering.iter_mut() {
        layer.sort_by_key(|id| (pre[*id], *id));
        for (i, &id) in layer.iter().enumerate() {
            graph.get_node_mut(id).unwrap().order = i;
        }
    }
}

fn build_layers(graph: &LayoutGraph, max_rank: usize) -> Vec<Vec<usize>> {
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    let mut ids: Vec<usize> = graph.nodes().map(|n| n.id()).collect();
    ids.sort_unstable();
    for &id in &ids {
        layers[layer_of(graph, id)].push(id);
    }
    for layer in &mut layers {
        layer.sort_by_key(|id| (graph.get_node(*id).unwrap().order, *id));
    }
    layers
}

/// Deterministic top-down reconstruction of per-rank slot sequences (vertices +
/// segment lanes) from the final orders, mirroring the absorb/merge/extract
/// dance without crossing counting.
fn build_slots(
    graph: &mut LayoutGraph,
    seg: &SegmentInfo,
    layers: &[Vec<usize>],
) -> Vec<Vec<Slot>> {
    let mut ctx = Context {
        graph,
        pvertices: seg.pvertices.clone(),
        qvertices: seg.qvertices.clone(),
        wishes: HashMap::default(),
        reversed: false,
    };

    let mut slots: Vec<Vec<Slot>> = Vec::with_capacity(layers.len());
    let mut layer = Layer::default();
    if let Some(first) = layers.first() {
        for &id in first {
            layer.items.push(Item::Vertex(id));
        }
        layer.normalize();
    }
    slots.push(flatten(&layer));

    for child in layers.iter().skip(1) {
        ctx.absorb_p(&mut layer);
        ctx.assign_container_positions(&mut layer);
        for &id in child {
            let order = ctx.graph.get_node(id).unwrap().order;
            ctx.wishes.insert(id, order as f64);
        }
        layer = ctx.merge(layer, child);
        ctx.extract_q(&mut layer, child);
        layer.normalize();
        slots.push(flatten(&layer));
    }

    slots
}

fn flatten(layer: &Layer) -> Vec<Slot> {
    let mut out = Vec::new();
    for item in &layer.items {
        match item {
            Item::Vertex(id) => out.push(Slot::Vertex(*id)),
            Item::Container(c, _) => {
                for &seg in c {
                    out.push(Slot::Segment(seg));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triskel::{
        layout::{EdgeLayoutData, NodeLayoutData},
        rank, segment,
    };

    fn context(graph: &mut LayoutGraph, reversed: bool) -> Context<'_> {
        Context {
            graph,
            pvertices: HashSet::default(),
            qvertices: HashSet::default(),
            wishes: HashMap::default(),
            reversed,
        }
    }

    #[test]
    fn crossing_score_uses_sweep_direction() {
        // a,b (top) → c,d (bottom), with a→d and b→c crossing once.
        let mut graph = LayoutGraph::default();
        let a = graph.make_node(NodeLayoutData {
            rank: 0,
            order: 0,
            ..Default::default()
        });
        let b = graph.make_node(NodeLayoutData {
            rank: 0,
            order: 1,
            ..Default::default()
        });
        let c = graph.make_node(NodeLayoutData {
            rank: 1,
            order: 0,
            ..Default::default()
        });
        let d = graph.make_node(NodeLayoutData {
            rank: 1,
            order: 1,
            ..Default::default()
        });
        graph.make_edge(a, d, EdgeLayoutData::default());
        graph.make_edge(b, c, EdgeLayoutData::default());

        let bottom = Layer {
            items: vec![Item::Vertex(c), Item::Vertex(d)],
        };
        assert_eq!(context(&mut graph, false).count_crossings(&bottom), 1);

        // The same geometric crossing viewed upward must score identically.
        let top = Layer {
            items: vec![Item::Vertex(a), Item::Vertex(b)],
        };
        assert_eq!(context(&mut graph, true).count_crossings(&top), 1);
    }

    #[test]
    fn transpose_strictly_improves_an_adjacent_crossing() {
        let mut graph = LayoutGraph::default();
        let a = graph.make_node(NodeLayoutData {
            rank: 0,
            order: 0,
            ..Default::default()
        });
        let b = graph.make_node(NodeLayoutData {
            rank: 0,
            order: 1,
            ..Default::default()
        });
        let c = graph.make_node(NodeLayoutData {
            rank: 1,
            order: 0,
            ..Default::default()
        });
        let d = graph.make_node(NodeLayoutData {
            rank: 1,
            order: 1,
            ..Default::default()
        });
        graph.make_edge(a, d, EdgeLayoutData::default());
        graph.make_edge(b, c, EdgeLayoutData::default());
        let seg = SegmentInfo {
            pvertices: HashSet::default(),
            qvertices: HashSet::default(),
        };
        let mut slots = vec![
            vec![Slot::Vertex(a), Slot::Vertex(b)],
            vec![Slot::Vertex(c), Slot::Vertex(d)],
        ];
        assert_eq!(slot_crossings(&graph, &seg, &slots), 1);
        transpose_slots(&graph, &seg, &mut slots, false);
        assert_eq!(slot_crossings(&graph, &seg, &slots), 0);
        assert!(
            slots[0] == vec![Slot::Vertex(b), Slot::Vertex(a)]
                || slots[1] == vec![Slot::Vertex(d), Slot::Vertex(c)],
            "one endpoint rank must make the improving deterministic swap: {slots:?}"
        );
    }

    #[test]
    fn crossing_score_matches_bruteforce_for_two_by_two_bipartite_graphs() {
        let permutations = [[0usize, 1], [1, 0]];
        for mask in 0u8..16 {
            for top_order in permutations {
                for bottom_order in permutations {
                    let mut graph = LayoutGraph::default();
                    let top: Vec<_> = (0..2)
                        .map(|_| graph.make_node(NodeLayoutData::default()))
                        .collect();
                    let bottom: Vec<_> = (0..2)
                        .map(|_| graph.make_node(NodeLayoutData::default()))
                        .collect();
                    let mut top_pos = [0usize; 2];
                    let mut bottom_pos = [0usize; 2];
                    for (position, index) in top_order.into_iter().enumerate() {
                        top_pos[index] = position;
                        graph.get_node_mut(top[index]).unwrap().order = position;
                    }
                    for (position, index) in bottom_order.into_iter().enumerate() {
                        bottom_pos[index] = position;
                        graph.get_node_mut(bottom[index]).unwrap().order = position;
                    }
                    let mut edges = Vec::new();
                    for (source, &source_id) in top.iter().enumerate() {
                        for (target, &target_id) in bottom.iter().enumerate() {
                            if mask & (1 << (source * 2 + target)) != 0 {
                                graph.make_edge(source_id, target_id, EdgeLayoutData::default());
                                edges.push((source, target));
                            }
                        }
                    }
                    let expected = edges
                        .iter()
                        .enumerate()
                        .map(|(i, &(a_source, a_target))| {
                            edges[i + 1..]
                                .iter()
                                .filter(|&&(b_source, b_target)| {
                                    (top_pos[a_source] < top_pos[b_source]
                                        && bottom_pos[a_target] > bottom_pos[b_target])
                                        || (top_pos[a_source] > top_pos[b_source]
                                            && bottom_pos[a_target] < bottom_pos[b_target])
                                })
                                .count()
                        })
                        .sum::<usize>();
                    let lower = Layer {
                        items: bottom_order
                            .into_iter()
                            .map(|i| Item::Vertex(bottom[i]))
                            .collect(),
                    };
                    let upper = Layer {
                        items: top_order
                            .into_iter()
                            .map(|i| Item::Vertex(top[i]))
                            .collect(),
                    };
                    assert_eq!(
                        context(&mut graph, false).count_crossings(&lower),
                        expected,
                        "downward mask={mask:04b}, top={top_order:?}, bottom={bottom_order:?}"
                    );
                    assert_eq!(
                        context(&mut graph, true).count_crossings(&upper),
                        expected,
                        "upward mask={mask:04b}, top={top_order:?}, bottom={bottom_order:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn transpose_scores_both_boundaries_and_is_direction_symmetric() {
        // Swapping c/d fixes the top a→d, b→c crossing but creates the bottom
        // c→e, d→f crossing. A one-sided transpose wrongly accepts it; the
        // all-boundary objective is unchanged and must reject it.
        let mut graph = LayoutGraph::default();
        let ids: Vec<_> = (0..6)
            .map(|i| {
                graph.make_node(NodeLayoutData {
                    rank: (i / 2) as i64,
                    ..Default::default()
                })
            })
            .collect();
        graph.make_edge(ids[0], ids[3], EdgeLayoutData::default());
        graph.make_edge(ids[1], ids[2], EdgeLayoutData::default());
        graph.make_edge(ids[2], ids[4], EdgeLayoutData::default());
        graph.make_edge(ids[3], ids[5], EdgeLayoutData::default());
        let seg = SegmentInfo {
            pvertices: HashSet::default(),
            qvertices: HashSet::default(),
        };
        let initial = vec![
            vec![Slot::Vertex(ids[0]), Slot::Vertex(ids[1])],
            vec![Slot::Vertex(ids[2]), Slot::Vertex(ids[3])],
            vec![Slot::Vertex(ids[4]), Slot::Vertex(ids[5])],
        ];
        let before = slot_crossings(&graph, &seg, &initial);
        assert_eq!(before, 1);
        let mut forward = initial.clone();
        let mut reverse = initial.clone();
        transpose_slots(&graph, &seg, &mut forward, false);
        transpose_slots(&graph, &seg, &mut reverse, true);
        assert!(slot_crossings(&graph, &seg, &forward) <= before);
        assert!(slot_crossings(&graph, &seg, &reverse) <= before);
        // Outer ranks may independently improve, but the middle tie must not
        // be accepted merely because one adjacent boundary improves.
        assert_eq!(forward[1], initial[1]);
        assert_eq!(reverse[1], initial[1]);
    }

    #[test]
    fn transpose_handles_first_and_last_rank_and_never_worsens() {
        let mut graph = LayoutGraph::default();
        let top: Vec<_> = (0..2)
            .map(|_| {
                graph.make_node(NodeLayoutData {
                    rank: 0,
                    ..Default::default()
                })
            })
            .collect();
        let bottom: Vec<_> = (0..2)
            .map(|_| {
                graph.make_node(NodeLayoutData {
                    rank: 1,
                    ..Default::default()
                })
            })
            .collect();
        graph.make_edge(top[0], bottom[1], EdgeLayoutData::default());
        graph.make_edge(top[1], bottom[0], EdgeLayoutData::default());
        let seg = SegmentInfo {
            pvertices: HashSet::default(),
            qvertices: HashSet::default(),
        };
        let mut slots = vec![
            vec![Slot::Vertex(top[0]), Slot::Vertex(top[1])],
            vec![Slot::Vertex(bottom[0]), Slot::Vertex(bottom[1])],
        ];
        let before = slot_crossings(&graph, &seg, &slots);
        transpose_slots(&graph, &seg, &mut slots, false);
        assert!(slot_crossings(&graph, &seg, &slots) <= before);
        assert_eq!(slot_crossings(&graph, &seg, &slots), 0);
    }

    #[test]
    fn slots_contain_each_vertex_and_each_active_segment_lane_once() {
        let mut graph = LayoutGraph::default();
        let nodes: Vec<_> = (0..5)
            .map(|_| graph.make_node(NodeLayoutData::default()))
            .collect();
        for pair in nodes.windows(2) {
            graph.make_edge(pair[0], pair[1], EdgeLayoutData::default());
        }
        graph.make_edge(nodes[0], nodes[4], EdgeLayoutData::default());
        rank::assign_ranks(&mut graph);
        let segments = segment::build_segments(&mut graph);
        let ordering = order(&mut graph, &segments, nodes[0], 4);

        debug_assert_slot_contract(&graph, &segments, &ordering.layers, &ordering.slots);
        let q = *segments.qvertices.iter().next().unwrap();
        let lane_ranks: Vec<_> = ordering
            .slots
            .iter()
            .enumerate()
            .filter_map(|(rank, row)| {
                row.iter()
                    .any(|slot| matches!(slot, Slot::Segment(id) if *id == q))
                    .then_some(rank)
            })
            .collect();
        assert_eq!(lane_ranks, vec![2]);
    }
}
