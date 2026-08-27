//! Edge routing. Turns the positioned [`LayoutGraph`] into a waypoint polyline
//! per original edge.
//!
//! Each original edge is a chain of internal edges (real → dummies → real). The
//! chain is walked from its acyclic tail to head; ports fan the chain's ends
//! across the real endpoints' faces so sibling edges don't overlap. If the
//! original edge was reversed to break a cycle, the final polyline is reversed
//! so it runs source → target (the arrowhead lands on the true target).

use rustc_hash::FxHashMap as HashMap;

use crate::{
    graph::{Graph, edge::Edge, node::Node},
    triskel::layout::{LayoutGraph, Point, layer_of},
};

const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeStyle {
    /// Right-angle paths following the dummy columns.
    #[default]
    Orthogonal,
    /// Use a direct anchor-to-anchor polyline for each safe edge. Edges whose
    /// direct chain would enter a node use orthogonal channel lanes; safe edges
    /// in the same component remain straight.
    Straight,
}

/// Produces waypoints (keyed by original-edge id) from a positioned graph.
pub(crate) trait EdgeRouter {
    fn route(&self, graph: &LayoutGraph, layers: &[Vec<usize>]) -> HashMap<usize, Vec<Point>>;
}

pub(crate) struct OrthogonalRouter;
pub(crate) struct StraightRouter;

impl EdgeRouter for OrthogonalRouter {
    fn route(&self, graph: &LayoutGraph, layers: &[Vec<usize>]) -> HashMap<usize, Vec<Point>> {
        route_orthogonal(graph, layers)
    }
}

impl EdgeRouter for StraightRouter {
    fn route(&self, graph: &LayoutGraph, layers: &[Vec<usize>]) -> HashMap<usize, Vec<Point>> {
        let chains = collect_chains(graph);
        let (safe, fallback): (Vec<_>, Vec<_>) = chains
            .into_iter()
            .partition(|(_, chain)| straight_chain_is_safe(graph, chain));
        // All fallback chains are assigned together, so they share the same
        // channel occupancy/lane deconfliction as an orthogonal component.
        let mut out = route_orthogonal_chains(graph, layers, &fallback);
        for (orig, chain) in safe {
            let points = simplify(chain.anchors);
            if points.len() >= 2 {
                out.insert(orig, points);
            }
        }
        out
    }
}

fn straight_chain_is_safe(graph: &LayoutGraph, chain: &Chain) -> bool {
    // There is deliberately no whole-route endpoint exemption: a valid first
    // or last anchor merely touches a face, while any segment (including the
    // first/final one) entering an endpoint's strict interior is unsafe.
    chain.anchors.windows(2).all(|pair| {
        graph.nodes().filter(|node| !node.is_dummy).all(|node| {
            !crate::triskel::geometry::segment_enters_rect_strict(
                pair[0],
                pair[1],
                Point {
                    x: node.x,
                    y: node.y,
                },
                node.width,
                node.height,
            )
        })
    })
}

/// The source → target anchor chain of one original edge, plus per-anchor rank
/// (so each horizontal jog can be attributed to the inter-rank channel it lives
/// in).
#[derive(Clone)]
struct Chain {
    anchors: Vec<Point>,
    ranks: Vec<usize>,
}

/// Builds the source → target anchor chain for every original edge, in ascending
/// `orig` order. Malformed chains are skipped. A back-edge group (its pieces
/// carry the `reversed` marker) is reassembled by [`gadget_anchors`]; every other
/// edge is a plain downward chain via [`chain_anchors`].
fn collect_chains(graph: &LayoutGraph) -> Vec<(usize, Chain)> {
    let ports = snap_ports(graph, assign_ports(graph));

    // Group internal edges by the original edge they belong to.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::default();
    let mut edge_ids: Vec<usize> = graph.edges().map(|e| e.id()).collect();
    edge_ids.sort_unstable();
    for eid in edge_ids {
        let orig = graph.get_edge(eid).unwrap().orig;
        groups.entry(orig).or_default().push(eid);
    }

    let mut origs: Vec<usize> = groups.keys().copied().collect();
    origs.sort_unstable();
    let mut chains = Vec::new();
    for orig in origs {
        let group = &groups[&orig];
        let is_gadget = group.iter().any(|&e| graph.get_edge(e).unwrap().reversed);
        let built = if is_gadget {
            gadget_anchors(graph, group, &ports)
        } else {
            chain_anchors(graph, group, &ports)
        };
        let Some((anchors, ranks)) = built else {
            continue;
        };
        chains.push((orig, Chain { anchors, ranks }));
    }
    chains
}

