//! Deterministic force-directed graph layout.
//!
//! This is a small, dependency-free energy layout intended for interactive
//! graph views where roughly 1,000 or fewer nodes are visible. It treats edge
//! direction as rendering metadata: physics uses undirected springs, and the
//! final edge waypoints are simple source-center to target-center lines.

use std::{
    collections::VecDeque,
    error::Error,
    fmt::{self, Debug},
    ops::{Add, AddAssign, Mul, Sub, SubAssign},
};

use rustc_hash::FxHashMap as HashMap;

use crate::{
    graph::{Graph, edge::Edge, node::Node, owning::OwningGraph},
    registry::Identifier,
    triskel::layout::{LayoutNode, LayoutResult, NodeGeometry, Point},
};

const DEFAULT_NODE_LIMIT: usize = 1_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Vec2 {
    x: f64,
    y: f64,
}

impl Vec2 {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    fn clamp_len(self, max_len: f64) -> Self {
        let len = self.len();
        if len > max_len {
            self * (max_len / len)
        } else {
            self
        }
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnergyAttraction {
    /// Hooke-style spring attraction: farther connected nodes pull harder.
    #[default]
    Spring,
    /// LinLog-style attraction: connected nodes pull with near-constant force.
    LinLog,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnergySettings {
    /// Edge attraction model.
    pub attraction: EnergyAttraction,
    /// Maximum number of simulation iterations.
    pub iterations: usize,
    /// Strength of inverse-square node repulsion.
    pub repulsion: f64,
    /// Strength of spring attraction along edges.
    pub spring_strength: f64,
    /// Natural edge length for spring attraction.
    pub ideal_edge_len: f64,
    /// Weak pull toward the origin.
    pub gravity: f64,
    /// Vertical "pressure" along directed edges: each edge pushes its source
    /// (caller) up by this amount and its target (callee) down by the same
    /// amount, scaled by edge weight. Settles entrypoints toward the top and
    /// deeply nested nodes toward the bottom. Kept much smaller than the edge
    /// forces: it biases the vertical equilibrium that gravity bounds, rather
    /// than setting the graph's scale. `0.0` disables it.
    pub pressure: f64,
    /// Velocity multiplier applied each step.
    pub damping: f64,
    /// Integration time step.
    pub dt: f64,
    /// Maximum distance any node may move in one iteration.
    pub max_step: f64,
    /// Refuse to lay out graphs above this visible-node count.
    pub max_nodes: usize,
    /// Number of post-layout passes that push apart overlapping node boxes.
    /// `0` disables overlap removal.
    pub overlap_passes: usize,
    /// Extra gap enforced between node boxes during overlap removal.
    pub overlap_margin: f64,
}

impl Default for EnergySettings {
    fn default() -> Self {
        Self {
            attraction: EnergyAttraction::default(),
            iterations: 400,
            repulsion: 10_000.0,
            spring_strength: 0.015,
            ideal_edge_len: 90.0,
            gravity: 0.002,
            pressure: 0.0,
            damping: 0.82,
            dt: 1.0,
            max_step: 15.0,
            max_nodes: DEFAULT_NODE_LIMIT,
            overlap_passes: 60,
            overlap_margin: 12.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyLayoutError {
    EmptyGraph,
    TooManyNodes { count: usize, max: usize },
}

impl fmt::Display for EnergyLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnergyLayoutError::EmptyGraph => f.write_str("cannot lay out an empty graph"),
            EnergyLayoutError::TooManyNodes { count, max } => {
                write!(
                    f,
                    "cannot lay out {count} nodes with energy layout limit {max}"
                )
            }
        }
    }
}

impl Error for EnergyLayoutError {}

struct SimNode<NodeId: Identifier> {
    id: NodeId,
    pos: Vec2,
    vel: Vec2,
    force: Vec2,
    mass: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy)]
struct SimEdge<EdgeId: Identifier> {
    id: EdgeId,
    source: usize,
    target: usize,
    weight: f64,
}

pub struct EnergyLayoutBuilder<'g, NodeId, EdgeId, NodeData, EdgeData>
where
    NodeId: Identifier,
    EdgeId: Identifier,
{
    graph: &'g OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    settings: EnergySettings,
    geometry: Box<dyn FnMut(NodeId) -> NodeGeometry + 'g>,
    /// Optional entrypoint. When set, nodes are seeded with a vertical position
    /// proportional to their BFS depth from this root (callees below callers),
    /// giving the pressure force a head start. When unset, nodes seed onto a
    /// circle as before.
    root: Option<NodeId>,
}

impl<'g, NodeId, EdgeId, NodeData, EdgeData>
    EnergyLayoutBuilder<'g, NodeId, EdgeId, NodeData, EdgeData>
where
    NodeId: Identifier + Debug,
    EdgeId: Identifier + Debug,
{
    pub fn new(graph: &'g OwningGraph<NodeId, EdgeId, NodeData, EdgeData>) -> Self {
        Self {
            graph,
            settings: EnergySettings::default(),
            geometry: Box::new(|_| NodeGeometry::default()),
            root: None,
        }
    }

    /// Seed the layout from an entrypoint: nodes get an initial vertical
    /// position by BFS depth from `root` (callees below callers). Pairs with
    /// [`EnergySettings::pressure`] to keep the entrypoint near the top.
    pub fn root(mut self, root: NodeId) -> Self {
        self.root = Some(root);
        self
    }

    pub fn settings(mut self, settings: EnergySettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn max_nodes(mut self, max_nodes: usize) -> Self {
        self.settings.max_nodes = max_nodes;
        self
    }

    pub fn attraction(mut self, attraction: EnergyAttraction) -> Self {
        self.settings.attraction = attraction;
        self
    }

    pub fn geometry<F>(mut self, geometry: F) -> Self
    where
        F: FnMut(NodeId) -> NodeGeometry + 'g,
    {
        self.geometry = Box::new(geometry);
        self
    }

    pub fn build(mut self) -> Result<LayoutResult<NodeId, EdgeId>, EnergyLayoutError> {
        let mut node_ids: Vec<NodeId> = self.graph.nodes().map(|n| n.id()).collect();
        node_ids.sort_by_key(|id| Into::<usize>::into(*id));

        if node_ids.is_empty() {
            return Err(EnergyLayoutError::EmptyGraph);
        }
        if node_ids.len() > self.settings.max_nodes {
            return Err(EnergyLayoutError::TooManyNodes {
                count: node_ids.len(),
                max: self.settings.max_nodes,
            });
        }

        let mut index = HashMap::default();
        for (i, &id) in node_ids.iter().enumerate() {
            index.insert(id, i);
        }

        let radius = if node_ids.len() <= 1 {
            0.0
        } else {
            self.settings.ideal_edge_len * (node_ids.len() as f64).sqrt()
        };

        let mut sim_nodes = Vec::with_capacity(node_ids.len());
        for (i, &id) in node_ids.iter().enumerate() {
            let geom = (self.geometry)(id);
            let angle = (i as f64 / node_ids.len() as f64) * std::f64::consts::TAU;
            let degree = self.graph.get_node(id).unwrap().edge_count();
            sim_nodes.push(SimNode {
                id,
                pos: Vec2::new(angle.cos() * radius, angle.sin() * radius),
                vel: Vec2::zero(),
                force: Vec2::zero(),
                mass: 1.0 + degree as f64 * 0.1,
                width: geom.width.max(1.0),
                height: geom.height.max(1.0),
            });
        }

        let mut edge_ids: Vec<EdgeId> = self.graph.edges().map(|e| e.id()).collect();
        edge_ids.sort_by_key(|id| Into::<usize>::into(*id));
        let mut sim_edges = Vec::new();
        for id in edge_ids {
            let edge = self.graph.get_edge(id).unwrap();
            let from = edge.from_id();
            let to = edge.to_id();
            if from == to {
                continue;
            }
            sim_edges.push(SimEdge {
                id,
                source: index[&from],
                target: index[&to],
                weight: 1.0,
            });
        }

        // Seed vertical position from BFS depth when an entrypoint is given:
        // keep the circular x spread (so repulsion has room to work) but place
        // callees below their callers. Nodes unreachable from the root keep
        // their circular y.
        if let Some(root) = self.root.and_then(|r| index.get(&r).copied()) {
            let mut adjacency = vec![Vec::new(); sim_nodes.len()];
            for edge in &sim_edges {
                adjacency[edge.source].push(edge.target);
            }

            let mut depth = vec![usize::MAX; sim_nodes.len()];
            depth[root] = 0;
            let mut queue = VecDeque::from([root]);
            while let Some(node) = queue.pop_front() {
                let next = depth[node] + 1;
                for &callee in &adjacency[node] {
                    if depth[callee] == usize::MAX {
                        depth[callee] = next;
                        queue.push_back(callee);
                    }
                }
            }

            for (i, node) in sim_nodes.iter_mut().enumerate() {
                if depth[i] != usize::MAX {
                    node.pos.y = depth[i] as f64 * self.settings.ideal_edge_len;
                }
            }
        }

        run_simulation(&mut sim_nodes, &sim_edges, self.settings);
        resolve_overlaps(&mut sim_nodes, self.settings);

        let nodes = sim_nodes
            .iter()
            .map(|node| {
                (
                    node.id,
                    LayoutNode {
                        id: node.id,
                        x: node.pos.x,
                        y: node.pos.y,
                        width: node.width,
                        height: node.height,
                    },
                )
            })
            .collect();

        let edges = sim_edges
            .iter()
            .map(|edge| {
                let source = sim_nodes[edge.source].pos;
                let target = sim_nodes[edge.target].pos;
                (
                    edge.id,
                    vec![
                        Point {
                            x: source.x,
                            y: source.y,
                        },
                        Point {
                            x: target.x,
                            y: target.y,
                        },
                    ],
                )
            })
            .collect();

        Ok(LayoutResult { nodes, edges })
    }
}

fn run_simulation<NodeId: Identifier, EdgeId: Identifier>(
    nodes: &mut [SimNode<NodeId>],
    edges: &[SimEdge<EdgeId>],
    settings: EnergySettings,
) {
    let n = nodes.len();

    for _ in 0..settings.iterations {
        for node in nodes.iter_mut() {
            node.force = Vec2::zero();
        }

        for i in 0..n {
            for j in (i + 1)..n {
                let delta = nodes[j].pos - nodes[i].pos;
                let dist = delta.len().max(1.0);
                let dir = delta * (1.0 / dist);
                // LinLog uses 1/dist repulsion (the gradient of -ln dist), which
                // pairs with its constant attraction to produce well-separated
                // clusters. Spring keeps the inverse-square (Fruchterman-Reingold)
                // repulsion. The two models use different `repulsion` scales, so
                // `ideal_edge_len` sets the LinLog repulsion strength instead.
                let force_mag = match settings.attraction {
                    EnergyAttraction::Spring => {
                        settings.repulsion * nodes[i].mass * nodes[j].mass / (dist * dist)
                    }
                    EnergyAttraction::LinLog => {
                        settings.ideal_edge_len * nodes[i].mass * nodes[j].mass / dist
                    }
                };
                let force = dir * force_mag;

                nodes[i].force -= force;
                nodes[j].force += force;
            }
        }

        for edge in edges {
            let delta = nodes[edge.target].pos - nodes[edge.source].pos;
            let dist = delta.len().max(1.0);
            let dir = delta * (1.0 / dist);
            let force_mag = match settings.attraction {
                EnergyAttraction::Spring => {
                    let stretch = dist - settings.ideal_edge_len;
                    settings.spring_strength * stretch * edge.weight
                }
                // Constant attraction; balances the 1/dist repulsion at an edge
                // length on the order of `ideal_edge_len`.
                EnergyAttraction::LinLog => edge.weight,
            };
            let force = dir * force_mag;

            nodes[edge.source].force += force;
            nodes[edge.target].force -= force;
        }

        // Vertical pressure along directed edges: caller floats up (−y), callee
        // sinks down (+y). Constant per edge, so it is inherently bounded and
        // sums to zero across the graph (no whole-graph drift). Cycles cancel.
        if settings.pressure != 0.0 {
            for edge in edges {
                let push = settings.pressure * edge.weight;
                nodes[edge.source].force.y -= push;
                nodes[edge.target].force.y += push;
            }
        }

        // Gravity centers on both axes: it bounds the graph's height (so
        // pressure shifts the equilibrium rather than growing it without limit)
        // and keeps unconnected nodes spread out instead of collapsing onto the
        // vertical center line.
        for node in nodes.iter_mut() {
            node.force += (Vec2::zero() - node.pos) * settings.gravity;
        }

        for node in nodes.iter_mut() {
            let accel = node.force * (1.0 / node.mass.max(1e-6));
            node.vel += accel * settings.dt;
            node.vel = node.vel * settings.damping;
            node.pos += (node.vel * settings.dt).clamp_len(settings.max_step);
        }
    }
}

/// Pushes apart any node boxes (axis-aligned, sized by width/height plus
/// `overlap_margin`) that overlap after the force simulation. Each pass moves
/// every overlapping pair apart by half their penetration along the axis of
/// least overlap, which preserves the cluster structure while separating boxes.
fn resolve_overlaps<NodeId: Identifier>(nodes: &mut [SimNode<NodeId>], settings: EnergySettings) {
    let n = nodes.len();
    let margin = settings.overlap_margin;

    for _ in 0..settings.overlap_passes {
        let mut moved = false;
        for i in 0..n {
            for j in (i + 1)..n {
                let min_x = (nodes[i].width + nodes[j].width) / 2.0 + margin;
                let min_y = (nodes[i].height + nodes[j].height) / 2.0 + margin;
                let delta = nodes[j].pos - nodes[i].pos;
                let overlap_x = min_x - delta.x.abs();
                let overlap_y = min_y - delta.y.abs();
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }

                // Separate along whichever axis needs the smaller push.
                let push = if overlap_x < overlap_y {
                    let dir = if delta.x >= 0.0 { 1.0 } else { -1.0 };
                    Vec2::new(dir * overlap_x / 2.0, 0.0)
                } else {
                    let dir = if delta.y >= 0.0 { 1.0 } else { -1.0 };
                    Vec2::new(0.0, dir * overlap_y / 2.0)
                };
                nodes[i].pos -= push;
                nodes[j].pos += push;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jstd_derive::Identifier;

    #[derive(Identifier)]
    struct N(usize);
    #[derive(Identifier)]
    struct E(usize);
    type G = OwningGraph<N, E, (), ()>;

    fn compact_settings() -> EnergySettings {
        EnergySettings {
            iterations: 80,
            ..Default::default()
        }
    }

    fn signature(result: &LayoutResult<N, E>) -> Vec<String> {
        let mut nodes: Vec<_> = result.nodes.values().collect();
        nodes.sort_by_key(|n| usize::from(n.id));
        let mut lines: Vec<String> = nodes
            .iter()
            .map(|n| format!("n{}:{:.3},{:.3}", usize::from(n.id), n.x, n.y))
            .collect();

        let mut edges: Vec<_> = result.edges.iter().collect();
        edges.sort_by_key(|(id, _)| usize::from(**id));
        for (id, points) in edges {
            let points = points
                .iter()
                .map(|p| format!("{:.3},{:.3}", p.x, p.y))
                .collect::<Vec<_>>()
                .join(";");
            lines.push(format!("e{}:{points}", usize::from(*id)));
        }
        lines
    }

    #[test]
    fn deterministic_for_same_graph() {
        let mut graph = G::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(a, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());

        let first = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .build()
            .unwrap();
        for _ in 0..10 {
            let next = EnergyLayoutBuilder::new(&graph)
                .settings(compact_settings())
                .build()
                .unwrap();
            assert_eq!(signature(&first), signature(&next));
        }
    }

    #[test]
    fn attraction_mode_changes_layout() {
        let mut graph = G::default();
        let hub = graph.make_node(());
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        graph.make_edge(hub, a, ());
        graph.make_edge(hub, b, ());
        graph.make_edge(a, c, ());
        graph.make_edge(b, d, ());
        graph.make_edge(c, d, ());

        let spring = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .build()
            .unwrap();
        let linlog = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .attraction(EnergyAttraction::LinLog)
            .build()
            .unwrap();

        assert_ne!(signature(&spring), signature(&linlog));
        assert_eq!(signature(&linlog), {
            let again = EnergyLayoutBuilder::new(&graph)
                .settings(compact_settings())
                .attraction(EnergyAttraction::LinLog)
                .build()
                .unwrap();
            signature(&again)
        });
    }

    #[test]
    fn edges_are_center_to_center_lines() {
        let mut graph = G::default();
        let a = graph.make_node(());
        let b = graph.make_node(());
        let edge = graph.make_edge(a, b, ());

        let result = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .geometry(|_| NodeGeometry {
                width: 80.0,
                height: 30.0,
            })
            .build()
            .unwrap();

        let points = result.get_waypoints(edge).unwrap();
        assert_eq!(points.len(), 2);
        let source = result.get_node(a).unwrap();
        let target = result.get_node(b).unwrap();
        assert_eq!(
            points,
            [
                Point {
                    x: source.x,
                    y: source.y
                },
                Point {
                    x: target.x,
                    y: target.y
                }
            ]
        );
    }

    #[test]
    fn linlog_separates_clusters() {
        // Two cliques joined by a single bridge edge must lay out as two
        // distinct groups: 1/dist repulsion plus constant attraction is what
        // gives LinLog its clustering, rather than collapsing onto one circle.
        let mut graph = G::default();
        let a: Vec<_> = (0..6).map(|_| graph.make_node(())).collect();
        let b: Vec<_> = (0..6).map(|_| graph.make_node(())).collect();
        for i in 0..6 {
            for j in (i + 1)..6 {
                graph.make_edge(a[i], a[j], ());
                graph.make_edge(b[i], b[j], ());
            }
        }
        graph.make_edge(a[0], b[0], ());

        let result = EnergyLayoutBuilder::new(&graph)
            .attraction(EnergyAttraction::LinLog)
            .build()
            .unwrap();

        let pos = |id: N| {
            let n = result.get_node(id).unwrap();
            (n.x, n.y)
        };
        let centroid = |v: &[N]| {
            let (mut cx, mut cy) = (0.0, 0.0);
            for &id in v {
                let (x, y) = pos(id);
                cx += x;
                cy += y;
            }
            (cx / v.len() as f64, cy / v.len() as f64)
        };
        let (ax, ay) = centroid(&a);
        let (bx, by) = centroid(&b);
        let inter = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        let (x0, y0) = pos(a[0]);
        let (x1, y1) = pos(a[1]);
        let intra = ((x0 - x1).powi(2) + (y0 - y1).powi(2)).sqrt();

        assert!(
            inter > 2.0 * intra,
            "clusters should separate: inter {inter} vs intra {intra}"
        );
    }

    #[test]
    fn overlap_removal_separates_boxes() {
        // A clique of large boxes packs tightly; after layout no two boxes may
        // overlap (centers must clear half the summed extents along some axis).
        let mut graph = G::default();
        let ids: Vec<_> = (0..8).map(|_| graph.make_node(())).collect();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                graph.make_edge(ids[i], ids[j], ());
            }
        }

        let (w, h, margin) = (120.0, 40.0, 12.0);
        let result = EnergyLayoutBuilder::new(&graph)
            .attraction(EnergyAttraction::LinLog)
            .geometry(|_| NodeGeometry {
                width: w,
                height: h,
            })
            .build()
            .unwrap();

        let placed: Vec<_> = result.nodes.values().collect();
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                let dx = (placed[i].x - placed[j].x).abs();
                let dy = (placed[i].y - placed[j].y).abs();
                // Allow a tiny epsilon for the half-overlap fixed point.
                assert!(
                    dx >= w + margin - 1.0 || dy >= h + margin - 1.0,
                    "boxes overlap: dx {dx} dy {dy}"
                );
            }
        }
    }

