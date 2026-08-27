//! X-coordinate assignment via width-aware Brandes–Köpf over the container slot
//! model.
//!
//! The ordering pass produced, per rank, the left-to-right sequence of slots
//! (real/dummy vertices and segment lanes). We materialise those as transient
//! *cells* — segment lanes become zero-width dummy cells linked vertically into
//! one straight run between their p- and q-vertices — and run the four
//! Brandes–Köpf alignment passes (up/down × left/right), averaging the results.
//! Each pass keeps the separation `x[right] − x[left] ≥ (w_l+w_r)/2 + node_gap`
//! for adjacent slots, so the average is collision-free at any node width, and a
//! segment's p/lanes/q share one x (a straight vertical run). This is enforced
//! as a hard equality constraint after the four directional passes; it is not
//! left to the optional Brandes–Köpf alignment heuristic.
//!
//! **Edge-offset drift.** Edges don't attach at node centres — the router fans
//! a node's edges across its bottom/top face, so an edge leaves its source at
//! `x + start_port` and arrives at its target at `x + end_port`. Aligning node
//! *centres* would then leave a kink where the (offset) edge endpoint meets the
//! (centred) dummy column. To avoid that we carry a per-cell `drift`: when a
//! cell aligns to a neighbour, drift accumulates the link's
//! `offset_at_neighbour − offset_at_self`, and the final x is `x[root] + drift`.
//! That shifts each cell within its block so the *edge endpoints* of the aligned
//! chain line up vertically. The port offsets are recomputed here via the
//! router's [`assign_ports`], which is deterministic in node width + order and
//! therefore matches the offsets the router applies afterwards.
//!
//! Cells are transient arrays freed after this phase. The graph itself stays at
//! O(1) objects per long edge, while these per-rank cells necessarily use memory
//! proportional to the total active segment span.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    graph::{Graph, GraphMut, edge::Edge, node::Node},
    triskel::{
        layout::{LayoutGraph, layer_of, loop_reserve},
        order::{Ordering, Slot},
        router::assign_ports,
        segment::SegmentInfo,
    },
};

enum Entity {
    Vertex(usize),
    Segment,
}

struct Cells {
    rank: Vec<usize>,
    pos: Vec<usize>,
    width: Vec<f64>,
    is_dummy: Vec<bool>,
    entity: Vec<Entity>,
    layers: Vec<Vec<usize>>,
    parents: Vec<Vec<usize>>,
    children: Vec<Vec<usize>>,
    /// Per parent→child link, the connecting edge's `(start_port, end_port)`
    /// offsets — parallel to `parents`/`children` respectively. Real-node ends
    /// carry the router's fan-out offset; dummy ends are 0 (centred).
    parent_off: Vec<Vec<(f64, f64)>>,
    child_off: Vec<Vec<(f64, f64)>>,
    /// Complete p → lanes → q chains. These are coordinate constraints, not
    /// optional alignment candidates.
    segment_chains: Vec<Vec<usize>>,
}

pub(crate) fn assign_x(
    graph: &mut LayoutGraph,
    seg: &SegmentInfo,
    ordering: &Ordering,
    node_gap: f64,
) {
    let cells = build_cells(graph, seg, ordering);
    if cells.rank.is_empty() {
        return;
    }

    let marked = mark_type1(&cells);
    let mut sum = vec![0.0f64; cells.rank.len()];
    for &down in &[true, false] {
        for &left in &[true, false] {
            let xs = directional_pass(&cells, &marked, node_gap, down, left);
            for (i, x) in xs.into_iter().enumerate() {
                sum[i] += x;
            }
        }
    }

    let mut xs: Vec<f64> = sum.into_iter().map(|x| x / 4.0).collect();
    enforce_segment_constraints(&cells, node_gap, &mut xs);

    for (cid, entity) in cells.entity.iter().enumerate() {
        if let Entity::Vertex(id) = entity {
            graph.get_node_mut(*id).unwrap().x = xs[cid];
        }
    }
}