/// Orthogonal routing with horizontal-segment lane assignment.
///
/// Each edge's horizontal jog between two adjacent ranks lives in that
/// inter-rank *channel*. Naively every jog sits at the channel midpoint, so
/// jogs with overlapping x-ranges draw on top of each other. Instead we group
/// the jogs per channel and stack them in *lanes* spread across the channel's
/// vertical band so no two horizontal segments share a y.
///
/// The lane order is chosen to keep a node's fan-out crossing-free: a longer
/// jog (one reaching further from its source port) turns *nearer the source*
/// (a higher lane), a shorter jog turns *nearer the targets* (a lower lane).
/// Whether the jog goes left or right, the furthest target nests outside the
/// closest one. Non-overlapping jogs still share a lane, so a clean fan with no
/// x-overlap collapses to the single channel midpoint.
fn route_orthogonal(graph: &LayoutGraph, layers: &[Vec<usize>]) -> HashMap<usize, Vec<Point>> {
    let chains = collect_chains(graph);
    route_orthogonal_chains(graph, layers, &chains)
}

/// Routes one selected set of chains while allocating all of their jog lanes
/// together. This is used by `Straight` fallback as well as fully orthogonal
/// routing; routing fallback edges one-by-one would permit collinear overlap.
fn route_orthogonal_chains(
    graph: &LayoutGraph,
    layers: &[Vec<usize>],
    chains: &[(usize, Chain)],
) -> HashMap<usize, Vec<Point>> {
    let bands = channel_bands(graph, layers);

    // A horizontal jog: the anchor index `idx` within edge `orig`, the channel
    // (= rank above it), and its x-extent.
    struct Jog {
        orig: usize,
        idx: usize,
        x0: f64,
        x1: f64,
    }
    let mut by_channel: HashMap<usize, Vec<Jog>> = HashMap::default();
    for (orig, chain) in chains {
        for (idx, pair) in chain.anchors.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            if (a.x - b.x).abs() < EPS {
                continue; // vertical: no horizontal component
            }
            // Lanes are only meaningful for adjacent-rank jogs; this includes
            // the upward attachment of a back-edge gadget. A (vertical)
            // multi-rank segment is excluded above.
            if chain.ranks[idx].abs_diff(chain.ranks[idx + 1]) != 1 {
                continue;
            }
            by_channel
                .entry(chain.ranks[idx].min(chain.ranks[idx + 1]))
                .or_default()
                .push(Jog {
                    orig: *orig,
                    idx,
                    x0: a.x.min(b.x),
                    x1: a.x.max(b.x),
                });
        }
    }

    // Assign each jog a y. Longest first, then first-fit into the top-most lane
    // it doesn't x-overlap. Processing long jogs first puts them on higher lanes
    // (nearer the source); a shorter jog overlapping a longer one is forced
    // below it — the nesting that keeps the fan crossing-free. Non-overlapping
    // jogs reuse a lane, so a clean fan stays at the channel midpoint.
    let mut lane_y: HashMap<(usize, usize), f64> = HashMap::default();
    let mut channels: Vec<usize> = by_channel.keys().copied().collect();
    channels.sort_unstable();
    for ch in channels {
        let jogs = by_channel.get_mut(&ch).unwrap();
        jogs.sort_by(|a, b| {
            (b.x1 - b.x0)
                .total_cmp(&(a.x1 - a.x0))
                .then(a.x0.total_cmp(&b.x0))
                .then(a.x1.total_cmp(&b.x1))
                .then(a.orig.cmp(&b.orig))
        });
        // Per lane, the x-spans already placed in it.
        let mut lanes: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut lane_of: Vec<usize> = Vec::with_capacity(jogs.len());
        for jog in jogs.iter() {
            let fits = lanes.iter().position(|occ| {
                occ.iter()
                    .all(|&(o0, o1)| jog.x1 <= o0 + EPS || jog.x0 + EPS >= o1)
            });
            let lane = match fits {
                Some(l) => {
                    lanes[l].push((jog.x0, jog.x1));
                    l
                }
                None => {
                    lanes.push(vec![(jog.x0, jog.x1)]);
                    lanes.len() - 1
                }
            };
            lane_of.push(lane);
        }
        let n_lanes = lanes.len();
        let (top, bottom) = bands[&ch];
        for (jog, &lane) in jogs.iter().zip(&lane_of) {
            let y = top + (bottom - top) * (lane + 1) as f64 / (n_lanes + 1) as f64;
            lane_y.insert((jog.orig, jog.idx), y);
        }
    }

    // Stitch each polyline, dropping the horizontal jog onto its lane y.
    let mut out = HashMap::default();
    for (orig, chain) in chains {
        let mut points = vec![chain.anchors[0]];
        for (idx, pair) in chain.anchors.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            if (a.x - b.x).abs() < EPS {
                points.push(b);
                continue;
            }
            // Lane y if one was assigned, else the midpoint (multi-rank jog).
            let y = lane_y
                .get(&(*orig, idx))
                .copied()
                .unwrap_or((a.y + b.y) / 2.0);
            points.push(Point { x: a.x, y });
            points.push(Point { x: b.x, y });
            points.push(b);
        }
        let points = simplify(points);
        if points.len() >= 2 {
            out.insert(*orig, points);
        }
    }
    out
}

