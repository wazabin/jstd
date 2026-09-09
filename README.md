# wazabin-jstd

[![CI](https://github.com/wazabin/jstd/actions/workflows/ci.yml/badge.svg)](https://github.com/wazabin/jstd/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/wazabin-jstd.svg)](https://crates.io/crates/wazabin-jstd)

Small, reusable Rust building blocks, extracted from a binary-analysis
toolchain and kept dependency-light.

Developed by [Thalium](https://blog.thalium.re/about/).

## What's here

- **`registry`** — strongly typed `usize` identifiers (`#[derive(Identifier)]`)
  and id-indexed registries, so a `NodeId` can never be passed where an
  `EdgeId` belongs.
- **`graph`** — directed graph containers (borrowing and owning), plus
  `graph::analysis`: dominator and post-dominator trees, and canonical
  edge-based SESE (single-entry/single-exit) region decomposition.
- **`stable_arena`** — an arena whose elements keep their address as it grows.
- **`intern`** — string interning.
- **`num_ref`** — small numeric reference helpers.

## Graph layout

Triskel, the layered ("Sugiyama-style") graph layout engine, now lives in its
own crate: [`wazabin-triskel`](https://crates.io/crates/wazabin-triskel). It
builds on this crate's `graph` module.

## Development

```sh
cargo test
```

The `jstd_derive` procedural macro is included in this repository and is used
by the main crate.

## License

Licensed under the [MIT License](LICENSE).
