//! Layered ("Sugiyama-style") directed graph layout.
//!
//! Pipeline (top-to-bottom, run per weakly-connected component):
//!
//! 1. [`cycle`] — reverse back-edges so the remaining phases see a DAG (rooted
//!    at the entry, so the entry stays the top rank). Self-loops are held out of
//!    the layered graph by [`layout`] and drawn as a loop off the node's right
//!    face once it is positioned.
//! 2. [`rank`] — assign each node a rank (layer) via network simplex.
//! 3. [`segment`] — decompose long edges into Eiglsperger segments (p/q
//!    endpoints + one pass-through segment), avoiding a dummy node per layer.
//! 4. [`order`] — within-rank ordering / crossing reduction.
//! 5. [`coordinate`] — x-coordinate assignment (segment-aware Brandes–Köpf).
//! 6. [`router`] — turn the positioned graph into edge waypoints.
//! 7. [`render`] — SVG/HTML output.
//!
//! The public entry point is [`layout::LayoutBuilder`]. By default each weak
//! component runs as one flat layered graph. [`layout::LayoutMode::Sese`] is an
//! opt-in CFG mode: canonical edge-based SESE regions are replaced by sized
//! proxies, laid out bottom-up, expanded, and then routed through explicit
//! obstacle-free boundary corridors. Weak components are still packed side by
//! side.

pub mod coordinate;
pub mod cycle;
pub mod energy;
pub(crate) mod geometry;
pub(crate) mod hammock;
pub mod layout;
pub mod order;
pub mod rank;
pub mod render;
pub mod router;
pub mod segment;