/// The vertical band of each inter-rank channel: from the bottom edge of the
/// tallest node in the upper rank to the top edge of the tallest node in the
/// lower rank. Horizontal lanes placed inside it never clip a node box. Only
/// channels whose bounding ranks both hold a vertex are returned.
fn channel_bands(graph: &LayoutGraph, layers: &[Vec<usize>]) -> HashMap<usize, (f64, f64)> {
    let mut rank_y = vec![0.0f64; layers.len()];
    let mut rank_h = vec![0.0f64; layers.len()];
    for (r, nodes) in layers.iter().enumerate() {
        for &id in nodes {
            let node = graph.get_node(id).unwrap();
            rank_y[r] = node.y; // every node in a rank shares its centre y
            rank_h[r] = rank_h[r].max(node.height);
        }
    }

    let mut bands = HashMap::default();
    for r in 0..layers.len().saturating_sub(1) {
        if layers[r].is_empty() || layers[r + 1].is_empty() {
            continue;
        }
        let top = rank_y[r] + rank_h[r] / 2.0;
        let bottom = rank_y[r + 1] - rank_h[r + 1] / 2.0;
        bands.insert(r, (top, bottom));
    }
    bands
}

/// Walks a forward chain tail → head and returns its anchor points (the tail's
/// bottom port, each dummy centre, then the head's top port) paired with each
/// anchor's rank. `None` if the chain is malformed.
fn chain_anchors(
    graph: &LayoutGraph,
    chain: &[usize],
    ports: &Ports,
) -> Option<(Vec<Point>, Vec<usize>)> {
    // Build the downward path from outgoing-edge adjacency within the group.
    let mut next: HashMap<usize, usize> = HashMap::default(); // from-node -> edge id
    let mut is_target: HashMap<usize, bool> = HashMap::default();
    for &eid in chain {
        let edge = graph.get_edge(eid).unwrap();
        next.insert(edge.from_id(), eid);
        is_target.entry(edge.from_id()).or_insert(false);
        is_target.insert(edge.to_id(), true);
    }
    let tail = *is_target.iter().find(|(_, t)| !**t).map(|(n, _)| n)?;

    let mut ordered_edges = Vec::new();
    let mut cur = tail;
    while let Some(&eid) = next.get(&cur) {
        ordered_edges.push(eid);
        cur = graph.get_edge(eid).unwrap().to_id();
    }
    if ordered_edges.is_empty() {
        return None;
    }

    let first = ordered_edges[0];
    let last = *ordered_edges.last().unwrap();
    let start_port = ports.start.get(&first).copied().unwrap_or(0.0);
    let end_port = ports.end.get(&last).copied().unwrap_or(0.0);

    let mut anchors = Vec::new();
    let mut ranks = Vec::new();
    let tail_node = graph.get_node(tail).unwrap();
    anchors.push(Point {
        x: tail_node.x + start_port,
        y: tail_node.y + tail_node.height / 2.0,
    });
    ranks.push(layer_of(graph, tail));

    // Intermediate dummies are the `to` of every edge except the last.
    for &eid in &ordered_edges[..ordered_edges.len() - 1] {
        let mid_id = graph.get_edge(eid).unwrap().to_id();
        let mid = graph.get_node(mid_id).unwrap();
        anchors.push(Point { x: mid.x, y: mid.y });
        ranks.push(layer_of(graph, mid_id));
    }

    let head_id = graph.get_edge(last).unwrap().to_id();
    let head = graph.get_node(head_id).unwrap();
    anchors.push(Point {
        x: head.x + end_port,
        y: head.y - head.height / 2.0,
    });
    ranks.push(layer_of(graph, head_id));

    Some((anchors, ranks))
}

