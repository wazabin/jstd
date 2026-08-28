# Named proxy ports for SESE / hammock composition

## Goal

Replace the current post-layout proxy-route splicing with **named, router-native
proxy ports**. A quotient edge terminating at a region proxy must end at the
same physical point at which the expanded child region's internal interface
route begins. Expansion must only concatenate equal points; it must never
invent an elbow after routing.

This is needed for canonical SESE regions and for the node-hammock fallback,
especially `examples/graphs/hammoc.dot` (`b,c,d,e,f` has one entry and two
exit edges to `end`).

## Current state

Recent relevant commits, newest first:

- `5a6ee0c fix(triskel): include region interfaces in proxy bounds`
- `f232d88 feat(triskel): use node hammocks as SESE fallback`
- `2ad683c feat(triskel): add DOT PNG layout tool`
- `4e627cc feat(triskel): normalize SESE components as hammocks`
- `b9a0449 fix(triskel): compose SESE routes through quotient ports`

The current implementation is safe enough to fall back to flat layout, but is
not true port composition:

- `src/triskel/layout.rs:797` — `RegionComposition` contains an optional entry
  path and `exit_interfaces: HashMap<NodeId, Vec<Point>>`.
- `src/triskel/layout.rs:814` — `compose_sese_region` lays out an internal
  quotient, creates invisible entry/exit terminal nodes, and retains their
  routes.
- `src/triskel/layout.rs:1032-1067` — parent and child routes are joined with
  `splice_points`.
- `src/triskel/layout.rs:1175` — `splice_points` inserts an L elbow after
  layout. This is the behavior to remove.
- `src/triskel/layout.rs:732-735` — unsafe, overlapping, or non-orthogonal
  composition currently falls back to ordinary flat layout. Retain this as a
  defensive last resort during migration, but it should not be the normal path.
- `src/triskel/layout.rs:788` — region/debug bounds. The latest stopgap includes
  terminal paths in the proxy bounds, which prevents terminals visibly sitting
  outside the debug rectangle, but creates excess top/bottom whitespace. Undo
  that dependency once ports are real boundary anchors.

The PNG renderer is `src/bin/triskel-png.rs`. Generate the relevant fixture
with:

```sh
cargo run --bin triskel-png -- examples/graphs/hammoc.dot output.png --sese --debug
```

`hammoc.dot` is currently user-created and untracked. Do not stage it without
asking. Root `*.png` files are intentionally ignored.

## Existing flat-router contract

The normal flat router already has a port model, but it only assigns ordinary
fan positions:

- `src/triskel/layout.rs:253` — `EdgeLayoutData`.
- `src/triskel/router.rs:456` — `Ports { start, end }`; offsets from the source
  bottom face and target top face.
- `src/triskel/router.rs:473` — `assign_ports`; deterministic fan allocation.
- `src/triskel/router.rs:123` — routing invokes `snap_ports(assign_ports(...))`.
- `src/triskel/coordinate.rs:182` — coordinate assignment also invokes
  `assign_ports`, so router and coordinate phases must agree.

After cycle handling, forward edges leave the source bottom face and enter the
target top face. Back-edge gadgets are also represented through those faces.
Therefore the first implementation only needs fixed **top/bottom x offsets**.
Do not introduce arbitrary left/right ports unless a concrete gadget proves
that necessary.

## Target data model

Introduce internal types, probably in `layout.rs` near `EdgeLayoutData`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RegionPortId<NodeId> {
    Entry,
    Exit { source: NodeId },
}

