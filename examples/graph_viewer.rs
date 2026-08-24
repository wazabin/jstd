use std::{collections::HashMap, fs, path::PathBuf};

use jstd::{
    graph::{Graph, Node, TestGraph},
    triskel::{
        layout::{LayoutBuilder, LayoutSettings, NodeGeometry},
        router::EdgeStyle,
    },
};

const GRAPHS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/graphs");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let straight = args.iter().any(|a| a == "--straight");
    let node_width = parse_flag(&args, "--node-width").unwrap_or(72.0);
    let node_height = parse_flag(&args, "--node-height").unwrap_or(32.0);
    let line_height = parse_flag(&args, "--line-height").unwrap_or(16.0);
    let layer_gap = parse_flag(&args, "--layer-gap").unwrap_or(100.0);
    let node_gap = parse_flag(&args, "--node-gap").unwrap_or(120.0);
    let out = args
        .iter()
        .find(|a| !a.starts_with("--") && !a.contains('=') && *a != &args[0])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("graph_view.html"));

    let mut dot_files: Vec<PathBuf> = fs::read_dir(GRAPHS_DIR)
        .expect("examples/graphs directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "dot"))
        .collect();
    dot_files.sort();

    if dot_files.is_empty() {
        eprintln!("No .dot files found in {GRAPHS_DIR}");
        std::process::exit(1);
    }

    let settings = LayoutSettings {
        edge_style: if straight {
            EdgeStyle::Straight
        } else {
            EdgeStyle::Orthogonal
        },
        layer_gap,
        node_gap,
        ..Default::default()
    };
    let geometry_for = |label: &str| -> NodeGeometry {
        let line_count = label.split('\n').count().max(1);
        NodeGeometry {
            width: node_width,
            height: ((line_count as f64) * line_height + 16.0).max(node_height),
        }
    };

    // Keep source strings alive — TestGraph borrows from them. Labels and
    // explicit per-node sizes ride alongside in side maps because multi-line
    // labels would break TestGraph's line-oriented parser.
    let prepared: Vec<(String, HashMap<String, NodeAttrs>)> = dot_files
        .iter()
        .map(|p| {
            let raw = fs::read_to_string(p)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", p.display()));
            preprocess_dot(&raw)
        })
        .collect();

    let mut cards = String::new();
    let mut card_count = 0usize;

    for (path, (source, attrs)) in dot_files.iter().zip(prepared.iter()) {
        let stem = path.file_stem().unwrap().to_string_lossy();
        let title = format_title(&stem);

        let graph = match TestGraph::parse(source) {
            Ok(g) if g.nodes().count() > 0 => g,
            Ok(_) => {
                eprintln!("skip {stem}: empty graph");
                continue;
            }
            Err(e) => {
                eprintln!("skip {stem}: {e}");
                continue;
            }
        };

        let root = graph.nodes().min_by_key(|n| n.id()).unwrap().id();

        let attrs_of = |id| -> (String, Option<f64>, Option<f64>) {
            let name = graph
                .get_node(id)
                .map(|n| n.data().to_string())
                .unwrap_or_default();
            match attrs.get(&name) {
                Some(a) => (
                    a.label.clone().unwrap_or_else(|| name.clone()),
                    a.width,
                    a.height,
                ),
                None => (name, None, None),
            }
        };
        let label_of = |id| attrs_of(id).0;

        let geom_fn = |id| {
            let (label, w, h) = attrs_of(id);
            let mut g = geometry_for(&label);
            if let Some(w) = w {
                g.width = w;
            }
            if let Some(h) = h {
                g.height = h;
            }
            g
        };
        let svg = match LayoutBuilder::new(&graph)
            .root(root)
            .settings(settings)
            .geometry(geom_fn)
            .build()
        {
            Ok(result) => {
                let label_fn = |id| {
                    let label = label_of(id);
                    if label.is_empty() {
                        id.to_string()
                    } else {
                        label
                    }
                };
                result.render_svg_with_labels(label_fn)
            }
            Err(e) => {
                eprintln!("skip {stem}: layout {e:?}");
                continue;
            }
        };

        let active = if card_count == 0 { " active" } else { "" };
        cards.push_str(&format!(
            "<div class=\"graph{active}\" data-title=\"{}\">{svg}</div>\n",
            html_attr(&title),
        ));
        card_count += 1;
    }

    if card_count == 0 {
        eprintln!("No graphs could be rendered.");
        std::process::exit(1);
    }

    let html = build_html(&cards);
    fs::write(&out, &html).expect("failed to write output HTML");
    println!(
        "Wrote {} ({card_count} graph{})",
        out.display(),
        if card_count == 1 { "" } else { "s" }
    );
    println!("Open in a browser. Use ← → arrows or buttons to navigate.");
}