/// Reassembles a back-edge gadget (see [`crate::triskel::cycle`]) into a single
/// source → target polyline. The group holds three logical pieces, told apart by
/// which ends are virtual: `B -> B'` (real → virtual), `A' -> A` (virtual →
/// real), and the `A' -> … -> B'` column (virtual → virtual). The result leaves
/// the source's bottom, runs down to `B'`, climbs the column to `A'`, and drops
/// into the target's top — the column and attach jogs already aligned by the
/// x-coordinate pass. `None` if the group isn't a well-formed gadget.
fn gadget_anchors(
    graph: &LayoutGraph,
    group: &[usize],
    ports: &Ports,
) -> Option<(Vec<Point>, Vec<usize>)> {
    let mut src_attach = None; // B -> B'
    let mut tgt_attach = None; // A' -> A
    let mut next: HashMap<usize, usize> = HashMap::default(); // column: from-node -> edge id
    for &eid in group {
        let edge = graph.get_edge(eid).unwrap();
        let from_dummy = graph.get_node(edge.from_id()).unwrap().is_dummy;
        let to_dummy = graph.get_node(edge.to_id()).unwrap().is_dummy;
        match (from_dummy, to_dummy) {
            (false, true) => src_attach = Some(eid),
            (true, false) => tgt_attach = Some(eid),
            (true, true) => {
                next.insert(edge.from_id(), eid);
            }
            (false, false) => return None,
        }
    }
    let src_attach = src_attach?;
    let tgt_attach = tgt_attach?;
    let source = graph.get_edge(src_attach).unwrap().from_id(); // B
    let b_prime = graph.get_edge(src_attach).unwrap().to_id(); // B'
    let a_prime = graph.get_edge(tgt_attach).unwrap().from_id(); // A'
    let target = graph.get_edge(tgt_attach).unwrap().to_id(); // A

    // Column node centres, walked A' → … → B'.
    let mut column = vec![a_prime];
    let mut cur = a_prime;
    while let Some(&eid) = next.get(&cur) {
        cur = graph.get_edge(eid).unwrap().to_id();
        column.push(cur);
    }
    if *column.last().unwrap() != b_prime {
        return None;
    }

    let sp = ports.start.get(&src_attach).copied().unwrap_or(0.0);
    let tp = ports.end.get(&tgt_attach).copied().unwrap_or(0.0);
    let src = graph.get_node(source).unwrap();
    let tgt = graph.get_node(target).unwrap();

    let mut anchors = Vec::new();
    let mut ranks = Vec::new();

    // Source bottom port.
    anchors.push(Point {
        x: src.x + sp,
        y: src.y + src.height / 2.0,
    });
    ranks.push(layer_of(graph, source));

    // Up the column, from B' to A'.
    for &nid in column.iter().rev() {
        let n = graph.get_node(nid).unwrap();
        anchors.push(Point { x: n.x, y: n.y });
        ranks.push(layer_of(graph, nid));
    }

    // Target top port.
    anchors.push(Point {
        x: tgt.x + tp,
        y: tgt.y - tgt.height / 2.0,
    });
    ranks.push(layer_of(graph, target));

    Some((anchors, ranks))
}

