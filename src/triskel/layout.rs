//! Public layout API and per-component orchestration.
//!
//! [`LayoutBuilder`] is the entry point. It runs the layered-layout pipeline on
//! each weakly-connected component of the input graph independently, then packs
//! the components left-to-right. Internally the work is done on a `usize`-id
//! [`LayoutGraph`] (so dummy vertices can be minted freely); results are mapped
//! back to the caller's strongly-typed node/edge ids.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{self, Debug},
};

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    graph::{
        Graph, GraphMut,
        analysis::{SeseRegion, SeseTree, compute_sese, compute_sese_candidates},
        edge::Edge,
        node::Node,
        owning::OwningGraph,
    },
    registry::Identifier,
    triskel::{
        coordinate, cycle,
        hammock::compute_hammocks,
        order, rank,
        render::{render_html, render_html_with_labels, render_svg, render_svg_with_labels},
        router::{EdgeRouter, EdgeStyle, OrthogonalRouter, StraightRouter},
        segment,
    },
};

const DEFAULT_NODE_SIZE: f64 = 24.0;
const DEFAULT_LAYER_GAP: f64 = 50.0;
const DEFAULT_NODE_GAP: f64 = 40.0;
const DEFAULT_MAX_SWEEPS: usize = 8;

/// Protrusion of the innermost self-loop beyond the node's right edge.
const SELF_LOOP_GAP: f64 = 16.0;
/// Extra protrusion for each further self-loop stacked on the same node.
const SELF_LOOP_STEP: f64 = 8.0;

/// Horizontal room a node must reserve on its right to draw `count` self-loops.
pub(crate) fn loop_reserve(count: u32) -> f64 {
    if count == 0 {
        0.0
    } else {
        SELF_LOOP_GAP + (count - 1) as f64 * SELF_LOOP_STEP
    }
}

/// The orthogonal waypoints for the `index`-th of `total` self-loops on a node
/// at `(x, y)` with size `width`×`height`. Loops nest: outer ones span more of
/// the right face and protrude further, so stacked loops never draw over each
/// other. The polyline leaves and re-enters the node's right face.
fn self_loop_waypoints(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    index: u32,
    total: u32,
) -> Vec<Point> {
    let right = x + width / 2.0;
    let span = height / 2.0 * (index + 1) as f64 / (total + 1) as f64;
    let out = right + SELF_LOOP_GAP + index as f64 * SELF_LOOP_STEP;
    vec![
        Point {
            x: right,
            y: y - span,
        },
        Point {
            x: out,
            y: y - span,
        },
        Point {
            x: out,
            y: y + span,
        },
        Point {
            x: right,
            y: y + span,
        },
    ]
}

// ── Public value types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeGeometry {
    pub width: f64,
    pub height: f64,
}

