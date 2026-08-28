# jstd

Small, reusable Rust utilities: typed identifiers, registries, graphs, string
interning, and stable arenas.

## Layout PNG tool

`triskel-png` reads a basic directed DOT graph and renders Triskel's layout to
PNG. It accepts DOT wrappers, node attributes, and edge chains; DOT styling and
labels are intentionally ignored.

```sh
cargo run --bin triskel-png -- input.dot output.png --sese --debug
```

Options: `--sese` enables SESE composition, `--debug` draws the SESE proxy
bounds used by the layout, `--straight` requests straight routing, and
`--scale N` controls raster scale (default `2`). In debug mode, an empty region
set means SESE fell back to flat layout for that component.

## Development

```sh
cargo test
```

The `jstd_derive` procedural macro is included in this repository and is used
by the main crate.

## License

Licensed under the [MIT License](LICENSE).