// ── Ports ───────────────────────────────────────────────────────────────────

pub(crate) struct Ports {
    pub start: HashMap<usize, f64>, // offset from from-node centre (bottom face)
    pub end: HashMap<usize, f64>,   // offset from to-node centre (top face)
}

/// Distributes each real node's outgoing edges across its bottom face and
/// incoming edges across its top face, ordered by the neighbour's within-rank
/// position so sibling edges fan out without crossing near the node.
///
/// The graph is acyclic here (back-edges became gadgets in
/// [`crate::triskel::cycle`]), so an edge always leaves its source's bottom and
/// enters its target's top — a back-edge gadget's `B -> B'` simply fans on B's
/// bottom and its `A' -> A` on A's top, with no special-casing.
///
/// Deterministic in node width + within-rank order only, so the x-coordinate
/// pass can compute the same offsets up front (it needs them to align edge
/// endpoints rather than node centres — see [`crate::triskel::coordinate`]).
pub(crate) fn assign_ports(graph: &LayoutGraph) -> Ports {
    let mut start = HashMap::default();
    let mut end = HashMap::default();

    let mut node_ids: Vec<usize> = graph.nodes().map(|n| n.id()).collect();
    node_ids.sort_unstable();

    for id in node_ids {
        let node = graph.get_node(id).unwrap();
        if node.is_dummy {
            continue; // dummy ports stay centred (offset 0)
        }
        let half = node.width / 2.0;

        let outs = ordered_out_edges(graph, id);
        let n = outs.len();
        for (i, &eid) in outs.iter().enumerate() {
            start.insert(eid, -half + node.width * (i + 1) as f64 / (n + 1) as f64);
        }

        let ins = ordered_in_edges(graph, id);
        let n = ins.len();
        for (i, &eid) in ins.iter().enumerate() {
            end.insert(eid, -half + node.width * (i + 1) as f64 / (n + 1) as f64);
        }
    }

    Ports { start, end }
}

/// A node's outgoing edge ids, ordered as [`assign_ports`] fans them across the
/// bottom face — by the target's within-rank order, then its id, then edge id.
fn ordered_out_edges(graph: &LayoutGraph, id: usize) -> Vec<usize> {
    let node = graph.get_node(id).unwrap();
    let mut outs: Vec<(usize, usize, usize)> = node
        .children()
        .map(|c| {
            (
                graph.get_node(c.node_id()).unwrap().order,
                c.node_id(),
                c.edge_id(),
            )
        })
        .collect();
    outs.sort_unstable();
    outs.into_iter().map(|(_, _, eid)| eid).collect()
}