impl Default for NodeGeometry {
    fn default() -> Self {
        Self {
            width: DEFAULT_NODE_SIZE,
            height: DEFAULT_NODE_SIZE,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LayoutMode {
    /// Lay out each weak component as one Eiglsperger graph.
    #[default]
    Flat,
    /// Recursively lay out canonical SESE regions as sized quotient nodes.
    Sese,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutSettings {
    /// Vertical gap between the facing edges of adjacent ranks.
    pub layer_gap: f64,
    /// Minimum horizontal gap between the facing edges of adjacent nodes.
    pub node_gap: f64,
    /// Edge drawing style.
    pub edge_style: EdgeStyle,
    /// Maximum up/down ordering sweeps during crossing reduction.
    pub max_sweeps: usize,
    /// Component orchestration strategy.
    pub mode: LayoutMode,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            layer_gap: DEFAULT_LAYER_GAP,
            node_gap: DEFAULT_NODE_GAP,
            edge_style: EdgeStyle::default(),
            max_sweeps: DEFAULT_MAX_SWEEPS,
            mode: LayoutMode::Flat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    EmptyGraph,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::EmptyGraph => f.write_str("cannot lay out an empty graph"),
        }
    }
}

impl Error for LayoutError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutNode<NodeId: Identifier> {
    pub id: NodeId,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct LayoutResult<NodeId: Identifier, EdgeId: Identifier> {
    pub nodes: HashMap<NodeId, LayoutNode<NodeId>>,
    pub edges: HashMap<EdgeId, Vec<Point>>,
    /// Analysis-only SESE proxy bounds, populated in [`LayoutMode::Sese`].
    /// They are useful for debug renderers and are empty for flat layout.
    pub regions: Vec<LayoutRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutRegion {
    pub id: usize,
    pub parent: Option<usize>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl<NodeId: Identifier, EdgeId: Identifier> LayoutResult<NodeId, EdgeId> {
    pub fn get_node(&self, node: NodeId) -> Option<&LayoutNode<NodeId>> {
        self.nodes.get(&node)
    }

    pub fn get_waypoints(&self, edge: EdgeId) -> Option<&[Point]> {
        self.edges.get(&edge).map(Vec::as_slice)
    }

    pub fn render_svg(&self) -> String {
        render_svg::<NodeId, EdgeId>(self)
    }

    pub fn render_svg_with_labels<F>(&self, label_for: F) -> String
    where
        F: FnMut(NodeId) -> String,
    {
        render_svg_with_labels::<NodeId, EdgeId, F>(self, label_for)
    }

    pub fn render_html(&self, title: &str) -> String {
        render_html::<NodeId, EdgeId>(self, title)
    }

    pub fn render_html_with_labels<F>(&self, title: &str, label_for: F) -> String
    where
        F: FnMut(NodeId) -> String,
    {
        render_html_with_labels::<NodeId, EdgeId, F>(self, title, label_for)
    }
}

// ── Internal working graph ──────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub(crate) struct NodeLayoutData {
    pub width: f64,
    pub height: f64,
    pub rank: i64,
    pub order: usize,
    pub x: f64,
    pub y: f64,
    pub is_dummy: bool,
    /// Number of self-loop edges drawn off this node's right face. Each reserves
    /// horizontal room (see [`loop_reserve`]) so the loops clear the right
    /// neighbour, mirroring how back-edges reserve a wrap column.
    pub self_loops: u32,
}

impl Default for NodeLayoutData {
    fn default() -> Self {
        Self {
            width: DEFAULT_NODE_SIZE,
            height: DEFAULT_NODE_SIZE,
            rank: 0,
            order: 0,
            x: 0.0,
            y: 0.0,
            is_dummy: false,
            self_loops: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EdgeLayoutData {
    pub minlen: i64,
    pub weight: i64,
    /// True if this edge was reversed to break a cycle (drawn back-to-front).
    pub reversed: bool,
    /// The caller's original edge id (as `usize`) this internal edge belongs to.
    pub orig: usize,
    /// Fixed offsets for composition through a region proxy.  These remain
    /// layout-local: public graph edges never carry routing state.
    pub port_start: Option<f64>,
    pub port_end: Option<f64>,
}

impl Default for EdgeLayoutData {
    fn default() -> Self {
        Self {
            minlen: 1,
            weight: 1,
            reversed: false,
            orig: 0,
            port_start: None,
            port_end: None,
        }
    }
}

pub(crate) type LayoutGraph = OwningGraph<usize, usize, NodeLayoutData, EdgeLayoutData>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum RegionPortId<NodeId> {
    Entry,
    Exit { source: NodeId },
}

#[derive(Clone, Copy, Debug)]
struct ProxyPort<NodeId> {
    id: RegionPortId<NodeId>,
    x_offset: f64,
    is_entry: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct PortHint {
    source_bottom: Option<f64>,
    target_top: Option<f64>,
}

/// Rank read as a layer index (ranks are normalised non-negative before use).
pub(crate) fn layer_of(graph: &LayoutGraph, node: usize) -> usize {
    graph.get_node(node).unwrap().rank.max(0) as usize
}

// ── Builder ─────────────────────────────────────────────────────────────────

pub struct LayoutBuilder<'g, NodeId, EdgeId, NodeData, EdgeData>
where
    NodeId: Identifier,
    EdgeId: Identifier,
{
    graph: &'g OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    root: Option<NodeId>,
    settings: LayoutSettings,
    geometry: Box<dyn FnMut(NodeId) -> NodeGeometry + 'g>,
}

impl<'g, NodeId, EdgeId, NodeData, EdgeData> LayoutBuilder<'g, NodeId, EdgeId, NodeData, EdgeData>
where
    NodeId: Identifier + Debug,
    EdgeId: Identifier + Debug,
{
    pub fn new(graph: &'g OwningGraph<NodeId, EdgeId, NodeData, EdgeData>) -> Self {
        Self {
            graph,
            root: None,
            settings: LayoutSettings::default(),
            geometry: Box::new(|_| NodeGeometry::default()),
        }
    }

    /// Preferred root: nodes in its component are seeded from it. Optional —
    /// other components fall back to their min-id node.
    pub fn root(mut self, root: NodeId) -> Self {
        self.root = Some(root);
        self
    }

    pub fn node_gap(mut self, gap: f64) -> Self {
        self.settings.node_gap = gap;
        self
    }

    pub fn layer_gap(mut self, gap: f64) -> Self {
        self.settings.layer_gap = gap;
        self
    }

    pub fn edge_style(mut self, style: EdgeStyle) -> Self {
        self.settings.edge_style = style;
        self
    }

    pub fn max_sweeps(mut self, sweeps: usize) -> Self {
        self.settings.max_sweeps = sweeps;
        self
    }

    pub fn mode(mut self, mode: LayoutMode) -> Self {
        self.settings.mode = mode;
        self
    }

    pub fn settings(mut self, settings: LayoutSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn geometry<F>(mut self, geometry: F) -> Self
    where
        F: FnMut(NodeId) -> NodeGeometry + 'g,
    {
        self.geometry = Box::new(geometry);
        self
    }

    pub fn build(mut self) -> Result<LayoutResult<NodeId, EdgeId>, LayoutError> {
        let mut node_ids: Vec<NodeId> = self.graph.nodes().map(|n| n.id()).collect();
        node_ids.sort_by_key(|id| Into::<usize>::into(*id));
        if node_ids.is_empty() {
            return Err(LayoutError::EmptyGraph);
        }

        let mut geometry = HashMap::default();
        for id in &node_ids {
            let g = (self.geometry)(*id);
            geometry.insert(
                *id,
                NodeGeometry {
                    width: g.width.max(1.0),
                    height: g.height.max(1.0),
                },
            );
        }

        let components = weakly_connected_components(self.graph, &node_ids);

        let mut nodes = HashMap::default();
        let mut edges = HashMap::default();
        let mut regions = Vec::new();
        let mut x_offset = 0.0f64;

        for component in components {
            let local = match self.settings.mode {
                LayoutMode::Flat => {
                    layout_component(self.graph, &component, &geometry, self.root, &self.settings)
                }
                LayoutMode::Sese => layout_component_sese(
                    self.graph,
                    &component,
                    &geometry,
                    self.root,
                    &self.settings,
                ),
            };

            let (min_x, max_x, min_y) = local_bounds(&local);
            let shift_x = x_offset - min_x;
            let shift_y = -min_y;

            for (id, mut node) in local.nodes {
                node.x += shift_x;
                node.y += shift_y;
                nodes.insert(id, node);
            }
            let region_base = regions.len();
            for region in local.regions {
                regions.push(LayoutRegion {
                    id: region_base + region.id,
                    parent: region.parent.map(|parent| region_base + parent),
                    x: region.x + shift_x,
                    y: region.y + shift_y,
                    ..region
                });
            }
            for (id, points) in local.edges {
                edges.insert(
                    id,
                    points
                        .into_iter()
                        .map(|p| Point {
                            x: p.x + shift_x,
                            y: p.y + shift_y,
                        })
                        .collect(),
                );
            }

            x_offset += (max_x - min_x) + self.settings.node_gap;
        }

        Ok(LayoutResult {
            nodes,
            edges,
            regions,
        })
    }
}

// ── Per-component pipeline ──────────────────────────────────────────────────

struct ComponentLayout<NodeId: Identifier, EdgeId: Identifier> {
    nodes: HashMap<NodeId, LayoutNode<NodeId>>,
    edges: HashMap<EdgeId, Vec<Point>>,
    regions: Vec<LayoutRegion>,
}

/// Enumerate raw edge-SESE candidates on the same explicitly normalized CFG
/// used by [`compute_sese_normalized`]. Synthetic boundaries and nodes are
/// discarded before layout sees the candidate.
fn compute_sese_candidates_normalized<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    root: NodeId,
) -> Vec<(Vec<NodeId>, EdgeId, EdgeId)>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let mut augmented = OwningGraph::<usize, usize, (), ()>::default();
    let super_entry = augmented.make_node(());
    let super_exit = augmented.make_node(());
    let mut augmented_node = HashMap::default();
    let mut original_node = HashMap::default();
    for &node in component {
        let augmented_id = augmented.make_node(());
        augmented_node.insert(node, augmented_id);
        original_node.insert(augmented_id, node);
    }
    augmented.make_edge(super_entry, augmented_node[&root], ());
    let component_set: HashSet<_> = component.iter().copied().collect();
    let mut has_real_out = HashMap::<NodeId, bool>::default();
    for &node in component {
        has_real_out.insert(node, false);
    }
    let mut original_edge = HashMap::default();
    let mut edge_ids: Vec<_> = graph.edges().map(|edge| edge.id()).collect();
    edge_ids.sort();
    for edge_id in edge_ids {
        let edge = graph.get_edge(edge_id).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if !component_set.contains(&from) || !component_set.contains(&to) {
            continue;
        }
        let augmented_id = augmented.make_edge(augmented_node[&from], augmented_node[&to], ());
        original_edge.insert(augmented_id, edge_id);
        if from != to {
            has_real_out.insert(from, true);
        }
    }
    let exits: Vec<_> = component
        .iter()
        .copied()
        .filter(|node| !has_real_out[node])
        .collect();
    if exits.is_empty() {
        augmented.make_edge(augmented_node[component.last().unwrap()], super_exit, ());
    } else {
        for node in exits {
            augmented.make_edge(augmented_node[&node], super_exit, ());
        }
    }

    compute_sese_candidates(&augmented, super_entry)
        .into_iter()
        .filter_map(|candidate| {
            Some((
                candidate
                    .contained_nodes
                    .into_iter()
                    .map(|node| original_node.get(&node).copied())
                    .collect::<Option<Vec<_>>>()?,
                original_edge.get(&candidate.entry_edge).copied()?,
                original_edge.get(&candidate.exit_edge).copied()?,
            ))
        })
        .collect()
}

/// Compute SESE regions on a CFG normalized into one explicit hammock: an
/// analysis-only source enters the selected root and all natural exits enter
/// an analysis-only sink.  The synthetic boundaries establish the outer
/// hammock without leaking fake nodes or edges into the composition tree.
fn compute_sese_normalized<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    root: NodeId,
) -> SeseTree<NodeId, EdgeId>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let mut augmented = OwningGraph::<usize, usize, (), ()>::default();
    let super_entry = augmented.make_node(());
    let super_exit = augmented.make_node(());
    let mut augmented_node = HashMap::default();
    let mut original_node = HashMap::default();
    for &node in component {
        let augmented_id = augmented.make_node(());
        augmented_node.insert(node, augmented_id);
        original_node.insert(augmented_id, node);
    }
    augmented.make_edge(super_entry, augmented_node[&root], ());

    let component_set: HashSet<_> = component.iter().copied().collect();
    let mut has_real_out = HashMap::<NodeId, bool>::default();
    for &node in component {
        has_real_out.insert(node, false);
    }
    let mut original_edge = HashMap::default();
    let mut edge_ids: Vec<_> = graph.edges().map(|edge| edge.id()).collect();
    edge_ids.sort();
    for edge_id in edge_ids {
        let edge = graph.get_edge(edge_id).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if !component_set.contains(&from) || !component_set.contains(&to) {
            continue;
        }
        let augmented_id = augmented.make_edge(augmented_node[&from], augmented_node[&to], ());
        original_edge.insert(augmented_id, edge_id);
        if from != to {
            has_real_out.insert(from, true);
        }
    }
    let exits: Vec<_> = component
        .iter()
        .copied()
        .filter(|node| !has_real_out[node])
        .collect();
    if exits.is_empty() {
        // Match compute_sese's deterministic normalization for a closed CFG.
        augmented.make_edge(augmented_node[component.last().unwrap()], super_exit, ());
    } else {
        for node in exits {
            augmented.make_edge(augmented_node[&node], super_exit, ());
        }
    }

    let raw = compute_sese(&augmented, super_entry);
    let mut old_to_new = vec![None; raw.regions.len()];
    old_to_new[0] = Some(0);
    let mut regions = vec![SeseRegion {
        parent: None,
        children: Vec::new(),
        entry_edge: None,
        exit_edge: None,
        nodes: Vec::new(),
        contained_nodes: component.to_vec(),
    }];
    for (old, region) in raw.regions.iter().enumerate().skip(1) {
        let Some(entry) = region
            .entry_edge
            .and_then(|edge| original_edge.get(&edge))
            .copied()
        else {
            continue;
        };
        let Some(exit) = region
            .exit_edge
            .and_then(|edge| original_edge.get(&edge))
            .copied()
        else {
            continue;
        };
        let Some(contained_nodes) = region
            .contained_nodes
            .iter()
            .map(|node| original_node.get(node).copied())
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if contained_nodes.is_empty() {
            continue;
        }
        let new = regions.len();
        old_to_new[old] = Some(new);
        regions.push(SeseRegion {
            parent: None,
            children: Vec::new(),
            entry_edge: Some(entry),
            exit_edge: Some(exit),
            nodes: Vec::new(),
            contained_nodes,
        });
    }
    for old in 1..raw.regions.len() {
        let Some(new) = old_to_new[old] else {
            continue;
        };
        let mut parent = raw.regions[old].parent;
        while let Some(old_parent) = parent {
            if let Some(new_parent) = old_to_new[old_parent] {
                regions[new].parent = Some(new_parent);
                regions[new_parent].children.push(new);
                break;
            }
            parent = raw.regions[old_parent].parent;
        }
    }
    for &node in component {
        let owner = regions
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, region)| region.contained_nodes.contains(&node))
            .min_by_key(|(_, region)| region.contained_nodes.len())
            .map(|(id, _)| id)
            .unwrap_or(0);
        regions[owner].nodes.push(node);
    }
    SeseTree { regions }
}

/// Candidate node hammocks. A hammock may have several boundary edges,
/// provided they all leave for the same external exit node.
#[cfg(test)]
fn compute_hammock_candidates<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
) -> Vec<(Vec<NodeId>, EdgeId, EdgeId)>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let component_set: HashSet<_> = component.iter().copied().collect();
    let mut edges: Vec<_> = graph
        .edges()
        .filter_map(|edge| {
            (component_set.contains(&edge.from_id()) && component_set.contains(&edge.to_id()))
                .then_some((edge.id(), edge.from_id(), edge.to_id()))
        })
        .collect();
    edges.sort_by_key(|(id, _, _)| *id);
    let mut candidates = Vec::<(Vec<NodeId>, EdgeId, EdgeId)>::new();
    for &entry in component {
        for &exit in component {
            if entry == exit {
                continue;
            }
            let mut forward = HashSet::default();
            let mut stack = vec![entry];
            while let Some(node) = stack.pop() {
                if node == exit || !forward.insert(node) {
                    continue;
                }
                stack.extend(
                    edges
                        .iter()
                        .filter_map(|(_, from, to)| (*from == node).then_some(*to)),
                );
            }
            let mut reverse = HashSet::default();
            let mut stack = vec![exit];
            while let Some(node) = stack.pop() {
                if !reverse.insert(node) {
                    continue;
                }
                stack.extend(
                    edges
                        .iter()
                        .filter_map(|(_, from, to)| (*to == node).then_some(*from)),
                );
            }
            let contained: HashSet<_> = forward.intersection(&reverse).copied().collect();
            if contained.len() <= 1 || !contained.contains(&entry) {
                continue;
            }
            let incoming: Vec<_> = edges
                .iter()
                .filter(|(_, from, to)| !contained.contains(from) && contained.contains(to))
                .collect();
            let outgoing: Vec<_> = edges
                .iter()
                .filter(|(_, from, to)| contained.contains(from) && !contained.contains(to))
                .collect();
            if incoming.is_empty()
                || outgoing.is_empty()
                || incoming.iter().any(|(_, _, to)| *to != entry)
                || outgoing.iter().any(|(_, _, to)| *to != exit)
            {
                continue;
            }
            let mut nodes: Vec<_> = contained.into_iter().collect();
            nodes.sort();
            candidates.push((nodes, incoming[0].0, outgoing[0].0));
        }
    }
    candidates.sort_by_key(|(nodes, entry, exit)| (std::cmp::Reverse(nodes.len()), *entry, *exit));
    candidates
}

/// Finds maximal node-hammocks when edge-based SESE has no useful region.
#[cfg(test)]
fn compute_hammock_fallback<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
) -> SeseTree<NodeId, EdgeId>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let candidates = compute_hammock_candidates(graph, component);
    let mut regions = vec![SeseRegion {
        parent: None,
        children: Vec::new(),
        entry_edge: None,
        exit_edge: None,
        nodes: Vec::new(),
        contained_nodes: component.to_vec(),
    }];
    for (nodes, entry, exit) in candidates {
        if regions.iter().skip(1).any(|region| {
            region
                .contained_nodes
                .iter()
                .any(|node| nodes.contains(node))
        }) {
            continue;
        }
        let id = regions.len();
        regions.push(SeseRegion {
            parent: Some(0),
            children: Vec::new(),
            entry_edge: Some(entry),
            exit_edge: Some(exit),
            nodes: Vec::new(),
            contained_nodes: nodes,
        });
        regions[0].children.push(id);
    }
    for &node in component {
        let owner = regions
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(id, region)| region.contained_nodes.contains(&node).then_some(id))
            .unwrap_or(0);
        regions[owner].nodes.push(node);
    }
    SeseTree { regions }
}

/// Layout's mixed region tree retains maximal node hammocks and useful raw
/// edge-SESE regions nested inside them. This deliberately does not alter the
/// public canonical `compute_sese` selection rule.
fn compute_layout_sese_tree<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    root: NodeId,
) -> SeseTree<NodeId, EdgeId>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let hammocks: Vec<_> = compute_hammocks(graph, component, root)
        .into_iter()
        .map(|hammock| (hammock.nodes, hammock.entry_edge, hammock.exit_edge))
        .collect();
    // Without a hammock, the established canonical hierarchy is the best
    // structural representation and preserves previous SESE behaviour.
    if hammocks.is_empty() {
        return compute_sese_normalized(graph, component, root);
    }

    let mut selected = Vec::new();
    for candidate in compute_sese_candidates_normalized(graph, component, root) {
        if candidate.0.len() <= 1
            || !hammocks.iter().any(|(nodes, _, _)| {
                candidate.0.len() < nodes.len()
                    && candidate.0.iter().all(|node| nodes.contains(node))
            })
        {
            continue;
        }
        let laminar = selected
            .iter()
            .all(|(other, _, _): &(Vec<NodeId>, EdgeId, EdgeId)| {
                let intersects = candidate.0.iter().any(|node| other.contains(node));
                !intersects
                    || candidate.0.iter().all(|node| other.contains(node))
                    || other.iter().all(|node| candidate.0.contains(node))
            });
        if laminar
            && !selected
                .iter()
                .any(|(nodes, _, _): &(Vec<NodeId>, EdgeId, EdgeId)| *nodes == candidate.0)
        {
            selected.push(candidate);
        }
    }
    selected.sort_by_key(|(nodes, entry, exit)| (std::cmp::Reverse(nodes.len()), *entry, *exit));

    let mut regions = vec![SeseRegion {
        parent: None,
        children: Vec::new(),
        entry_edge: None,
        exit_edge: None,
        nodes: Vec::new(),
        contained_nodes: component.to_vec(),
    }];
    for (nodes, entry_edge, exit_edge) in hammocks {
        let id = regions.len();
        regions.push(SeseRegion {
            parent: Some(0),
            children: Vec::new(),
            entry_edge: Some(entry_edge),
            exit_edge: Some(exit_edge),
            nodes: Vec::new(),
            contained_nodes: nodes,
        });
        regions[0].children.push(id);
    }
    for (nodes, entry_edge, exit_edge) in selected {
        let parent = regions
            .iter()
            .enumerate()
            .filter(|(_, region)| {
                nodes
                    .iter()
                    .all(|node| region.contained_nodes.contains(node))
            })
            .min_by_key(|(_, region)| region.contained_nodes.len())
            .map(|(id, _)| id)
            .unwrap_or(0);
        let id = regions.len();
        regions.push(SeseRegion {
            parent: Some(parent),
            children: Vec::new(),
            entry_edge: Some(entry_edge),
            exit_edge: Some(exit_edge),
            nodes: Vec::new(),
            contained_nodes: nodes,
        });
        regions[parent].children.push(id);
    }
    for &node in component {
        let owner = regions
            .iter()
            .enumerate()
            .filter(|(_, region)| region.contained_nodes.contains(&node))
            .min_by_key(|(_, region)| region.contained_nodes.len())
            .map(|(id, _)| id)
            .unwrap_or(0);
        regions[owner].nodes.push(node);
    }
    SeseTree { regions }
}

