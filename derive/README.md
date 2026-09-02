# wazabin-jstd-derive

Procedural macros for [`wazabin-jstd`](https://crates.io/crates/wazabin-jstd).

This crate provides `#[derive(Identifier)]` for tuple newtypes backed by
`usize` or `u32`. It is maintained and released from the `wazabin-jstd`
repository; most users should depend on `wazabin-jstd` and use
`jstd::Identifier` instead of depending on this crate directly.

## License

Licensed under the [MIT License](../LICENSE).
