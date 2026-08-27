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
//! The public entry point is [`layout::LayoutBuilder`].

pub mod coordinate;
pub mod cycle;
pub mod energy;
pub(crate) mod geometry;
pub mod layout;
pub mod order;
pub mod rank;
pub mod render;
pub mod router;
pub mod segment;