fn layout_component_sese<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    geometry: &HashMap<NodeId, NodeGeometry>,
    preferred_root: Option<NodeId>,
    settings: &LayoutSettings,
) -> ComponentLayout<NodeId, EdgeId>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let component_set: HashSet<_> = component.iter().copied().collect();
    let root = preferred_root
        .filter(|root| component_set.contains(root))
        .unwrap_or_else(|| *component.iter().min().unwrap());
    let tree = compute_layout_sese_tree(graph, component, root);
    let useful_region = tree
        .regions
        .iter()
        .skip(1)
        .any(|region| region.contained_nodes.len() > 1);
    if !useful_region || tree.root().contained_nodes.len() != component.len() {
        return layout_component(graph, component, geometry, preferred_root, settings);
    }

    let composition = compose_sese_region(graph, geometry, settings, &tree, 0, root);
    // A proxy port is valid only when every parent/child endpoint equality and
    // geometry invariant holds. Do not resurrect an expanded-graph router to
    // repair a bad composition: retain flat layout as the defensive fallback.
    if !composition.valid
        || !routes_clear_of_nodes(&composition.nodes, &composition.edges)
        || !routes_are_deconflicted(&composition.edges)
        || (settings.edge_style == EdgeStyle::Orthogonal
            && !routes_are_orthogonal(&composition.edges))
    {
        return layout_component(graph, component, geometry, preferred_root, settings);
    }
    let RegionComposition {
        mut nodes,
        edges,
        mut region_boxes,
        ..
    } = composition;
    // Keep the component centred around its own bounds; the public packer will
    // apply the final top/left translation.
    let (min_x, max_x, min_y, max_y) = node_bounds(&nodes);
    let dx = -(min_x + max_x) / 2.0;
    let dy = -(min_y + max_y) / 2.0;
    for node in nodes.values_mut() {
        node.x += dx;
        node.y += dy;
    }
    let regions = region_boxes
        .drain(..)
        .map(|region| LayoutRegion {
            id: region.id,
            parent: region.parent,
            x: (region.min_x + region.max_x) / 2.0 + dx,
            y: (region.min_y + region.max_y) / 2.0 + dy,
            width: region.max_x - region.min_x,
            height: region.max_y - region.min_y,
        })
        .collect();
    let edges = edges
        .into_iter()
        .map(|(id, points)| {
            (
                id,
                points
                    .into_iter()
                    .map(|point| Point {
                        x: point.x + dx,
                        y: point.y + dy,
                    })
                    .collect(),
            )
        })
        .collect();
    ComponentLayout {
        nodes,
        edges,
        regions,
    }
}

