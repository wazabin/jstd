//! Graph analysis algorithms.
//!
//! This module currently provides classic control-flow analyses:
//! - dominators
//! - post-dominators
//! - canonical edge-based SESE regions / program-structure trees
//!
//! Re-exported helpers:
//! - [`reachable_from_root`]
//! - [`compute_dominators`]
//! - [`DominatorTree`]
//! - [`compute_postdominators`]
//! - [`compute_sese`]

pub mod dominator;
pub mod post_dominator;
pub mod sese;

pub use dominator::{DominatorTree, compute_dominators, reachable_from_root};
pub use post_dominator::compute_postdominators;
pub(crate) use sese::compute_sese_candidates;
pub use sese::{SeseRegion, SeseTree, compute_sese};