/// A node's incoming edge ids, ordered as [`assign_ports`] fans them across the
/// top face.
fn ordered_in_edges(graph: &LayoutGraph, id: usize) -> Vec<usize> {
    let node = graph.get_node(id).unwrap();
    let mut ins: Vec<(usize, usize, usize)> = node
        .parents()
        .map(|p| {
            (
                graph.get_node(p.node_id()).unwrap().order,
                p.node_id(),
                p.edge_id(),
            )
        })
        .collect();
    ins.sort_unstable();
    ins.into_iter().map(|(_, _, eid)| eid).collect()
}

/// The [lo, hi] offset range one port may occupy without changing the fan
/// order: bounded by the midpoints to its even-fan neighbours (and the node's
/// own faces at the ends). Disjoint per index, so nudged ports stay ordered.
fn port_bounds(width: f64, index: usize, count: usize) -> (f64, f64) {
    let half = width / 2.0;
    let even = |k: usize| -half + width * (k + 1) as f64 / (count + 1) as f64;
    let lo = if index == 0 {
        -half
    } else {
        (even(index - 1) + even(index)) / 2.0
    };
    let hi = if index + 1 == count {
        half
    } else {
        (even(index) + even(index + 1)) / 2.0
    };
    (lo, hi)
}

/// Straightens single-segment real→real edges *when it can be done cleanly*. An
/// edge between two real nodes one rank apart is a whole chain (long edges carry
/// dummies instead), so its two ports may move within their faces without
/// disturbing a dummy column.
///
/// A common attach x makes the edge a single vertical line. We only move the
/// ports if such an x exists within both ports' [`port_bounds`] slots — i.e.
/// the edge can be made fully straight without reordering the fan — and then
/// pick the x nearest the two even-fan attachments, so the ports barely leave
/// their slots. If no shared x exists (the target sits well off to the side) the
/// even fan is kept verbatim: the fan stays evenly spaced and never collapses
/// onto a node's corner.
fn snap_ports(graph: &LayoutGraph, mut ports: Ports) -> Ports {
    let mut node_ids: Vec<usize> = graph.nodes().map(|n| n.id()).collect();
    node_ids.sort_unstable();

    for id in node_ids {
        let node = graph.get_node(id).unwrap();
        if node.is_dummy {
            continue;
        }
        let from_x = node.x;
        let outs = ordered_out_edges(graph, id);
        let n_out = outs.len();
        for (i, &eid) in outs.iter().enumerate() {
            let to_id = graph.get_edge(eid).unwrap().to_id();
            let to = graph.get_node(to_id).unwrap();
            if to.is_dummy {
                continue;
            }
            let ins = ordered_in_edges(graph, to_id);
            let j = ins.iter().position(|&e| e == eid).unwrap();

            let (s_lo, s_hi) = port_bounds(node.width, i, n_out);
            let (e_lo, e_hi) = port_bounds(to.width, j, ins.len());

            // The attach-x window each face allows, intersected. Empty → leave
            // the even fan alone.
            let lo = (from_x + s_lo).max(to.x + e_lo);
            let hi = (from_x + s_hi).min(to.x + e_hi);
            if lo > hi {
                continue;
            }
            let a_even = from_x + ports.start[&eid];
            let b_even = to.x + ports.end[&eid];
            let x = ((a_even + b_even) / 2.0).clamp(lo, hi);
            ports.start.insert(eid, x - from_x);
            ports.end.insert(eid, x - to.x);
        }
    }

    ports
}

// ── Polyline cleanup ────────────────────────────────────────────────────────

fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if out.last().is_some_and(|last| points_eq(*last, point)) {
            continue;
        }
        while out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            let collinear = (a.x == b.x && b.x == point.x) || (a.y == b.y && b.y == point.y);
            if collinear {
                out.pop();
            } else {
                break;
            }
        }
        out.push(point);
    }
    out
}