#[derive(Clone, Copy)]
struct RegionBox {
    id: usize,
    parent: Option<usize>,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

struct RegionInterface<NodeId: Identifier> {
    width: f64,
    height: f64,
    ports: BTreeMap<RegionPortId<NodeId>, ProxyPort<NodeId>>,
    /// Internal routes start/end at the corresponding named boundary port.
    entry_route: Option<Vec<Point>>,
    exit_routes: HashMap<NodeId, Vec<Point>>,
}

struct RegionComposition<NodeId: Identifier, EdgeId: Identifier> {
    nodes: HashMap<NodeId, LayoutNode<NodeId>>,
    /// Original-edge routes produced by flat layouts at this region or one of
    /// its descendants. A region proxy is only an intermediate layout node.
    edges: HashMap<EdgeId, Vec<Point>>,
    interface: RegionInterface<NodeId>,
    valid: bool,
    region_boxes: Vec<RegionBox>,
}

fn compose_sese_region<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    geometry: &HashMap<NodeId, NodeGeometry>,
    settings: &LayoutSettings,
    tree: &SeseTree<NodeId, EdgeId>,
    region_id: usize,
    component_root: NodeId,
) -> RegionComposition<NodeId, EdgeId>
where
    NodeId: Identifier + Debug + Ord,
    EdgeId: Identifier + Debug + Ord,
{
    let region = &tree.regions[region_id];
    let children: Vec<_> = region
        .children
        .iter()
        .map(|child| {
            (
                *child,
                compose_sese_region(graph, geometry, settings, tree, *child, component_root),
            )
        })
        .collect();

    let mut valid = children.iter().all(|(_, child)| child.valid);
    let mut quotient = OwningGraph::<usize, usize, (), ()>::default();
    let mut entity_for_node = HashMap::default();
    let mut direct_entity = HashMap::default();
    let mut proxy_entity = HashMap::default();
    let mut child_for_proxy = HashMap::default();
    let mut quotient_geometry = HashMap::default();
    for &node in &region.nodes {
        let entity = quotient.make_node(());
        direct_entity.insert(entity, node);
        entity_for_node.insert(node, entity);
        quotient_geometry.insert(entity, geometry[&node]);
    }
    for (child_id, child) in &children {
        let entity = quotient.make_node(());
        proxy_entity.insert(*child_id, entity);
        child_for_proxy.insert(entity, *child_id);
        quotient_geometry.insert(
            entity,
            // The parent proxy is exactly the child's interface rectangle.
            // Its named ports are offsets from this same centre.
            NodeGeometry {
                width: child.interface.width,
                height: child.interface.height,
            },
        );
        for &node in &tree.regions[*child_id].contained_nodes {
            entity_for_node.insert(node, entity);
        }
    }

    let contained: HashSet<_> = region.contained_nodes.iter().copied().collect();
    let mut original_for_quotient = HashMap::default();
    // A non-root region has an entry terminal and one exit terminal for every
    // boundary source. Canonical edge SESE has one; a node hammock can have
    // several edges leaving distinct nodes for the same external exit node.
    let exit_sources: Vec<_> = graph
        .edges()
        .filter_map(|edge| {
            (contained.contains(&edge.from_id())
                && !contained.contains(&edge.to_id())
                && edge.from_id() != edge.to_id())
            .then_some(edge.from_id())
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let interface = region.entry_edge.map(|entry| {
        let entry_terminal = quotient.make_node(());
        quotient_geometry.insert(
            entry_terminal,
            NodeGeometry {
                width: 1.0,
                height: 1.0,
            },
        );
        let entry_target = graph.get_edge(entry).unwrap().to_id();
        let entry_link = quotient.make_edge(entry_terminal, entity_for_node[&entry_target], ());
        let mut exits = Vec::new();
        for source in &exit_sources {
            let terminal = quotient.make_node(());
            quotient_geometry.insert(
                terminal,
                NodeGeometry {
                    width: 1.0,
                    height: 1.0,
                },
            );
            let link = quotient.make_edge(entity_for_node[source], terminal, ());
            exits.push((*source, terminal, link));
        }
        (entry_terminal, entry_link, exits)
    });
    let mut edge_ids: Vec<_> = graph.edges().map(|edge| edge.id()).collect();
    edge_ids.sort();
    for edge_id in edge_ids {
        let edge = graph.get_edge(edge_id).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if !contained.contains(&from) || !contained.contains(&to) || from == to {
            continue;
        }
        let from_entity = entity_for_node[&from];
        let to_entity = entity_for_node[&to];
        if from_entity != to_entity {
            let quotient_edge = quotient.make_edge(from_entity, to_entity, ());
            original_for_quotient.insert(quotient_edge, edge_id);
        }
    }

    let mut entities: Vec<_> = quotient.nodes().map(|node| node.id()).collect();
    entities.sort_unstable();
    let entry_node = region
        .entry_edge
        .and_then(|edge| graph.get_edge(edge).map(|edge| edge.to_id()))
        .unwrap_or(component_root);
    let quotient_root = entity_for_node
        .get(&entry_node)
        .copied()
        .unwrap_or_else(|| entities[0]);
    // A quotient edge that touches a child proxy is constrained to the exact
    // named child port. This is established before coordinate assignment and
    // routing, rather than repaired while expanding the child.
    let child_ports: HashMap<usize, BTreeMap<RegionPortId<NodeId>, ProxyPort<NodeId>>> = children
        .iter()
        .map(|(child_id, child)| (proxy_entity[child_id], child.interface.ports.clone()))
        .collect();
    let mut port_hints = HashMap::default();
    for (&quotient_edge, &original) in &original_for_quotient {
        let edge = graph.get_edge(original).unwrap();
        let mut hint = PortHint::default();
        if let Some(ports) = child_ports.get(&entity_for_node[&edge.from_id()]) {
            hint.source_bottom = ports
                .get(&RegionPortId::Exit {
                    source: edge.from_id(),
                })
                .filter(|port| {
                    !port.is_entry
                        && port.id
                            == RegionPortId::Exit {
                                source: edge.from_id(),
                            }
                })
                .map(|port| port.x_offset);
        }
        if let Some(ports) = child_ports.get(&entity_for_node[&edge.to_id()]) {
            hint.target_top = ports
                .get(&RegionPortId::Entry)
                .filter(|port| port.is_entry && port.id == RegionPortId::Entry)
                .map(|port| port.x_offset);
        }
        if hint.source_bottom.is_some() || hint.target_top.is_some() {
            port_hints.insert(quotient_edge, hint);
        }
    }
    if let Some((_, entry_link, exits)) = &interface {
        let target = graph.get_edge(region.entry_edge.unwrap()).unwrap().to_id();
        if let Some(port) = child_ports
            .get(&entity_for_node[&target])
            .and_then(|ports| ports.get(&RegionPortId::Entry))
            .filter(|port| port.is_entry && port.id == RegionPortId::Entry)
        {
            port_hints.insert(
                *entry_link,
                PortHint {
                    target_top: Some(port.x_offset),
                    ..Default::default()
                },
            );
        }
        for (source, _, link) in exits {
            if let Some(port) = child_ports
                .get(&entity_for_node[source])
                .and_then(|ports| ports.get(&RegionPortId::Exit { source: *source }))
                .filter(|port| !port.is_entry && port.id == RegionPortId::Exit { source: *source })
            {
                port_hints.insert(
                    *link,
                    PortHint {
                        source_bottom: Some(port.x_offset),
                        ..Default::default()
                    },
                );
            }
        }
    }
    let mut flat_settings = *settings;
    flat_settings.mode = LayoutMode::Flat;
    let quotient_layout = layout_component_with_port_hints(
        &quotient,
        &entities,
        &quotient_geometry,
        Some(quotient_root),
        &flat_settings,
        &port_hints,
    );

    let (mut entry_interface, mut exit_interfaces) =
        if let Some((entry, entry_link, exits)) = interface {
            let entry_node = quotient_layout.nodes[&entry];
            let mut entry_points = vec![
                Point {
                    x: entry_node.x,
                    y: entry_node.y - entry_node.height / 2.0,
                },
                Point {
                    x: entry_node.x,
                    y: entry_node.y + entry_node.height / 2.0,
                },
            ];
            entry_points.extend(quotient_layout.edges[&entry_link].iter().copied().skip(1));
            let mut paths = HashMap::default();
            for (source, terminal, link) in exits {
                let terminal_node = quotient_layout.nodes[&terminal];
                let mut points = quotient_layout.edges[&link].clone();
                points.push(Point {
                    x: terminal_node.x,
                    y: terminal_node.y + terminal_node.height / 2.0,
                });
                paths.insert(source, points);
            }
            (Some(entry_points), paths)
        } else {
            (None, HashMap::default())
        };

    let mut nodes = HashMap::default();
    for (entity, original) in direct_entity {
        let positioned = quotient_layout.nodes[&entity];
        let geom = geometry[&original];
        nodes.insert(
            original,
            LayoutNode {
                id: original,
                x: positioned.x,
                y: positioned.y,
                width: geom.width,
                height: geom.height,
            },
        );
    }
    let mut edges = HashMap::default();
    let mut region_boxes = Vec::new();
    let mut child_interfaces = HashMap::default();
    for (child_id, mut child) in children {
        let proxy = quotient_layout.nodes[&proxy_entity[&child_id]];
        for (_, mut node) in child.nodes.drain() {
            node.x += proxy.x;
            node.y += proxy.y;
            nodes.insert(node.id, node);
        }
        for (edge_id, points) in child.edges.drain() {
            edges.insert(edge_id, translate_points(points, proxy.x, proxy.y));
        }
        for mut region_box in child.region_boxes.drain(..) {
            region_box.min_x += proxy.x;
            region_box.max_x += proxy.x;
            region_box.min_y += proxy.y;
            region_box.max_y += proxy.y;
            region_boxes.push(region_box);
        }
        let child_interface = RegionInterface {
            width: child.interface.width,
            height: child.interface.height,
            ports: child.interface.ports,
            entry_route: child
                .interface
                .entry_route
                .take()
                .map(|points| translate_points(points, proxy.x, proxy.y)),
            exit_routes: child
                .interface
                .exit_routes
                .drain()
                .map(|(source, points)| (source, translate_points(points, proxy.x, proxy.y)))
                .collect(),
        };
        child_interfaces.insert(proxy_entity[&child_id], child_interface);
    }

    // Synthetic terminal links can themselves end at a nested proxy. Expand
    // them exactly like an original quotient edge, so an interface always
    // reaches the real boundary node rather than stopping at an inner proxy.
    if let Some(entry) = region.entry_edge {
        let target = graph.get_edge(entry).unwrap().to_id();
        let entity = entity_for_node[&target];
        if child_for_proxy.contains_key(&entity) {
            let nested_entry = child_interfaces[&entity].entry_route.as_ref().unwrap();
            let points = entry_interface.as_mut().unwrap();
            valid &= join_equal_points(points, nested_entry);
        }
    }
    for (&source, points) in &mut exit_interfaces {
        let entity = entity_for_node[&source];
        if child_for_proxy.contains_key(&entity) {
            let nested_exits = &child_interfaces[&entity].exit_routes;
            let mut expanded = nested_exits[&source].clone();
            valid &= join_equal_points(&mut expanded, points);
            *points = expanded;
        }
    }

    // Every quotient edge retains the route from the ordinary flat pipeline.
    // When an endpoint is a proxy, splice that route to the original endpoint
    // inside the expanded region.  This is composition glue, not a second
    // routing pass over the expanded graph.
    for (quotient_edge, mut points) in quotient_layout.edges {
        let Some(&original) = original_for_quotient.get(&quotient_edge) else {
            continue; // synthetic entry/exit terminal edge
        };
        let edge = graph.get_edge(original).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if child_for_proxy.contains_key(&entity_for_node[&from]) {
            let exits = &child_interfaces[&entity_for_node[&from]].exit_routes;
            let mut expanded = exits[&from].clone();
            valid &= join_equal_points(&mut expanded, &points);
            points = expanded;
        }
        if child_for_proxy.contains_key(&entity_for_node[&to]) {
            let entry = child_interfaces[&entity_for_node[&to]]
                .entry_route
                .as_ref()
                .unwrap();
            valid &= join_equal_points(&mut points, entry);
        }
        simplify_points(&mut points);
        edges.insert(original, points);
    }

    // Self-loops are deliberately outside every quotient graph, just as they
    // are in flat layout.  Emit them once at the root after all real positions
    // are known.
    if region_id == 0 {
        let mut loop_index = HashMap::<NodeId, u32>::default();
        let mut loop_count = HashMap::<NodeId, u32>::default();
        for edge in graph.edges() {
            if edge.from_id() == edge.to_id() && contained.contains(&edge.from_id()) {
                *loop_count.entry(edge.from_id()).or_default() += 1;
            }
        }
        for edge in graph.edges() {
            if edge.from_id() != edge.to_id() || !contained.contains(&edge.from_id()) {
                continue;
            }
            let node = nodes[&edge.from_id()];
            let index = loop_index.entry(node.id).or_default();
            edges.insert(
                edge.id(),
                self_loop_waypoints(
                    node.x,
                    node.y,
                    node.width,
                    node.height,
                    *index,
                    loop_count[&node.id],
                ),
            );
            *index += 1;
        }
    }

    // Proxy geometry is real content plus precisely one intentional margin.
    // Terminal ranks are routing-only: expose their paths through zero-area
    // perimeter ports rather than allowing them to enlarge this rectangle.
    let (content_min_x, content_max_x, content_min_y, content_max_y) = node_bounds(&nodes);
    let width = content_max_x - content_min_x + settings.node_gap;
    let height = content_max_y - content_min_y + settings.layer_gap;
    let center_x = (content_min_x + content_max_x) / 2.0;
    let center_y = (content_min_y + content_max_y) / 2.0;
    let top = center_y - height / 2.0;
    let bottom = center_y + height / 2.0;
    if let Some(points) = &mut entry_interface {
        // Replace the temporary terminal's top/bottom faces with the real
        // proxy attachment; retaining them would make an out-and-back detour.
        let terminal_x = points[0].x;
        let port_x = terminal_x.clamp(center_x - width / 2.0, center_x + width / 2.0);
        points.drain(..2);
        points.insert(0, Point { x: port_x, y: top });
    }
    let mut exit_sources: Vec<_> = exit_interfaces.keys().copied().collect();
    exit_sources.sort();
    let exit_count = exit_sources.len();
    let mut used_exit_x = Vec::new();
    for source in exit_sources {
        let points = exit_interfaces.get_mut(&source).unwrap();
        // The final two points are the temporary terminal's top/bottom faces.
        let terminal_x = points[points.len() - 2].x;
        let mut port_x = terminal_x.clamp(center_x - width / 2.0, center_x + width / 2.0);
        if used_exit_x.iter().any(|x: &f64| (*x - port_x).abs() < 1e-6) {
            let step = width / (exit_count + 1) as f64;
            port_x = (center_x - width / 2.0 + step * (used_exit_x.len() + 1) as f64)
                .clamp(center_x - width / 2.0, center_x + width / 2.0);
        }
        used_exit_x.push(port_x);
        points.truncate(points.len() - 2);
        points.push(Point {
            x: port_x,
            y: bottom,
        });
    }
    for node in nodes.values_mut() {
        node.x -= center_x;
        node.y -= center_y;
    }
    for points in edges.values_mut() {
        translate_points_in_place(points, -center_x, -center_y);
    }
    for region_box in &mut region_boxes {
        region_box.min_x -= center_x;
        region_box.max_x -= center_x;
        region_box.min_y -= center_y;
        region_box.max_y -= center_y;
    }
    region_boxes.push(RegionBox {
        id: region_id,
        parent: region.parent,
        min_x: content_min_x - center_x - settings.node_gap / 2.0,
        max_x: content_max_x - center_x + settings.node_gap / 2.0,
        min_y: content_min_y - center_y - settings.layer_gap / 2.0,
        max_y: content_max_y - center_y + settings.layer_gap / 2.0,
    });
    if let Some(points) = &mut entry_interface {
        translate_points_in_place(points, -center_x, -center_y);
    }
    for points in exit_interfaces.values_mut() {
        translate_points_in_place(points, -center_x, -center_y);
    }
    let mut ports = BTreeMap::new();
    if let Some(route) = &entry_interface {
        let point = route[0];
        valid &= (point.y + height / 2.0).abs() < 1e-6;
        ports.insert(
            RegionPortId::Entry,
            ProxyPort {
                id: RegionPortId::Entry,
                x_offset: point.x,
                is_entry: true,
            },
        );
    }
    for (&source, route) in &exit_interfaces {
        let point = *route.last().unwrap();
        valid &= (point.y - height / 2.0).abs() < 1e-6;
        ports.insert(
            RegionPortId::Exit { source },
            ProxyPort {
                id: RegionPortId::Exit { source },
                x_offset: point.x,
                is_entry: false,
            },
        );
    }
    RegionComposition {
        nodes,
        edges,
        interface: RegionInterface {
            width,
            height,
            ports,
            entry_route: entry_interface,
            exit_routes: exit_interfaces,
        },
        valid,
        region_boxes,
    }
}

fn translate_points(mut points: Vec<Point>, dx: f64, dy: f64) -> Vec<Point> {
    translate_points_in_place(&mut points, dx, dy);
    points
}

fn translate_points_in_place(points: &mut [Point], dx: f64, dy: f64) {
    for point in points {
        point.x += dx;
        point.y += dy;
    }
}

/// Concatenate routes at a router-native proxy port. A mismatch means a
/// caller failed to carry the fixed port through the ordinary layout pipeline;
/// inventing an elbow here would hide that error and violate SESE composition.
fn join_equal_points(points: &mut Vec<Point>, suffix: &[Point]) -> bool {
    const EPS: f64 = 1e-6;
    let last = *points.last().expect("non-empty route");
    let first = *suffix.first().expect("non-empty route");
    let equal = (last.x - first.x).abs() <= EPS && (last.y - first.y).abs() <= EPS;
    if equal {
        points.extend(suffix.iter().copied().skip(1));
    }
    equal
}

fn simplify_points(points: &mut Vec<Point>) {
    let mut simplified = Vec::with_capacity(points.len());
    for point in points.drain(..) {
        if simplified.last() != Some(&point) {
            simplified.push(point);
        }
    }
    *points = simplified;
}

fn routes_clear_of_nodes<NodeId: Identifier, EdgeId: Identifier>(
    nodes: &HashMap<NodeId, LayoutNode<NodeId>>,
    edges: &HashMap<EdgeId, Vec<Point>>,
) -> bool {
    edges.values().all(|points| {
        points.windows(2).all(|segment| {
            nodes.values().all(|node| {
                !crate::triskel::geometry::segment_enters_rect_strict(
                    segment[0],
                    segment[1],
                    Point {
                        x: node.x,
                        y: node.y,
                    },
                    node.width,
                    node.height,
                )
            })
        })
    })
}

fn routes_are_orthogonal<EdgeId: Identifier>(edges: &HashMap<EdgeId, Vec<Point>>) -> bool {
    edges.values().all(|points| {
        points.windows(2).all(|segment| {
            (segment[0].x - segment[1].x).abs() < 1e-6 || (segment[0].y - segment[1].y).abs() < 1e-6
        })
    })
}

fn routes_are_deconflicted<EdgeId: Identifier>(edges: &HashMap<EdgeId, Vec<Point>>) -> bool {
    let routes: Vec<_> = edges.values().collect();
    for (index, route) in routes.iter().enumerate() {
        for other in routes.iter().skip(index + 1) {
            if routes_have_collinear_overlap(route, other) {
                return false;
            }
        }
    }
    true
}

fn routes_have_collinear_overlap(a: &[Point], b: &[Point]) -> bool {
    const EPS: f64 = 1e-6;
    a.windows(2).any(|lhs| {
        b.windows(2).any(|rhs| {
            let lhs_vertical = (lhs[0].x - lhs[1].x).abs() < EPS;
            let rhs_vertical = (rhs[0].x - rhs[1].x).abs() < EPS;
            if lhs_vertical && rhs_vertical && (lhs[0].x - rhs[0].x).abs() < EPS {
                lhs[0].y.min(lhs[1].y).max(rhs[0].y.min(rhs[1].y)) + EPS
                    < lhs[0].y.max(lhs[1].y).min(rhs[0].y.max(rhs[1].y))
            } else if !lhs_vertical && !rhs_vertical && (lhs[0].y - rhs[0].y).abs() < EPS {
                lhs[0].x.min(lhs[1].x).max(rhs[0].x.min(rhs[1].x)) + EPS
                    < lhs[0].x.max(lhs[1].x).min(rhs[0].x.max(rhs[1].x))
            } else {
                false
            }
        })
    })
}

fn node_bounds<NodeId: Identifier>(
    nodes: &HashMap<NodeId, LayoutNode<NodeId>>,
) -> (f64, f64, f64, f64) {
    let mut bounds = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for node in nodes.values() {
        bounds.0 = bounds.0.min(node.x - node.width / 2.0);
        bounds.1 = bounds.1.max(node.x + node.width / 2.0);
        bounds.2 = bounds.2.min(node.y - node.height / 2.0);
        bounds.3 = bounds.3.max(node.y + node.height / 2.0);
    }
    bounds
}

fn layout_component<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    geometry: &HashMap<NodeId, NodeGeometry>,
    preferred_root: Option<NodeId>,
    settings: &LayoutSettings,
) -> ComponentLayout<NodeId, EdgeId>
where
    NodeId: Identifier + Debug,
    EdgeId: Identifier + Debug,
{
    layout_component_with_port_hints(
        graph,
        component,
        geometry,
        preferred_root,
        settings,
        &HashMap::default(),
    )
}

fn layout_component_with_port_hints<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    component: &[NodeId],
    geometry: &HashMap<NodeId, NodeGeometry>,
    preferred_root: Option<NodeId>,
    settings: &LayoutSettings,
    hints: &HashMap<EdgeId, PortHint>,
) -> ComponentLayout<NodeId, EdgeId>
where
    NodeId: Identifier + Debug,
    EdgeId: Identifier + Debug,
{
    // Build an internal usize-id graph for this component, recording the
    // mapping back to the caller's node/edge ids.
    let mut local = LayoutGraph::default();
    let mut to_local: HashMap<NodeId, usize> = HashMap::default();
    let mut to_orig: HashMap<usize, NodeId> = HashMap::default();

    let mut sorted: Vec<NodeId> = component.to_vec();
    sorted.sort_by_key(|id| Into::<usize>::into(*id));
    for id in &sorted {
        let geom = geometry[id];
        let local_id = local.make_node(NodeLayoutData {
            width: geom.width,
            height: geom.height,
            ..Default::default()
        });
        to_local.insert(*id, local_id);
        to_orig.insert(local_id, *id);
    }

    let in_component: HashSet<NodeId> = component.iter().copied().collect();
    // Self-loops are kept out of the layered graph (they would be degenerate
    // rank-0 cycles); instead each is drawn as a loop off its node's right face,
    // recorded here as (original edge id, local node id) in edge-id order.
    let mut self_loops: Vec<(EdgeId, usize)> = Vec::new();
    let mut edge_ids: Vec<EdgeId> = graph.edges().map(|e| e.id()).collect();
    edge_ids.sort_by_key(|id| Into::<usize>::into(*id));
    for edge_id in edge_ids {
        let edge = graph.get_edge(edge_id).unwrap();
        let from = edge.from_id();
        let to = edge.to_id();
        if !in_component.contains(&from) {
            continue; // cross-component edges impossible
        }
        if from == to {
            let node = to_local[&from];
            local.get_node_mut(node).unwrap().self_loops += 1;
            self_loops.push((edge_id, node));
            continue;
        }
        local.make_edge(
            to_local[&from],
            to_local[&to],
            EdgeLayoutData {
                orig: edge_id.into(),
                port_start: hints.get(&edge_id).and_then(|hint| hint.source_bottom),
                port_end: hints.get(&edge_id).and_then(|hint| hint.target_top),
                ..Default::default()
            },
        );
    }

    // Pick a deterministic root: the preferred root if it lives here, else the
    // component's min-id node.
    let root_local = preferred_root
        .filter(|r| in_component.contains(r))
        .map(|r| to_local[&r])
        .unwrap_or_else(|| to_local[&sorted[0]]);

    cycle::break_cycles(&mut local, root_local);
    rank::assign_ranks(&mut local);
    let seg = segment::build_segments(&mut local);
    let ordering = order::order(&mut local, &seg, root_local, settings.max_sweeps);
    // This is the ordering→coordinates phase contract.  The generated layout
    // properties execute the real cycle/rank/segment/order pipeline and check
    // that no mandatory p/lane/q equality can contradict slot separation.
    #[cfg(test)]
    assert!(coordinate::mandatory_constraints_feasible(
        &local, &seg, &ordering
    ));
    coordinate::assign_x(&mut local, &seg, &ordering, settings.node_gap);
    assign_y(&mut local, &ordering.layers, settings.layer_gap);

    let waypoints = match settings.edge_style {
        EdgeStyle::Orthogonal => OrthogonalRouter.route(&local, &ordering.layers),
        EdgeStyle::Straight => StraightRouter.route(&local, &ordering.layers),
    };

    let mut nodes = HashMap::default();
    for local_id in to_orig.keys().copied() {
        let node = local.get_node(local_id).unwrap();
        let orig = to_orig[&local_id];
        nodes.insert(
            orig,
            LayoutNode {
                id: orig,
                x: node.x,
                y: node.y,
                width: node.width,
                height: node.height,
            },
        );
    }

    let mut edges = HashMap::default();
    for (orig, points) in waypoints {
        edges.insert(EdgeId::from(orig), points);
    }

    // Lay out the self-loops over their node's reserved right margin. Multiple
    // loops on one node nest, drawn in stable edge-id order.
    let mut loop_index: HashMap<usize, u32> = HashMap::default();
    for (edge_id, node_id) in self_loops {
        let node = local.get_node(node_id).unwrap();
        let total = node.self_loops;
        let index = loop_index.entry(node_id).or_insert(0);
        let points = self_loop_waypoints(node.x, node.y, node.width, node.height, *index, total);
        *index += 1;
        edges.insert(edge_id, points);
    }

    ComponentLayout {
        nodes,
        edges,
        regions: Vec::new(),
    }
}