/// Materialises the per-rank slot sequences into a transient cell graph: one
/// cell per slot, segment lanes linked vertically into straight runs, real
/// adjacencies linked rank-to-rank.
fn build_cells(graph: &LayoutGraph, seg: &SegmentInfo, ordering: &Ordering) -> Cells {
    let slots = &ordering.slots;
    let mut rank = Vec::new();
    let mut pos = Vec::new();
    let mut width = Vec::new();
    let mut is_dummy = Vec::new();
    let mut entity = Vec::new();
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); slots.len()];

    let mut vertex_cell: HashMap<usize, usize> = HashMap::default();
    let mut seg_cell: HashMap<(usize, usize), usize> = HashMap::default();

    for (r, row) in slots.iter().enumerate() {
        for (i, slot) in row.iter().enumerate() {
            let cid = rank.len();
            rank.push(r);
            pos.push(i);
            match *slot {
                Slot::Vertex(id) => {
                    let node = graph.get_node(id).unwrap();
                    // Reserve the self-loop margin symmetrically so the loops on
                    // the right face clear the neighbour (the spare left margin
                    // is harmless) — the node centre and rendered box are
                    // unchanged.
                    width.push(node.width + 2.0 * loop_reserve(node.self_loops));
                    is_dummy.push(node.is_dummy);
                    entity.push(Entity::Vertex(id));
                    vertex_cell.insert(id, cid);
                }
                Slot::Segment(qid) => {
                    width.push(0.0);
                    is_dummy.push(true);
                    entity.push(Entity::Segment);
                    seg_cell.insert((r, qid), cid);
                }
            }
            layers[r].push(cid);
        }
    }

    let n = rank.len();
    let mut parents = vec![Vec::new(); n];
    let mut children = vec![Vec::new(); n];
    let mut parent_off: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n];
    let mut child_off: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n];
    let mut link = |p: usize, c: usize, off: (f64, f64)| {
        children[p].push(c);
        child_off[p].push(off);
        parents[c].push(p);
        parent_off[c].push(off);
    };

    // Router fan-out offsets, keyed by internal edge id. Recomputed here so the
    // alignment can line up edge endpoints; the router derives the identical
    // offsets later (same widths + orders).
    let ports = assign_ports(graph);
    let off_of = |eid: usize| {
        (
            ports.start.get(&eid).copied().unwrap_or(0.0),
            ports.end.get(&eid).copied().unwrap_or(0.0),
        )
    };

    // Rule 1: adjacent-rank edges, skipping the spanning p → q segment edges.
    let mut edge_ids: Vec<usize> = graph.edges().map(|e| e.id()).collect();
    edge_ids.sort_unstable();
    for eid in edge_ids {
        let edge = graph.get_edge(eid).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if seg.pvertices.contains(&from) && seg.qvertices.contains(&to) {
            continue; // spanning segment, handled by Rule 2
        }
        if layer_of(graph, to) == layer_of(graph, from) + 1
            && let (Some(&p), Some(&c)) = (vertex_cell.get(&from), vertex_cell.get(&to))
        {
            link(p, c, off_of(eid));
        }
    }

    // Rule 2: each segment's straight run p → lanes → q. All internal links join
    // dummy ends, so they carry no offset (centred).
    let mut segment_chains = Vec::new();
    let mut qids: Vec<usize> = seg.qvertices.iter().copied().collect();
    qids.sort_unstable();
    for q in qids {
        let p = graph
            .get_node(q)
            .unwrap()
            .parents()
            .next()
            .unwrap()
            .node_id();
        let prank = layer_of(graph, p);
        let qrank = layer_of(graph, q);
        let mut chain = vec![vertex_cell[&p]];
        for r in (prank + 1)..qrank {
            chain.push(seg_cell[&(r, q)]);
        }
        chain.push(vertex_cell[&q]);
        for pair in chain.windows(2) {
            link(pair[0], pair[1], (0.0, 0.0));
        }
        segment_chains.push(chain);
    }

    Cells {
        rank,
        pos,
        width,
        is_dummy,
        entity,
        layers,
        parents,
        children,
        parent_off,
        child_off,
        segment_chains,
    }
}