fn points_eq(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triskel::layout::{EdgeLayoutData, NodeLayoutData};

    fn node(graph: &mut LayoutGraph, rank: i64, x: f64, y: f64, dummy: bool) -> usize {
        graph.make_node(NodeLayoutData {
            rank,
            x,
            y,
            width: if dummy { 0.0 } else { 20.0 },
            height: if dummy { 0.0 } else { 20.0 },
            is_dummy: dummy,
            ..Default::default()
        })
    }

    #[test]
    fn straight_falls_back_per_edge_without_converting_safe_chains() {
        let mut graph = LayoutGraph::default();
        // orig 0 is a safe diagonal on the right. Orig 1's first diagonal
        // crosses `blocker`, so only it must receive orthogonal jogs.
        let safe_a = node(&mut graph, 0, 60.0, 0.0, false);
        let unsafe_a = node(&mut graph, 0, -40.0, 0.0, false);
        let blocker = node(&mut graph, 1, 0.0, 50.0, false);
        let dummy = node(&mut graph, 1, 0.0, 50.0, true);
        let dummy_two = node(&mut graph, 1, 0.0, 50.0, true);
        let safe_b = node(&mut graph, 1, 80.0, 50.0, false);
        let unsafe_b = node(&mut graph, 2, 40.0, 100.0, false);
        let unsafe_c = node(&mut graph, 0, -60.0, 0.0, false);
        let unsafe_d = node(&mut graph, 2, 45.0, 100.0, false);
        graph.make_edge(
            safe_a,
            safe_b,
            EdgeLayoutData {
                orig: 0,
                ..Default::default()
            },
        );
        graph.make_edge(
            unsafe_a,
            dummy,
            EdgeLayoutData {
                orig: 1,
                ..Default::default()
            },
        );
        graph.make_edge(
            dummy,
            unsafe_b,
            EdgeLayoutData {
                orig: 1,
                ..Default::default()
            },
        );
        graph.make_edge(
            unsafe_c,
            dummy_two,
            EdgeLayoutData {
                orig: 2,
                ..Default::default()
            },
        );
        graph.make_edge(
            dummy_two,
            unsafe_d,
            EdgeLayoutData {
                orig: 2,
                ..Default::default()
            },
        );
        let layers = vec![
            vec![safe_a, unsafe_a, unsafe_c],
            vec![blocker, dummy, dummy_two, safe_b],
            vec![unsafe_b, unsafe_d],
        ];
        let result = StraightRouter.route(&graph, &layers);
        assert_eq!(result[&0].len(), 2, "safe edge stays direct");
        assert!(result[&1].len() > 2, "unsafe edge alone falls back");
        assert!(
            result[&1]
                .windows(2)
                .all(|p| { (p[0].x - p[1].x).abs() < EPS || (p[0].y - p[1].y).abs() < EPS })
        );
        let horizontal_y = |points: &[Point]| {
            points.windows(2).find_map(|p| {
                ((p[0].y - p[1].y).abs() < EPS && (p[0].x - p[1].x).abs() > EPS).then_some(p[0].y)
            })
        };
        assert_ne!(
            horizontal_y(&result[&1]),
            horizontal_y(&result[&2]),
            "overlapping fallback jogs need distinct lanes"
        );
        assert_eq!(result, StraightRouter.route(&graph, &layers));
    }

    #[test]
    fn strict_safety_rejects_endpoint_reentry_but_allows_face_attachment() {
        let mut graph = LayoutGraph::default();
        let source = node(&mut graph, 0, 0.0, 0.0, false);
        let target = node(&mut graph, 1, 30.0, 30.0, false);
        let safe = Chain {
            anchors: vec![Point { x: 0.0, y: 10.0 }, Point { x: 30.0, y: 20.0 }],
            ranks: vec![0, 1],
        };
        assert!(straight_chain_is_safe(&graph, &safe));
        let reentry = Chain {
            anchors: vec![
                Point { x: 0.0, y: 10.0 },
                Point { x: 0.0, y: 0.0 },
                Point { x: 30.0, y: 20.0 },
            ],
            ranks: vec![0, 0, 1],
        };
        assert!(!straight_chain_is_safe(&graph, &reentry));
        assert_ne!(source, target);
    }
}