/// Assigns each node a y by rank, using the tallest node in each rank so bends
/// never fall inside a neighbouring rank's bounding box.
fn assign_y(graph: &mut LayoutGraph, layers: &[Vec<usize>], layer_gap: f64) {
    let mut max_h = vec![0.0f64; layers.len()];
    for (rank, nodes) in layers.iter().enumerate() {
        for &id in nodes {
            max_h[rank] = max_h[rank].max(graph.get_node(id).unwrap().height);
        }
    }

    let mut centers = vec![0.0f64; layers.len()];
    for rank in 1..layers.len() {
        centers[rank] = centers[rank - 1] + (max_h[rank - 1] + max_h[rank]) / 2.0 + layer_gap;
    }

    for (rank, nodes) in layers.iter().enumerate() {
        for &id in nodes {
            graph.get_node_mut(id).unwrap().y = centers[rank];
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn weakly_connected_components<NodeId, EdgeId, NodeData, EdgeData>(
    graph: &OwningGraph<NodeId, EdgeId, NodeData, EdgeData>,
    sorted_ids: &[NodeId],
) -> Vec<Vec<NodeId>>
where
    NodeId: Identifier + Debug,
    EdgeId: Identifier + Debug,
{
    let mut seen: HashSet<NodeId> = HashSet::default();
    let mut components = Vec::new();
    for &start in sorted_ids {
        if seen.contains(&start) {
            continue;
        }
        let mut component: Vec<NodeId> = graph
            .undirected_dfs(start)
            .map(|(_, node)| node.id())
            .collect();
        component.sort_by_key(|id| Into::<usize>::into(*id));
        for id in &component {
            seen.insert(*id);
        }
        components.push(component);
    }
    components
}

fn local_bounds<NodeId: Identifier, EdgeId: Identifier>(
    layout: &ComponentLayout<NodeId, EdgeId>,
) -> (f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;

    for node in layout.nodes.values() {
        min_x = min_x.min(node.x - node.width / 2.0);
        max_x = max_x.max(node.x + node.width / 2.0);
        min_y = min_y.min(node.y - node.height / 2.0);
    }
    for point in layout.edges.values().flatten() {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
    }

    if !min_x.is_finite() {
        return (0.0, 0.0, 0.0);
    }
    (min_x, max_x, min_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jstd_derive::Identifier;
    use proptest::prelude::*;

    use crate::triskel::geometry::segment_enters_rect_strict;

    #[derive(Identifier)]
    struct N(usize);
    #[derive(Identifier)]
    struct E(usize);
    type G = OwningGraph<N, E, (), ()>;

    fn geom() -> impl FnMut(N) -> NodeGeometry {
        |_| NodeGeometry {
            width: 60.0,
            height: 30.0,
        }
    }

    fn layout(graph: &G, root: N) -> LayoutResult<N, E> {
        LayoutBuilder::new(graph)
            .root(root)
            .geometry(geom())
            .build()
            .expect("graph should lay out")
    }

    fn diamond() -> (G, N) {
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(a, c, ());
        g.make_edge(b, d, ());
        g.make_edge(c, d, ());
        (g, a)
    }

    fn signature(result: &LayoutResult<N, E>) -> Vec<String> {
        let mut nodes: Vec<_> = result.nodes.values().collect();
        nodes.sort_by_key(|n| usize::from(n.id));
        let mut lines: Vec<String> = nodes
            .iter()
            .map(|n| format!("n{}:{:.3},{:.3}", usize::from(n.id), n.x, n.y))
            .collect();
        let mut edges: Vec<_> = result.edges.iter().collect();
        edges.sort_by_key(|(e, _)| usize::from(**e));
        for (e, pts) in edges {
            let s = pts
                .iter()
                .map(|p| format!("{:.3},{:.3}", p.x, p.y))
                .collect::<Vec<_>>()
                .join(";");
            lines.push(format!("e{}:{s}", usize::from(*e)));
        }
        lines
    }

    fn assert_no_node_overlap(result: &LayoutResult<N, E>) {
        let nodes: Vec<_> = result.nodes.values().collect();
        for (i, a) in nodes.iter().enumerate() {
            for b in nodes.iter().skip(i + 1) {
                let sep_x = (a.x - b.x).abs() + 1e-6 >= (a.width + b.width) / 2.0;
                let sep_y = (a.y - b.y).abs() + 1e-6 >= (a.height + b.height) / 2.0;
                assert!(
                    sep_x || sep_y,
                    "nodes {} and {} overlap at ({:.2},{:.2}) / ({:.2},{:.2})",
                    usize::from(a.id),
                    usize::from(b.id),
                    a.x,
                    a.y,
                    b.x,
                    b.y
                );
            }
        }
    }

    fn assert_no_edge_through_node(result: &LayoutResult<N, E>) {
        for (edge_id, points) in &result.edges {
            for (index, seg) in points.windows(2).enumerate() {
                for node in result.nodes.values() {
                    assert!(
                        !segment_enters_rect_strict(
                            seg[0],
                            seg[1],
                            Point {
                                x: node.x,
                                y: node.y
                            },
                            node.width,
                            node.height,
                        ),
                        "edge {} segment {index} passes through node {}",
                        usize::from(*edge_id),
                        usize::from(node.id)
                    );
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "segment 1 passes through node 0")]
    fn edge_checker_rejects_later_reentry_into_endpoint_node() {
        let source = N::from(0);
        let target = N::from(1);
        let edge = E::from(0);
        let mut nodes = HashMap::default();
        nodes.insert(
            source,
            LayoutNode {
                id: source,
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        );
        nodes.insert(
            target,
            LayoutNode {
                id: target,
                x: 20.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        );
        let mut edges = HashMap::default();
        // The first point validly touches source's bottom boundary. Segment 1
        // then re-enters its interior, which the former whole-polyline
        // endpoint exemption incorrectly accepted.
        edges.insert(
            edge,
            vec![
                Point { x: 0.0, y: 5.0 },
                Point { x: 0.0, y: 10.0 },
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
            ],
        );
        assert_no_edge_through_node(&LayoutResult {
            nodes,
            edges,
            regions: Vec::new(),
        });
    }

    /// Rejects every edge intersection with a node that is not an endpoint of
    /// that edge. Endpoint interiors are checked separately because a route may
    /// validly begin/end on their boundaries.
    fn assert_route_attaches_to_endpoints(
        result: &LayoutResult<N, E>,
        endpoints: &HashMap<E, (N, N)>,
    ) {
        let eps = 1e-6;
        for (edge, points) in &result.edges {
            let &(source, target) = endpoints.get(edge).expect("missing edge endpoint data");
            for (point, node) in [
                (points.first().unwrap(), result.nodes[&source]),
                (points.last().unwrap(), result.nodes[&target]),
            ] {
                let on_vertical_face = (point.x - (node.x - node.width / 2.0)).abs() < eps
                    || (point.x - (node.x + node.width / 2.0)).abs() < eps;
                let on_horizontal_face = (point.y - (node.y - node.height / 2.0)).abs() < eps
                    || (point.y - (node.y + node.height / 2.0)).abs() < eps;
                assert!(
                    (on_vertical_face
                        && point.y >= node.y - node.height / 2.0 - eps
                        && point.y <= node.y + node.height / 2.0 + eps)
                        || (on_horizontal_face
                            && point.x >= node.x - node.width / 2.0 - eps
                            && point.x <= node.x + node.width / 2.0 + eps),
                    "edge {} does not attach to node {}: {point:?} vs {node:?}",
                    usize::from(*edge),
                    usize::from(node.id),
                );
            }
        }
    }

    fn assert_no_edge_through_nonincident_node(
        result: &LayoutResult<N, E>,
        endpoints: &HashMap<E, (N, N)>,
    ) {
        for (edge_id, points) in &result.edges {
            let &(source, target) = endpoints.get(edge_id).expect("missing edge endpoint data");
            for seg in points.windows(2) {
                for node in result.nodes.values() {
                    if node.id == source || node.id == target {
                        continue;
                    }
                    assert!(
                        !segment_enters_rect_strict(
                            seg[0],
                            seg[1],
                            Point {
                                x: node.x,
                                y: node.y
                            },
                            node.width,
                            node.height,
                        ),
                        "edge {} passes through non-incident node {}",
                        usize::from(*edge_id),
                        usize::from(node.id)
                    );
                }
            }
        }
    }

    /// No two horizontal edge segments (from distinct edges) may share a y while
    /// their x-ranges overlap — that is the overlap the lane assignment removes.
    fn assert_no_horizontal_overlap(result: &LayoutResult<N, E>) {
        let eps = 1e-6;
        // (edge id, y, x0, x1) for every horizontal segment.
        let mut hsegs: Vec<(usize, f64, f64, f64)> = Vec::new();
        for (edge_id, points) in &result.edges {
            for seg in points.windows(2) {
                let (p, q) = (seg[0], seg[1]);
                if (p.y - q.y).abs() < eps && (p.x - q.x).abs() > eps {
                    hsegs.push((usize::from(*edge_id), p.y, p.x.min(q.x), p.x.max(q.x)));
                }
            }
        }
        for (i, &(ea, ya, ax0, ax1)) in hsegs.iter().enumerate() {
            for &(eb, yb, bx0, bx1) in hsegs.iter().skip(i + 1) {
                if ea == eb || (ya - yb).abs() > eps {
                    continue;
                }
                let overlap = ax0.max(bx0) + eps < ax1.min(bx1);
                assert!(
                    !overlap,
                    "edges {ea} and {eb} have overlapping horizontal segments at y={ya:.2}: \
                     [{ax0:.2},{ax1:.2}] vs [{bx0:.2},{bx1:.2}]"
                );
            }
        }
    }

    /// No two vertical edge segments (from distinct edges) may share an x while
    /// their y-ranges overlap. This is the vertical counterpart of the channel
    /// lane check above and catches accidentally bundled dummy columns.
    fn assert_no_vertical_overlap(result: &LayoutResult<N, E>) {
        let eps = 1e-6;
        let mut vsegs: Vec<(usize, f64, f64, f64)> = Vec::new();
        for (edge_id, points) in &result.edges {
            for seg in points.windows(2) {
                let (p, q) = (seg[0], seg[1]);
                if (p.x - q.x).abs() < eps && (p.y - q.y).abs() > eps {
                    vsegs.push((usize::from(*edge_id), p.x, p.y.min(q.y), p.y.max(q.y)));
                }
            }
        }
        for (i, &(ea, xa, ay0, ay1)) in vsegs.iter().enumerate() {
            for &(eb, xb, by0, by1) in vsegs.iter().skip(i + 1) {
                if ea == eb || (xa - xb).abs() > eps {
                    continue;
                }
                let overlap = ay0.max(by0) + eps < ay1.min(by1);
                assert!(
                    !overlap,
                    "edges {ea} and {eb} have overlapping vertical segments at x={xa:.2}: \
                     [{ay0:.2},{ay1:.2}] vs [{by0:.2},{by1:.2}]"
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn generated_segmented_layouts_preserve_phase_and_route_invariants(
            // Every generated graph includes the fixture below, which forces
            // simultaneous/nested long segments, a cycle/back-edge gadget and
            // a self-loop reservation through the real pipeline.
            node_count in 6usize..=10,
            edge_pairs in proptest::collection::vec((0usize..20, 0usize..20), 0..24),
            dimensions in proptest::collection::vec((10u32..=140, 10u32..=80), 6..=10),
            node_gap in 8u32..=40,
            layer_gap in 16u32..=80,
            sweeps in 1usize..=6,
            orthogonal in any::<bool>(),
            sese in any::<bool>(),
        ) {
            let mut graph = G::default();
            let nodes: Vec<_> = (0..node_count).map(|_| graph.make_node(())).collect();
            let mut endpoints = HashMap::default();
            for (from, to) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (0, 5), (1, 4), (5, 0), (2, 2)]
                .into_iter()
                .chain(edge_pairs)
            {
                let (from, to) = (nodes[from % node_count], nodes[to % node_count]);
                let edge = graph.make_edge(from, to, ());
                endpoints.insert(edge, (from, to));
            }
            let geometry = dimensions.clone();
            let build = || LayoutBuilder::new(&graph)
                .root(nodes[0])
                .node_gap(node_gap as f64)
                .layer_gap(layer_gap as f64)
                .max_sweeps(sweeps)
                .edge_style(if orthogonal { EdgeStyle::Orthogonal } else { EdgeStyle::Straight })
                .mode(if sese { LayoutMode::Sese } else { LayoutMode::Flat })
                .geometry(|id| {
                    let (width, height) = geometry[usize::from(id) % geometry.len()];
                    NodeGeometry { width: width as f64, height: height as f64 }
                })
                .build()
                .unwrap();
            let first = build();
            let second = build();
            prop_assert_eq!(signature(&first), signature(&second));
            prop_assert_eq!(first.nodes.len(), node_count);
            prop_assert_eq!(first.edges.len(), graph.edges().count());
            for node in first.nodes.values() {
                prop_assert!(node.x.is_finite() && node.y.is_finite());
                prop_assert!(node.width.is_finite() && node.height.is_finite());
                prop_assert!(node.width > 0.0 && node.height > 0.0);
            }
            for points in first.edges.values() {
                prop_assert!(points.len() >= 2);
                for pair in points.windows(2) {
                    prop_assert!(pair[0].x.is_finite() && pair[0].y.is_finite());
                    prop_assert!(pair[1].x.is_finite() && pair[1].y.is_finite());
                    prop_assert!((pair[0].x - pair[1].x).abs() > 1e-9 || (pair[0].y - pair[1].y).abs() > 1e-9);
                    if orthogonal {
                        prop_assert!((pair[0].x - pair[1].x).abs() < 1e-6 || (pair[0].y - pair[1].y).abs() < 1e-6);
                    }
                }
            }
            assert_no_node_overlap(&first);
            assert_no_edge_through_node(&first);
            assert_no_edge_through_nonincident_node(&first, &endpoints);
            // Mixed Straight output can contain fallback orthogonal segments,
            // so collinear-overlap checks apply even when the requested style
            // is not fully orthogonal.
            if !orthogonal {
                assert_no_horizontal_overlap(&first);
                assert_no_vertical_overlap(&first);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        #[ignore = "run explicitly as cargo test triskel_stress_generated_layouts -- --ignored"]
        fn triskel_stress_generated_layouts(
            node_count in 2usize..=10,
            edge_pairs in proptest::collection::vec((0usize..20, 0usize..20), 1..24),
            dimensions in proptest::collection::vec((10u32..=140, 10u32..=80), 2..=10),
            root_index in 0usize..24,
            orthogonal in any::<bool>(),
            sese in any::<bool>(),
        ) {
            // This deliberately uses proptest rather than seed enumeration, so
            // failures shrink to an edge list, root, dimensions, and style.
            let mut graph = G::default();
            let nodes: Vec<_> = (0..node_count).map(|_| graph.make_node(())).collect();
            let mut endpoints = HashMap::default();
            for (from, to) in edge_pairs {
                let edge = graph.make_edge(nodes[from % node_count], nodes[to % node_count], ());
                endpoints.insert(edge, (nodes[from % node_count], nodes[to % node_count]));
            }
            let geometry = dimensions.clone();
            let result = LayoutBuilder::new(&graph)
                .root(nodes[root_index % node_count])
                .max_sweeps(8)
                .edge_style(if orthogonal { EdgeStyle::Orthogonal } else { EdgeStyle::Straight })
                .mode(if sese { LayoutMode::Sese } else { LayoutMode::Flat })
                .geometry(|id| {
                    let (width, height) = geometry[usize::from(id) % geometry.len()];
                    NodeGeometry { width: width as f64, height: height as f64 }
                })
                .build()
                .unwrap();
            prop_assert_eq!(result.edges.len(), graph.edges().count());
            assert_no_node_overlap(&result);
            assert_no_edge_through_node(&result);
            assert_no_edge_through_nonincident_node(&result, &endpoints);
        }
    }

    #[test]
    fn crossing_edges_use_separate_horizontal_lanes() {
        // a,b on rank 0; c,d on rank 1. a->d and b->c cross, so both jog across
        // the same channel with overlapping x-ranges. They must land on
        // different y-lanes rather than drawing over each other.
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, c, ());
        g.make_edge(a, d, ());
        g.make_edge(b, c, ());
        g.make_edge(b, d, ());
        let r = layout(&g, a);
        assert_no_horizontal_overlap(&r);
        assert_no_edge_through_node(&r);
        assert_no_node_overlap(&r);
    }

    #[test]
    fn non_overlapping_jogs_share_the_channel_midpoint() {
        // In the diamond, a's two out-edges jog in channel 0 toward b (left) and
        // c (right); their x-ranges do not overlap, so both keep lane 0 — the
        // exact channel midpoint — confirming the lane pass is a no-op when there
        // is nothing to deconflict.
        let (g, root) = diamond();
        let r = layout(&g, root);
        let a = r.get_node(root).unwrap();
        let any_child = r.nodes.values().find(|n| n.y > a.y).unwrap();
        let midpoint = ((a.y + a.height / 2.0) + (any_child.y - any_child.height / 2.0)) / 2.0;
        for points in r.edges.values() {
            for seg in points.windows(2) {
                let (p, q) = (seg[0], seg[1]);
                let horizontal = (p.y - q.y).abs() < 1e-9 && (p.x - q.x).abs() > 1e-9;
                // Only the top channel (a → b/c) sits at this midpoint.
                if horizontal && (p.y - midpoint).abs() < 5.0 {
                    assert!(
                        (p.y - midpoint).abs() < 1e-6,
                        "jog y {:.3} should equal channel midpoint {:.3}",
                        p.y,
                        midpoint
                    );
                }
            }
        }
        assert_no_horizontal_overlap(&r);
        assert_no_vertical_overlap(&r);
    }

    #[test]
    fn is_deterministic() {
        let (g, root) = diamond();
        let first = signature(&layout(&g, root));
        for _ in 0..10 {
            assert_eq!(first, signature(&layout(&g, root)));
        }
    }

    #[test]
    fn chain_ranks_increase_downward() {
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(b, c, ());
        let r = layout(&g, a);
        let ay = r.get_node(a).unwrap().y;
        let by = r.get_node(b).unwrap().y;
        let cy = r.get_node(c).unwrap().y;
        assert!(ay < by && by < cy, "{ay} {by} {cy}");
    }

    #[test]
    fn diamond_has_no_overlap_and_clean_edges() {
        let (g, root) = diamond();
        let r = layout(&g, root);
        assert_no_node_overlap(&r);
        assert_no_edge_through_node(&r);
        for points in r.edges.values() {
            assert!(points.len() >= 2);
        }
    }

    #[test]
    fn long_edge_spans_with_dummies_and_renders_once() {
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(b, c, ());
        g.make_edge(c, d, ());
        let long = g.make_edge(a, d, ());
        let r = layout(&g, a);
        assert!(r.get_waypoints(long).unwrap().len() >= 2);
        let svg = r.render_svg();
        let marker = format!("data-edge-id=\"{}\"", usize::from(long));
        assert_eq!(svg.matches(&marker).count(), 1);
    }

    #[test]
    fn orthogonal_router_yields_right_angles() {
        let (g, root) = diamond();
        let r = layout(&g, root);
        for points in r.edges.values() {
            for seg in points.windows(2) {
                let axis_aligned =
                    (seg[0].x - seg[1].x).abs() < 1e-6 || (seg[0].y - seg[1].y).abs() < 1e-6;
                assert!(
                    axis_aligned,
                    "non-orthogonal segment {:?}->{:?}",
                    seg[0], seg[1]
                );
            }
        }
    }

    #[test]
    fn back_edge_runs_upward_to_target() {
        // a -> b -> c -> a : the back edge c->a should be drawn source(c)->target(a)
        // travelling upward (first point lower than last).
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(b, c, ());
        let back = g.make_edge(c, a, ());
        let r = layout(&g, a);
        let pts = r.get_waypoints(back).unwrap();
        assert!(
            pts.first().unwrap().y > pts.last().unwrap().y,
            "back edge should travel upward: {:?}",
            pts
        );

        // It wraps around the end blocks: leaving the source (c) from its bottom
        // and entering the target (a) at its top, like every forward edge.
        let eps = 1e-6;
        let src = r.get_node(c).unwrap();
        let tgt = r.get_node(a).unwrap();
        let start = pts.first().unwrap();
        let end = pts.last().unwrap();
        assert!(
            (start.y - (src.y + src.height / 2.0)).abs() < eps,
            "back edge should start at the source's bottom: {start:?} vs c={src:?}"
        );
        assert!(
            (end.y - (tgt.y - tgt.height / 2.0)).abs() < eps,
            "back edge should end at the target's top: {end:?} vs a={tgt:?}"
        );
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn adjacent_back_edge_wraps_without_dummies() {
        // a <-> b : the back edge b->a is one rank apart, so it has no dummy
        // column to reuse. It must still exit b's bottom and enter a's top,
        // wrapping around the side, and pass through no node.
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        g.make_edge(a, b, ());
        let back = g.make_edge(b, a, ());
        let r = layout(&g, a);
        let pts = r.get_waypoints(back).unwrap();
        let eps = 1e-6;
        let src = r.get_node(b).unwrap();
        let tgt = r.get_node(a).unwrap();
        let start = pts.first().unwrap();
        let end = pts.last().unwrap();
        assert!(
            (start.y - (src.y + src.height / 2.0)).abs() < eps,
            "back edge should start at the source's bottom: {start:?} vs b={src:?}"
        );
        assert!(
            (end.y - (tgt.y - tgt.height / 2.0)).abs() < eps,
            "back edge should end at the target's top: {end:?} vs a={tgt:?}"
        );
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn back_edge_keeps_source_port_on_the_bottom_fan() {
        // top -> s, s -> a, s -> b, and the back edge s -> top. All three edges
        // leaving s (two forward, one back) must fan across s's BOTTOM face — the
        // back edge counts as an exit, not as if s still had one fewer.
        let mut g = G::default();
        let top = g.make_node(());
        let s = g.make_node(());
        let a = g.make_node(());
        let b = g.make_node(());
        g.make_edge(top, s, ());
        let e_a = g.make_edge(s, a, ());
        let e_b = g.make_edge(s, b, ());
        let e_back = g.make_edge(s, top, ()); // back edge: top is s's ancestor
        let r = layout(&g, top);

        let sn = r.get_node(s).unwrap();
        let bottom_y = sn.y + sn.height / 2.0;
        let eps = 1e-6;

        // Each edge's endpoint that touches s is the one at s's bottom face. For
        // forward edges that's the first point; for the reversed edge the polyline
        // runs s -> top, so it is also the first point (s is its source).
        let port_x = |edge| {
            let pts = r.get_waypoints(edge).unwrap();
            let p = pts.first().unwrap();
            assert!(
                (p.y - bottom_y).abs() < eps,
                "edge should leave s's bottom: {p:?} vs y={bottom_y}"
            );
            p.x
        };
        let mut xs = [port_x(e_a), port_x(e_b), port_x(e_back)];
        xs.sort_by(f64::total_cmp);

        // The bottom face fans three edges across three slots — one each in the
        // left, middle and right third of s's width. Under the old (broken)
        // assignment the back edge would have counted on the TOP face, leaving
        // only a two-slot fan and the back edge starting at s's top instead.
        let half = sn.width / 2.0;
        assert!(
            xs[0] < sn.x - half / 4.0
                && (xs[1] - sn.x).abs() <= half / 2.0
                && xs[2] > sn.x + half / 4.0,
            "the three exits should fan across s's bottom in distinct slots: {xs:?} (s.x={})",
            sn.x
        );
        for p in xs {
            assert!(
                p >= sn.x - half - eps && p <= sn.x + half + eps,
                "port {p} outside s's width [{}, {}]",
                sn.x - half,
                sn.x + half
            );
        }
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn multiple_back_edges_into_one_node_wrap_cleanly() {
        // a -> b, b -> c, b -> d, c -> a, d -> a : two back edges (c->a and d->a)
        // both target a. Each must wrap into a's top through its own reserved
        // lane, leaving its source's bottom, and clip no node.
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(b, c, ());
        g.make_edge(b, d, ());
        let back_c = g.make_edge(c, a, ());
        let back_d = g.make_edge(d, a, ());
        let r = layout(&g, a);

        let eps = 1e-6;
        let tgt = r.get_node(a).unwrap();
        let mut top_ports = Vec::new();
        for (src_id, back) in [(c, back_c), (d, back_d)] {
            let pts = r.get_waypoints(back).unwrap();
            let src = r.get_node(src_id).unwrap();
            let start = pts.first().unwrap();
            let end = pts.last().unwrap();
            assert!(
                (start.y - (src.y + src.height / 2.0)).abs() < eps,
                "back edge should leave the source's bottom: {start:?}"
            );
            assert!(
                (end.y - (tgt.y - tgt.height / 2.0)).abs() < eps,
                "back edge should enter the target's top: {end:?}"
            );
            top_ports.push(end.x);
        }
        assert!(
            (top_ports[0] - top_ports[1]).abs() > eps,
            "the two back edges must enter a's top at distinct ports: {top_ports:?}"
        );
        assert_no_edge_through_node(&r);
        assert_no_node_overlap(&r);
    }

    #[test]
    fn long_back_edge_has_a_straight_column() {
        // a -> b -> c -> d -> e with the back edge e -> a spanning every rank.
        // Its gadget should draw as a single straight vertical column with one
        // jog at each end: source bottom -> down -> column -> up -> top jog ->
        // target top (six points).
        let mut g = G::default();
        let n: Vec<_> = (0..5).map(|_| g.make_node(())).collect();
        for w in n.windows(2) {
            g.make_edge(w[0], w[1], ());
        }
        let back = g.make_edge(n[4], n[0], ());
        let r = layout(&g, n[0]);
        let pts = r.get_waypoints(back).unwrap();
        assert_eq!(
            pts.len(),
            6,
            "a single long back edge should wrap with a straight column: {pts:?}"
        );
        // Points 2..3 are the column: vertical and spanning the ranks.
        assert!(
            (pts[2].x - pts[3].x).abs() < 1e-6,
            "the column must be vertical: {pts:?}"
        );
        assert!(
            (pts[2].y - pts[3].y).abs() > r.get_node(n[0]).unwrap().height,
            "the column must span the ranks: {pts:?}"
        );
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn self_loop_is_rendered_off_the_right_face() {
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let loop_edge = g.make_edge(a, a, ());
        g.make_edge(a, b, ());
        let r = layout(&g, a);
        assert_eq!(r.nodes.len(), 2);

        // The loop is a polyline that leaves and re-enters a's right face.
        let an = r.get_node(a).unwrap();
        let pts = r.get_waypoints(loop_edge).expect("self-loop must render");
        assert!(
            pts.len() >= 4,
            "self-loop should be a loop polyline: {pts:?}"
        );
        let right = an.x + an.width / 2.0;
        let eps = 1e-6;
        assert!(
            (pts.first().unwrap().x - right).abs() < eps
                && (pts.last().unwrap().x - right).abs() < eps,
            "loop must attach to the right face at x={right}: {pts:?}"
        );
        // It protrudes to the right of the node and stays within its height band.
        assert!(
            pts.iter().any(|p| p.x > right + eps),
            "loop must protrude rightward: {pts:?}"
        );
        for p in pts {
            assert!(
                p.y >= an.y - an.height / 2.0 - eps && p.y <= an.y + an.height / 2.0 + eps,
                "loop must stay within the node's height band: {p:?}"
            );
        }

        // The reserved margin keeps the loop clear of the other node.
        assert_no_node_overlap(&r);
    }

    #[test]
    fn self_loop_reserves_room_from_neighbours() {
        // a has a self-loop and a wide right neighbour `c`; the loop must not
        // collide with c. b/c sit on the rank below a.
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let loop_edge = g.make_edge(a, a, ());
        g.make_edge(a, b, ());
        g.make_edge(a, c, ());
        let r = layout(&g, a);
        let an = r.get_node(a).unwrap();
        let loop_max_x = r
            .get_waypoints(loop_edge)
            .unwrap()
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        // The loop protrudes past the node's right edge but the reservation is
        // accounted for in spacing, so no node box is crossed by it.
        assert!(loop_max_x > an.x + an.width / 2.0);
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn sese_mode_expands_nested_regions_and_preserves_routes() {
        let mut graph = G::default();
        let nodes: Vec<_> = (0..10).map(|_| graph.make_node(())).collect();
        let mut endpoints = HashMap::default();
        for (from, to) in [
            (0, 1),
            (1, 2),
            (1, 3),
            (2, 4),
            (3, 4),
            (4, 5),
            (5, 6),
            (5, 7),
            (6, 8),
            (7, 8),
            (8, 9),
        ] {
            let edge = graph.make_edge(nodes[from], nodes[to], ());
            endpoints.insert(edge, (nodes[from], nodes[to]));
        }
        let build = || {
            LayoutBuilder::new(&graph)
                .root(nodes[0])
                .mode(LayoutMode::Sese)
                .geometry(|id| NodeGeometry {
                    width: 30.0 + usize::from(id) as f64 * 3.0,
                    height: 24.0,
                })
                .build()
                .unwrap()
        };
        let first = build();
        let second = build();
        assert_eq!(signature(&first), signature(&second));
        assert_eq!(first.nodes.len(), nodes.len());
        assert_eq!(first.edges.len(), endpoints.len());
        assert!(
            !first.regions.is_empty(),
            "SESE debug regions should be retained"
        );
        assert_no_node_overlap(&first);
        assert_no_edge_through_nonincident_node(&first, &endpoints);
        assert_route_attaches_to_endpoints(&first, &endpoints);
        for points in first.edges.values() {
            assert!(points.len() >= 2);
            assert!(points.windows(2).all(|pair| {
                (pair[0].x - pair[1].x).abs() < 1e-6 || (pair[0].y - pair[1].y).abs() < 1e-6
            }));
        }

        let straight = LayoutBuilder::new(&graph)
            .root(nodes[0])
            .mode(LayoutMode::Sese)
            .edge_style(EdgeStyle::Straight)
            .build()
            .unwrap();
        assert!(
            straight
                .edges
                .values()
                .any(|points| points.windows(2).any(|pair| {
                    (pair[0].x - pair[1].x).abs() > 1e-6 && (pair[0].y - pair[1].y).abs() > 1e-6
                }))
        );
        assert_no_edge_through_nonincident_node(&straight, &endpoints);
    }

    #[test]
    fn sese_normalization_keeps_virtual_hammock_terminals_internal() {
        let mut graph = G::default();
        let entry = graph.make_node(());
        let left = graph.make_node(());
        let right = graph.make_node(());
        graph.make_edge(entry, left, ());
        graph.make_edge(entry, right, ());
        let component = vec![entry, left, right];
        let tree = compute_sese_normalized(&graph, &component, entry);
        assert_eq!(tree.root().contained_nodes, component);
        let mut owned: Vec<_> = tree
            .regions
            .iter()
            .flat_map(|region| region.nodes.iter().copied())
            .collect();
        owned.sort();
        assert_eq!(owned, vec![entry, left, right]);
        assert!(tree.regions.iter().all(|region| {
            region
                .contained_nodes
                .iter()
                .all(|node| [entry, left, right].contains(node))
        }));
    }

    #[test]
    fn hammock_fallback_groups_multiple_exit_edges_to_one_exit_node() {
        let mut graph = G::default();
        let root = graph.make_node(());
        let a = graph.make_node(());
        let end = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        let e = graph.make_node(());
        let f = graph.make_node(());
        graph.make_edge(root, a, ());
        graph.make_edge(a, end, ());
        let entry = graph.make_edge(root, b, ());
        graph.make_edge(b, c, ());
        graph.make_edge(c, d, ());
        let first_exit = graph.make_edge(d, end, ());
        graph.make_edge(b, e, ());
        graph.make_edge(e, f, ());
        graph.make_edge(f, end, ());
        let component = vec![root, a, end, b, c, d, e, f];
        let fallback = compute_hammock_fallback(&graph, &component);
        let tree = compute_layout_sese_tree(&graph, &component, root);
        assert_eq!(
            tree.regions.len(),
            4,
            "root, hammock, and two edge-SESE children"
        );
        let hammock = tree
            .regions
            .iter()
            .find(|region| region.contained_nodes == vec![b, c, d, e, f])
            .expect("b..f hammock");
        assert_eq!(hammock.entry_edge, Some(entry));
        assert_eq!(hammock.exit_edge, Some(first_exit));
        let hammock_id = tree
            .regions
            .iter()
            .position(|region| region.contained_nodes == vec![b, c, d, e, f])
            .unwrap();
        for nodes in [[c, d], [e, f]] {
            let child = tree
                .regions
                .iter()
                .find(|region| region.contained_nodes == nodes)
                .expect("nested edge-SESE");
            assert_eq!(child.parent, Some(hammock_id));
        }
        assert!(
            fallback
                .regions
                .iter()
                .any(|region| region.contained_nodes == vec![b, c, d, e, f])
        );

        // The two exits are distinct named bottom-face ports. Their internal
        // routes terminate exactly there, and composing the root records that
        // every proxy join was an equality (rather than an inserted elbow).
        let geometry: HashMap<_, _> = graph
            .nodes()
            .map(|node| (node.id(), NodeGeometry::default()))
            .collect();
        let hammock_composition = compose_sese_region(
            &graph,
            &geometry,
            &LayoutSettings::default(),
            &tree,
            hammock_id,
            root,
        );
        assert!(hammock_composition.valid, "all proxy joins are point equal");
        // Region bounds are content plus one margin; terminal rank spacing is
        // not allowed to inflate a proxy rectangle.
        for region_box in &hammock_composition.region_boxes {
            let owned = &tree.regions[region_box.id].contained_nodes;
            let owned_nodes: HashMap<_, _> = hammock_composition
                .nodes
                .iter()
                .filter(|(id, _)| owned.contains(id))
                .map(|(&id, &node)| (id, node))
                .collect();
            let (min_x, max_x, min_y, max_y) = node_bounds(&owned_nodes);
            assert!((region_box.max_x - region_box.min_x - (max_x - min_x + 40.0)).abs() < 1e-6);
            assert!((region_box.max_y - region_box.min_y - (max_y - min_y + 50.0)).abs() < 1e-6);
        }
        let first_port = hammock_composition.interface.ports[&RegionPortId::Exit { source: d }];
        let second_port = hammock_composition.interface.ports[&RegionPortId::Exit { source: f }];
        assert_ne!(first_port.x_offset, second_port.x_offset);
        for (source, port) in [(d, first_port), (f, second_port)] {
            assert!(!port.is_entry);
            assert_eq!(port.id, RegionPortId::Exit { source });
            assert!(port.x_offset.abs() <= hammock_composition.interface.width / 2.0);
            let route = &hammock_composition.interface.exit_routes[&source];
            let endpoint = route.last().unwrap();
            assert!((endpoint.x - port.x_offset).abs() < 1e-6);
            assert!((endpoint.y - hammock_composition.interface.height / 2.0).abs() < 1e-6);
        }
        assert!(
            compose_sese_region(
                &graph,
                &geometry,
                &LayoutSettings::default(),
                &tree,
                0,
                root,
            )
            .valid
        );

        let layout = LayoutBuilder::new(&graph)
            .root(root)
            .mode(LayoutMode::Sese)
            .build()
            .unwrap();
        assert!(layout.regions.len() >= 2, "root and hammock debug bounds");
        let mut endpoints = HashMap::default();
        for edge in graph.edges() {
            endpoints.insert(edge.id(), (edge.from_id(), edge.to_id()));
        }
        assert_route_attaches_to_endpoints(&layout, &endpoints);
        assert_no_edge_through_nonincident_node(&layout, &endpoints);
    }

    #[test]
    fn sese_parallel_boundary_edges_keep_flat_router_ports() {
        // More than the former 29-port cap: these must keep the ordinary flat
        // router's degree-aware fan, not be sent through a bespoke SESE lane
        // allocator that reuses port positions.
        let mut graph = G::default();
        let s = graph.make_node(());
        let a = graph.make_node(());
        let b = graph.make_node(());
        let c = graph.make_node(());
        let d = graph.make_node(());
        let t = graph.make_node(());
        let mut endpoints = HashMap::default();
        let record = |graph: &mut G, from, to, endpoints: &mut HashMap<E, (N, N)>| {
            let edge = graph.make_edge(from, to, ());
            endpoints.insert(edge, (from, to));
        };
        record(&mut graph, s, a, &mut endpoints);
        for _ in 0..35 {
            record(&mut graph, a, b, &mut endpoints);
        }
        record(&mut graph, a, c, &mut endpoints);
        record(&mut graph, b, d, &mut endpoints);
        record(&mut graph, c, d, &mut endpoints);
        record(&mut graph, d, t, &mut endpoints);

        let result = LayoutBuilder::new(&graph)
            .root(s)
            .mode(LayoutMode::Sese)
            .build()
            .unwrap();
        assert_route_attaches_to_endpoints(&result, &endpoints);
        assert_no_edge_through_nonincident_node(&result, &endpoints);
        assert_no_horizontal_overlap(&result);
        assert_no_vertical_overlap(&result);
    }

    #[test]
    fn sese_mode_falls_back_for_unstructured_component() {
        let mut graph = G::default();
        let root = graph.make_node(());
        let middle = graph.make_node(());
        let exit = graph.make_node(());
        graph.make_edge(root, middle, ());
        graph.make_edge(middle, exit, ());
        let flat = LayoutBuilder::new(&graph).root(root).build().unwrap();
        let sese = LayoutBuilder::new(&graph)
            .root(root)
            .mode(LayoutMode::Sese)
            .build()
            .unwrap();
        assert_eq!(signature(&flat), signature(&sese));
    }

    #[test]
    fn disconnected_components_pack_without_overlap() {
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(c, d, ());
        for mode in [LayoutMode::Flat, LayoutMode::Sese] {
            let r = LayoutBuilder::new(&g).root(a).mode(mode).build().unwrap();
            assert_no_node_overlap(&r);
            // Two components: their x-extents must not interleave.
            let comp1_max = r.get_node(a).unwrap().x.max(r.get_node(b).unwrap().x);
            let comp2_min = r.get_node(c).unwrap().x.min(r.get_node(d).unwrap().x);
            assert!(comp2_min > comp1_max, "components overlap horizontally");
        }
    }

    #[test]
    fn variable_width_nodes_do_not_overlap() {
        let mut g = G::default();
        let a = g.make_node(());
        let wide = g.make_node(());
        let narrow = g.make_node(());
        let d = g.make_node(());
        g.make_edge(a, wide, ());
        g.make_edge(a, narrow, ());
        g.make_edge(wide, d, ());
        g.make_edge(narrow, d, ());
        let r = LayoutBuilder::new(&g)
            .root(a)
            .geometry(move |id| {
                let w = if id == wide { 200.0 } else { 40.0 };
                NodeGeometry {
                    width: w,
                    height: 30.0,
                }
            })
            .build()
            .unwrap();
        assert_no_node_overlap(&r);
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn segment_chain_preserves_order_beside_wide_node_regression() {
        // Regression for a p/q segment whose endpoints were assigned different
        // x coordinates, making the router bridge through node 3.
        let mut g = G::default();
        let n: Vec<_> = (0..4).map(|_| g.make_node(())).collect();
        g.make_edge(n[0], n[2], ());
        g.make_edge(n[1], n[0], ());
        let edge = g.make_edge(n[1], n[2], ());
        g.make_edge(n[2], n[0], ());
        g.make_edge(n[2], n[3], ());
        g.make_edge(n[3], n[1], ());
        let r = LayoutBuilder::new(&g)
            .root(n[0])
            .geometry(|id| NodeGeometry {
                width: if usize::from(id) % 3 == 0 {
                    120.0
                } else {
                    40.0
                },
                height: 30.0,
            })
            .build()
            .unwrap();
        assert!(r.get_waypoints(edge).unwrap().len() >= 2);
        assert_no_edge_through_node(&r);
        assert_no_node_overlap(&r);
    }

    #[test]
    fn long_edge_segment_avoids_wide_intermediate_node() {
        // Chain a..e over 5 ranks plus a long edge a->e spanning all of them.
        // Its q-vertex/segment passes through rank 2 where `c` is very wide; the
        // segment lane must route clear of c (the container-reserves-width
        // guarantee the old x-assignment violated).
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let c = g.make_node(());
        let d = g.make_node(());
        let e = g.make_node(());
        g.make_edge(a, b, ());
        g.make_edge(b, c, ());
        g.make_edge(c, d, ());
        g.make_edge(d, e, ());
        g.make_edge(a, e, ());
        let r = LayoutBuilder::new(&g)
            .root(a)
            .geometry(move |id| {
                let w = if id == c { 220.0 } else { 40.0 };
                NodeGeometry {
                    width: w,
                    height: 30.0,
                }
            })
            .build()
            .unwrap();
        assert_no_node_overlap(&r);
        assert_no_edge_through_node(&r);
    }

    #[test]
    fn root_sits_above_its_back_edge_predecessor() {
        // A 2-cycle where the entry is NOT the min-id node: pred(0) -> entry(1)
        // and entry(1) -> pred(0). Cycle-breaking rooted at `entry` must treat
        // entry -> pred as forward and pred -> entry as the back-edge, so the
        // entry lands on top — even though `pred` has the smaller id and the DFS
        // would otherwise have started there and floated it above the entry.
        let mut g = G::default();
        let pred = g.make_node(());
        let entry = g.make_node(());
        g.make_edge(pred, entry, ());
        g.make_edge(entry, pred, ());
        let r = layout(&g, entry);
        let ey = r.get_node(entry).unwrap().y;
        let py = r.get_node(pred).unwrap().y;
        assert!(
            ey < py,
            "entry (root) should sit above its back-edge predecessor: entry.y={ey} pred.y={py}"
        );
    }

    #[test]
    fn root_tops_loop_when_entry_has_higher_id() {
        // entry(2) -> a(0) -> b(1) -> entry  : the latch b -> entry is the loop
        // back-edge. With id-order cycle breaking the DFS would start at a(0) and
        // misclassify, pushing a real block above the entry. Rooted at the entry,
        // the entry is the unique top real node.
        let mut g = G::default();
        let a = g.make_node(());
        let b = g.make_node(());
        let entry = g.make_node(());
        g.make_edge(entry, a, ());
        g.make_edge(a, b, ());
        g.make_edge(b, entry, ());
        let r = layout(&g, entry);
        let ey = r.get_node(entry).unwrap().y;
        for (id, n) in &r.nodes {
            if *id != entry {
                assert!(
                    ey <= n.y,
                    "entry must be the top real node: entry.y={ey} node{}.y={}",
                    usize::from(*id),
                    n.y
                );
            }
        }
    }

    #[test]
    fn empty_graph_errors() {
        let g = G::default();
        let err = LayoutBuilder::new(&g).geometry(geom()).build().unwrap_err();
        assert_eq!(err, LayoutError::EmptyGraph);
    }
}