fn build_html(cards: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Triskel Graph Viewer</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  background: #0f172a; color: #f1f5f9;
  font-family: system-ui, sans-serif;
  height: 100vh; display: flex; flex-direction: column; overflow: hidden;
}}
#toolbar {{
  display: flex; align-items: center; gap: 10px;
  padding: 10px 16px; background: #1e293b;
  border-bottom: 1px solid #334155; flex-shrink: 0;
}}
#graph-title {{ font-size: 15px; font-weight: 600; flex: 1; }}
#counter {{ font-size: 12px; color: #64748b; white-space: nowrap; }}
button {{
  background: #334155; border: none; color: #e2e8f0;
  padding: 5px 16px; border-radius: 5px; cursor: pointer; font-size: 14px;
}}
button:hover:not(:disabled) {{ background: #475569; }}
button:disabled {{ opacity: 0.35; cursor: default; }}
#content {{
  flex: 1; overflow: auto;
  display: flex; align-items: center; justify-content: center; padding: 24px;
}}
.graph {{ display: none; }}
.graph.active {{ display: flex; }}
.graph.active svg {{ max-width: 100%; max-height: calc(100vh - 70px); width: auto; height: auto; }}
</style>
</head>
<body>
<div id="toolbar">
  <button id="prev">&#8592;</button>
  <span id="graph-title"></span>
  <span id="counter"></span>
  <button id="next">&#8594;</button>
</div>
<div id="content">
{cards}</div>
<script>
const graphs = document.querySelectorAll('.graph');
let cur = 0;
function show(i) {{
  if (i < 0 || i >= graphs.length) return;
  graphs[cur].classList.remove('active');
  cur = i;
  graphs[cur].classList.add('active');
  document.getElementById('graph-title').textContent = graphs[cur].dataset.title;
  document.getElementById('counter').textContent =
    (cur + 1) + ' / ' + graphs.length;
  document.getElementById('prev').disabled = cur === 0;
  document.getElementById('next').disabled = cur === graphs.length - 1;
}}
document.getElementById('prev').onclick = () => show(cur - 1);
document.getElementById('next').onclick = () => show(cur + 1);
document.addEventListener('keydown', e => {{
  if (e.key === 'ArrowLeft') show(cur - 1);
  if (e.key === 'ArrowRight') show(cur + 1);
}});
show(0);
</script>
</body>
</html>"#,
        cards = cards,
    )
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_flag(args: &[String], flag: &str) -> Option<f64> {
    let prefix = format!("{flag}=");
    for (i, a) in args.iter().enumerate() {
        if let Some(rest) = a.strip_prefix(&prefix) {
            return rest.parse().ok();
        }
        if a == flag
            && let Some(next) = args.get(i + 1)
            && let Ok(v) = next.parse()
        {
            return Some(v);
        }
    }
    None
}

fn html_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_title(stem: &str) -> String {
    // "01_if_else" → "If Else"
    let s = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_' || c == '-');
    s.split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Default, Clone)]
struct NodeAttrs {
    label: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
}

/// Strips the `digraph { }` wrapper, emits one edge/node declaration per line
/// (no payloads — multi-line labels would break the line-oriented parser), and
/// returns per-node attributes (label/width/height) keyed by node name.
fn preprocess_dot(source: &str) -> (String, HashMap<String, NodeAttrs>) {
    let mut lines = Vec::new();
    let mut attrs: HashMap<String, NodeAttrs> = HashMap::new();
    let mut depth = 0i32;

    for raw in source.lines() {
        let line = match raw.find("//") {
            Some(pos) => raw[..pos].trim(),
            None => raw.trim(),
        };

        if line.is_empty() {
            continue;
        }

        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;
        let prev_depth = depth;
        depth += opens - closes;

        if prev_depth == 0 && opens > 0 {
            continue;
        }
        if closes > 0 && depth < closes {
            continue;
        }
        if depth <= 0 {
            continue;
        }

        let line = line.trim_end_matches(';').trim();
        if line.is_empty() {
            continue;
        }

        if let Some(arrow) = line.find("->") {
            let from = line[..arrow].trim();
            let after = line[arrow + 2..].trim();
            let to = match after.find('[') {
                Some(b) => after[..b].trim(),
                None => after,
            };
            if !from.is_empty() && !to.is_empty() {
                lines.push(format!("{from} -> {to}"));
            }
            continue;
        }

        if let Some(b) = line.find('[') {
            let name = line[..b].trim();
            if name.is_empty() || matches!(name, "node" | "edge" | "graph") {
                continue;
            }
            let attrs_end = line.rfind(']').unwrap_or(line.len());
            let attr_text = &line[b + 1..attrs_end];
            let parsed = parse_node_attrs(attr_text);
            if parsed.label.is_some() || parsed.width.is_some() || parsed.height.is_some() {
                attrs.insert(name.to_string(), parsed);
            }
            lines.push(name.to_string());
            continue;
        }

        if line.contains('=') || line.starts_with("subgraph") {
            continue;
        }

        lines.push(line.to_string());
    }

    (lines.join("\n"), attrs)
}

fn parse_node_attrs(input: &str) -> NodeAttrs {
    let mut out = NodeAttrs::default();
    for (key, value) in split_attr_pairs(input) {
        match key {
            "label" => out.label = Some(decode_dot_escapes(&value)),
            "width" => out.width = value.parse().ok(),
            "height" => out.height = value.parse().ok(),
            _ => {}
        }
    }
    out
}

fn split_attr_pairs(input: &str) -> Vec<(&str, String)> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len()
            && bytes[i] != b'='
            && bytes[i] != b','
            && !bytes[i].is_ascii_whitespace()
        {
            i += 1;
        }
        let key_end = i;
        if key_start == key_end {
            break;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let v = &input[start..i];
            if i < bytes.len() {
                i += 1;
            }
            v.to_string()
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b',' && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            input[start..i].to_string()
        };
        out.push((&input[key_start..key_end], value));
    }
    out
}

fn decode_dot_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('l') | Some('r') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
