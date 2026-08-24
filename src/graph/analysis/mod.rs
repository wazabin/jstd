//! Graph analysis algorithms.
//!
//! This module currently provides classic control-flow analyses:
//! - dominators
//! - post-dominators
//!
//! Re-exported helpers:
//! - [`reachable_from_root`]
//! - [`compute_dominators`]
//! - [`DominatorTree`]
//! - [`compute_postdominators`]

pub mod dominator;
pub mod post_dominator;

pub use dominator::{DominatorTree, compute_dominators, reachable_from_root};
pub use post_dominator::compute_postdominators;
