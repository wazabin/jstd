extern crate self as jstd;

/// Derive macro for strongly typed `usize` identifiers.
///
/// This derive is intended for tuple newtypes with exactly one `usize` field,
/// and generates implementations for:
/// - `Copy`, `Clone`, `Debug`, `Default`, `PartialEq`, `Eq`, `Hash`
/// - `From<usize>` and `From<Self> for usize`
/// - [`registry::Identifier`]
///
/// # Example
/// ```
/// use jstd::Identifier;
///
/// #[derive(Identifier)]
/// pub struct MyId(usize);
///
/// let id = MyId::from(42usize);
/// let raw: usize = id.into();
/// assert_eq!(raw, 42);
/// assert_eq!(MyId::default(), MyId::from(0usize));
/// ```
pub use jstd_derive::Identifier;

pub mod graph;
pub mod intern;
pub mod log;
pub mod num_ref;
pub mod registry;
pub mod stable_arena;
pub mod triskel;