#[derive(Clone, Copy, Debug)]
struct ProxyPort<NodeId> {
    id: RegionPortId<NodeId>,
    // Relative to the proxy centre. Entry is on y = -height/2; exits are on
    // y = +height/2.
    x_offset: f64,
    is_entry: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct PortHint {
    // Offset from the relevant node centre. `None` retains normal fan routing.
    source_bottom: Option<f64>,
    target_top: Option<f64>,
}
```

`RegionComposition` should return a `RegionInterface` rather than arbitrary
route fragments:

```rust
struct RegionInterface<NodeId> {
    width: f64,
    height: f64,
    ports: BTreeMap<RegionPortId<NodeId>, ProxyPort<NodeId>>,
    // Routes begin/end at the exact named port positions.
    entry_route: Option<Vec<Point>>,
    exit_routes: HashMap<NodeId, Vec<Point>>,
}
```

The proxy's size is based on real child content plus one intentional padding
margin. Interface routes/terminal rank spacing must **not** grow the proxy
height. A port is zero-area geometry on the proxy perimeter.

## Implementation sequence

### A. Add fixed-port support to the ordinary pipeline

1. Add `port_start: Option<f64>` and `port_end: Option<f64>` to
   `EdgeLayoutData`.
2. In `router::assign_ports`, first calculate the normal fan allocation, then
   overwrite `Ports.start[eid]` / `Ports.end[eid]` for hinted edges.
3. Ensure `snap_ports` does not move a hinted endpoint. It may still adjust the
   unconstrained endpoint of the same edge.
4. Because `coordinate::assign_x` uses `assign_ports`, it will automatically
   see the same offsets if this logic is centralized there. Add tests proving
   coordinate and router attachment agree.
5. Generalize `layout_component` so a caller can supply a map from original
   component edge ID to `PortHint`. Keep the existing `layout_component`
   wrapper using an empty map, so flat/public callers remain unchanged.

A reasonable shape is:

```rust
fn layout_component_with_port_hints(..., hints: &HashMap<EdgeId, PortHint>)
```

When it constructs `EdgeLayoutData`, copy the matching hint. Do not make
public graph edge payloads carry layout-only port information.

### B. Materialize child ports at real proxy faces

1. Keep invisible terminal nodes for routing *inside* a child if useful, but
   convert their exposed endpoint into a named port on the child proxy face.
2. Pick the entry port from the entry terminal's x coordinate, clamped to the
   proxy width, at `y = -height / 2`.
3. Pick one exit port per boundary source (`Exit { source }`) at
   `y = +height / 2`. Allocate distinct x offsets deterministically, using the
   same source-id ordering as the current exit interface map.
4. Route terminal-to-port stubs inside the child using the normal internal
   layout/router model. They must terminate exactly at the returned port point.
   If a constrained-coordinate facility is needed, add it explicitly; do not
   translate completed routes afterward.
5. Revert the current `compose_sese_region` bounds change that includes every
   interface route point. Bounds should include real child boxes and one
   deliberate proxy margin only.

### C. Use ports while laying out the parent quotient

For every original edge represented in a quotient edge:

- direct node -> child region: set `target_top` from child's `Entry` port;
- child region -> direct node: set `source_bottom` from child's
  `Exit { source: original_edge.from }` port;
- child region -> child region: set both hints;
- direct -> direct: no hints.

This works for the hammock fixture because `d -> end` and `f -> end` use two
different `Exit { source }` ports.

The parent quotient node geometry must exactly equal the child's returned
`RegionInterface.width/height`, and its centre is the translation origin for
all child port coordinates.

### D. Eliminate splice-based composition

At expansion time, replace every use of `splice_points` with:

1. translate child routes and port coordinates by the positioned proxy centre;
2. assert the parent route endpoint and translated child port are equal within
   a small epsilon;
3. concatenate while dropping the duplicate equal point.

Delete `splice_points` once all canonical SESE and hammock paths use named
ports. The old safety fallback can remain temporarily, but a valid structured
fixture should no longer take it.

## Required invariants

Add reusable test helpers, ideally in `layout.rs` tests:

1. **Port equality:** every expanded parent/child join has equal adjacent points
   (within epsilon), not merely an orthogonal connector.
2. **Perimeter:** every `ProxyPort` lies on the intended proxy top/bottom face
   and within its width.
3. **Unique hammock exits:** parallel/multiple exit sources receive distinct
   port offsets unless they intentionally share an exact semantic port.
4. **No geometry regressions:** preserve current node non-overlap, no-edge-
   through-node, orthogonality, collinear-overlap, and determinism checks.
5. **Bounds:** every real node is inside its region rectangle; no terminal stub
   artificially enlarges the rectangle.

## Fixtures/tests

- Add `examples/graphs/hammoc.dot` only if the user agrees to track it; the
  equivalent graph already exists inline in
  `hammock_fallback_groups_multiple_exit_edges_to_one_exit_node`.
- Extend that test to assert two distinct exit port positions and exact route
  joins for `d -> end` and `f -> end`.
- Extend `sese_mode_expands_nested_regions_and_preserves_routes` to assert exact
  joins across nested canonical regions.
- Add a back-edge fixture: proxy source/target port constraints must survive
  cycle gadgets.
- Run:

```sh
cargo test --workspace --all-targets
cargo test triskel_stress_generated_layouts -- --ignored
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo run --bin triskel-png -- examples/graphs/hammoc.dot output.png --sese --debug
```

## Follow-up: compose maximal SESE regions inside hammocks

The expected structured hierarchy for `hammoc.dot` is four regions:

```text
root graph
└── node hammock: b,c,d,e,f
    ├── edge-SESE: c,d   (entry b -> c, exit d -> end)
    └── edge-SESE: e,f   (entry b -> e, exit f -> end)
```

This is intentionally **not** the current behaviour. `compute_sese` chooses
canonical atomic regions by retaining the *smallest* candidate for each
boundary edge. It therefore returns singleton regions `c`, `d`, `e`, and `f`
rather than the useful pairs. `layout_component_sese` then replaces that
singleton-only tree wholesale with `compute_hammock_fallback`, whose greedy
maximal-node selection returns only `b,c,d,e,f`. Consequently the debug layout
shows the root and hammock but not `(c,d)` or `(e,f)`.

Implement a layout-specific region-tree builder that **merges**, rather than
chooses between, these two analyses:

1. Preserve the existing public/canonical `compute_sese` semantics. Do not
   change its smallest-boundary rule merely to serve layout.
2. Add an internal way to enumerate the raw valid edge-SESE candidates before
   `compute_sese` applies `smallest_for_boundary`. Reuse its actual criteria:
   entry-edge dominance, exit-edge post-dominance, nonzero equal JPP
   cycle-equivalence class, and `region_nodes`. Normalize synthetic entry/exit
   boundaries exactly as `compute_sese_normalized` already does; discard any
   candidate whose boundary or contained node is synthetic.
3. Build the maximal node-hammock candidates as today. Select maximal,
   nontrivial edge-SESE candidates (`contained_nodes.len() > 1`) that are
   strict subsets of a selected hammock. For the fixture this must select
   exactly `{c,d}` and `{e,f}`. Selection must be deterministic, laminar, and
   must not select crossing/overlapping siblings. Prefer larger candidates
   under containment; preserve legitimate nested SESE candidates as children.
4. Construct one `SeseTree` with the root graph as region 0, each selected
   hammock as its child, and the selected SESE candidates nested under the
   smallest containing hammock/SESE region. Assign each original node once to
   its deepest owner. Keep boundary edges on each region so
   `compose_sese_region` can create its interfaces.
5. Use this merged tree in `layout_component_sese`. Hammock fallback must no
   longer discard useful canonical/edge-SESE structure. It remains appropriate
   for subgraphs that have no useful edge-SESE candidates.
6. Extend `hammock_fallback_groups_multiple_exit_edges_to_one_exit_node` (or a
   focused companion test) to assert the exact four-region hierarchy above,
   including parent ids and contained node sets. Render the fixture with
   `triskel-png --sese --debug` and verify both `(c,d)` and `(e,f)` rectangles
   appear inside the hammock rectangle.
7. Re-run all port-composition invariants for this nested mixed hierarchy:
   `b -> c`, `b -> e`, `d -> end`, and `f -> end` must use exact named-port
   joins; the two hammock exit ports remain distinct.

## Scope cautions

- Do not reintroduce a global obstacle router for expanded original edges.
- Do not silently duplicate CFG nodes/edges to solve routing.
- Do not expose synthetic terminal nodes or edges in `LayoutResult`.
- Preserve flat and energy layout behavior; their `LayoutResult.regions` stay
  empty.
- Keep commits atomic. The worktree currently also has an unrelated modified
  `.github/workflows/ci.yml`; do not include it.