/// Makes each p/lane/q chain a single coordinate variable, then restores every
/// per-rank separation inequality. Brandes–Köpf alignment is deliberately not
/// used for this: conflict marking may reject an alignment even though segment
/// continuity is a representation invariant.
fn enforce_segment_constraints(cells: &Cells, node_gap: f64, xs: &mut [f64]) {
    fn find(parent: &mut [usize], x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }

    let n = xs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    for chain in &cells.segment_chains {
        for pair in chain.windows(2) {
            let a = find(&mut parent, pair[0]);
            let b = find(&mut parent, pair[1]);
            parent[b] = a;
        }
    }
    let groups: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();

    // Seed each equality block at the mean of the four-pass result.
    let mut total = vec![0.0; n];
    let mut count = vec![0usize; n];
    for (i, &g) in groups.iter().enumerate() {
        total[g] += xs[i];
        count[g] += 1;
    }
    let mut value = vec![0.0; n];
    for i in 0..n {
        if count[i] != 0 {
            value[i] = total[i] / count[i] as f64;
        }
    }

    // Difference constraints x[right] >= x[left] + separation. Relaxing them
    // also covers constraints on ranks between p and q, whose coordinates are
    // otherwise transient and unavailable to the router.
    for _ in 0..n {
        let mut changed = false;
        for layer in &cells.layers {
            for pair in layer.windows(2) {
                let (left, right) = (pair[0], pair[1]);
                let (a, b) = (groups[left], groups[right]);
                if a == b {
                    continue;
                }
                let sep = (cells.width[left] + cells.width[right]) / 2.0 + node_gap;
                let required = value[a] + sep;
                if value[b] < required {
                    value[b] = required;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    debug_assert!(
        cells
            .layers
            .iter()
            .all(|layer| layer.windows(2).all(|pair| {
                let sep = (cells.width[pair[0]] + cells.width[pair[1]]) / 2.0 + node_gap;
                value[groups[pair[1]]] + 1e-9 >= value[groups[pair[0]]] + sep
            })),
        "incompatible segment/order constraints"
    );

    for (i, &g) in groups.iter().enumerate() {
        xs[i] = value[g];
    }
}

fn directional_pass(
    cells: &Cells,
    marked: &HashSet<(usize, usize)>,
    node_gap: f64,
    down: bool,
    left: bool,
) -> Vec<f64> {
    if left {
        core(cells, &cells.pos, marked, node_gap, down, false)
    } else {
        // Reflect horizontally: reversed pos within each rank, run leftward core,
        // negate. Separation feasibility is preserved under reflection. Drift is a
        // signed horizontal offset, so it mirrors with the rest (`mirror = true`
        // negates the per-link contributions before the final negation flips them
        // back into real space).
        let n = cells.rank.len();
        let mut pos = vec![0usize; n];
        for layer in &cells.layers {
            let len = layer.len();
            for (i, &c) in layer.iter().enumerate() {
                pos[c] = len - 1 - i;
            }
        }
        let mut xs = core(cells, &pos, marked, node_gap, down, true);
        for x in &mut xs {
            *x = -*x;
        }
        xs
    }
}

fn core(
    cells: &Cells,
    pos: &[usize],
    marked: &HashSet<(usize, usize)>,
    node_gap: f64,
    down: bool,
    mirror: bool,
) -> Vec<f64> {
    let n = cells.rank.len();
    let mut root: Vec<usize> = (0..n).collect();
    let mut align: Vec<usize> = (0..n).collect();
    let mut drift = vec![0.0f64; n];

    // Per-rank cell order under `pos`.
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); cells.layers.len()];
    for c in 0..n {
        layers[cells.rank[c]].push(c);
    }
    for layer in &mut layers {
        layer.sort_by_key(|&c| pos[c]);
    }

    vertical_align(
        cells, &layers, pos, marked, down, mirror, &mut root, &mut align, &mut drift,
    );
    horizontal_compact(cells, &layers, pos, node_gap, &root, &align, &drift)
}

#[allow(clippy::too_many_arguments)]
fn vertical_align(
    cells: &Cells,
    layers: &[Vec<usize>],
    pos: &[usize],
    marked: &HashSet<(usize, usize)>,
    down: bool,
    mirror: bool,
    root: &mut [usize],
    align: &mut [usize],
    drift: &mut [f64],
) {
    let rank_order: Vec<usize> = if down {
        (0..layers.len()).collect()
    } else {
        (0..layers.len()).rev().collect()
    };
    let sign = if mirror { -1.0 } else { 1.0 };

    for r in rank_order {
        let mut prev_bound: i64 = -1;
        for &v in &layers[r] {
            // (neighbour cell, its `(start_port, end_port)` link offsets). For
            // `down` the link is parent→v; for up it is v→child.
            let (cells_nb, offs) = if down {
                (&cells.parents[v], &cells.parent_off[v])
            } else {
                (&cells.children[v], &cells.child_off[v])
            };
            if cells_nb.is_empty() {
                continue;
            }
            let mut neighbors: Vec<(usize, f64, f64)> = cells_nb
                .iter()
                .zip(offs)
                .map(|(&c, &(s, e))| (c, s, e))
                .collect();
            neighbors.sort_by_key(|&(c, _, _)| pos[c]);
            let d = neighbors.len();
            for &m in &[(d - 1) / 2, d / 2] {
                if align[v] != v {
                    break;
                }
                let (um, start, end) = neighbors[m];
                let key = if down { (um, v) } else { (v, um) };
                if !marked.contains(&key) && prev_bound < pos[um] as i64 {
                    align[um] = v;
                    root[v] = root[um];
                    align[v] = root[v];
                    prev_bound = pos[um] as i64;
                    // drift[v] = drift[um] + (offset_at_um − offset_at_v): shifts v
                    // within the block so the linking edge is vertical at its ports.
                    // down link um→v: start is at um, end at v → start − end.
                    // up   link v→um: start is at v, end at um → end − start.
                    let contrib = if down { start - end } else { end - start };
                    drift[v] = drift[um] + sign * contrib;
                }
            }
        }
    }
}

fn horizontal_compact(
    cells: &Cells,
    layers: &[Vec<usize>],
    pos: &[usize],
    node_gap: f64,
    root: &[usize],
    align: &[usize],
    drift: &[f64],
) -> Vec<f64> {
    let n = cells.rank.len();
    let mut sink: Vec<usize> = (0..n).collect();
    let mut shift = vec![f64::INFINITY; n];
    let mut x = vec![f64::NAN; n];

    for layer in layers {
        for &v in layer {
            if root[v] == v {
                place_block(
                    cells, layers, pos, node_gap, root, align, drift, &mut sink, &mut shift,
                    &mut x, v,
                );
            }
        }
    }

    let mut result = vec![0.0f64; n];
    for v in 0..n {
        let r = root[v];
        let mut xv = x[r] + drift[v];
        let s = shift[sink[r]];
        if s.is_finite() {
            xv += s;
        }
        result[v] = xv;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn place_block(
    cells: &Cells,
    layers: &[Vec<usize>],
    pos: &[usize],
    node_gap: f64,
    root: &[usize],
    align: &[usize],
    drift: &[f64],
    sink: &mut [usize],
    shift: &mut [f64],
    x: &mut [f64],
    v: usize,
) {
    if !x[v].is_nan() {
        return;
    }
    x[v] = 0.0;

    let mut w = v;
    loop {
        let r = cells.rank[w];
        let p = pos[w];
        if p > 0 {
            let pred = layers[r][p - 1];
            let u = root[pred];
            place_block(
                cells, layers, pos, node_gap, root, align, drift, sink, shift, x, u,
            );
            if sink[v] == v {
                sink[v] = sink[u];
            }
            // Separation is on the drifted positions: with final x = x[root] +
            // drift, the gap between w and its left neighbour pred must clear
            // sep, i.e. (x[v]+drift[w]) − (x[u]+drift[pred]) ≥ sep.
            let sep = (cells.width[pred] + cells.width[w]) / 2.0 + node_gap;
            if sink[v] != sink[u] {
                let candidate = x[v] - x[u] - sep + drift[w] - drift[pred];
                shift[sink[u]] = shift[sink[u]].min(candidate);
            } else {
                let candidate = x[u] + sep + drift[pred] - drift[w];
                if candidate > x[v] {
                    x[v] = candidate;
                }
            }
        }
        w = align[w];
        if w == v {
            break;
        }
    }
}

/// Type-1 conflicts: a non-inner segment crossing an inner (dummy-dummy) one.
fn mark_type1(cells: &Cells) -> HashSet<(usize, usize)> {
    let mut marked = HashSet::default();
    for r in 0..cells.layers.len().saturating_sub(1) {
        let upper = &cells.layers[r];
        let lower = &cells.layers[r + 1];
        let mut k0: i64 = 0;
        let mut l = 0usize;
        for (l1, &v) in lower.iter().enumerate() {
            let inner_upper = inner_upper(cells, v, r);
            let is_last = l1 == lower.len() - 1;
            if is_last || inner_upper.is_some() {
                let k1 = inner_upper.unwrap_or_else(|| upper.len().saturating_sub(1) as i64);
                while l <= l1 {
                    let w = lower[l];
                    for &p in &cells.parents[w] {
                        let k = cells.pos[p] as i64;
                        if k < k0 || k > k1 {
                            marked.insert((p, w));
                        }
                    }
                    l += 1;
                }
                k0 = k1;
            }
        }
    }
    marked
}

fn inner_upper(cells: &Cells, v: usize, r: usize) -> Option<i64> {
    if !cells.is_dummy[v] {
        return None;
    }
    for &p in &cells.parents[v] {
        if cells.is_dummy[p] && cells.rank[p] == r {
            return Some(cells.pos[p] as i64);
        }
    }
    None
}
