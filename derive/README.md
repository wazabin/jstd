# jstd_derive

Procedural macros for [`jstd`](https://crates.io/crates/jstd).

This crate provides `#[derive(Identifier)]` for tuple newtypes backed by
`usize` or `u32`. It is maintained and released from the `jstd` repository;
most users should depend on `jstd` and use `jstd::Identifier` instead of
depending on this crate directly.

## License

Licensed under the [MIT License](../LICENSE).