    #[test]
    fn pressure_stratifies_callers_above_callees() {
        // A simple call chain root -> a -> b -> c. With vertical pressure and a
        // root seed, each callee must settle strictly below its caller (down is
        // +y), so the chain reads top-to-bottom.
        let mut graph = G::default();
        let root = graph.make_node(());
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        graph.make_edge(root, a, ());
        graph.make_edge(a, b, ());
        graph.make_edge(b, c, ());

        let result = EnergyLayoutBuilder::new(&graph)
            .settings(EnergySettings {
                pressure: 1.0,
                ..compact_settings()
            })
            .root(root)
            .build()
            .unwrap();

        let y = |id: N| result.get_node(id).unwrap().y;
        assert!(
            y(root) < y(a),
            "root {} should sit above a {}",
            y(root),
            y(a)
        );
        assert!(y(a) < y(b), "a {} should sit above b {}", y(a), y(b));
        assert!(y(b) < y(c), "b {} should sit above c {}", y(b), y(c));
    }

    #[test]
    fn self_loops_are_skipped() {
        let mut graph = G::default();
        let a = graph.make_node(());
        let loop_edge = graph.make_edge(a, a, ());

        let result = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .build()
            .unwrap();

        assert!(result.get_waypoints(loop_edge).is_none());
        assert_eq!(result.nodes.len(), 1);
    }

    #[test]
    fn too_many_nodes_errors_before_simulation() {
        let mut graph = G::default();
        for _ in 0..3 {
            graph.make_node(());
        }

        let err = EnergyLayoutBuilder::new(&graph)
            .settings(compact_settings())
            .max_nodes(2)
            .build()
            .unwrap_err();

        assert_eq!(err, EnergyLayoutError::TooManyNodes { count: 3, max: 2 });
    }

    #[test]
    fn empty_graph_errors() {
        let graph = G::default();
        let err = EnergyLayoutBuilder::new(&graph).build().unwrap_err();
        assert_eq!(err, EnergyLayoutError::EmptyGraph);
    }
}
