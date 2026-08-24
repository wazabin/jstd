//! SVG / HTML rendering of a [`LayoutResult`].

use crate::{registry::Identifier, triskel::layout::LayoutResult};

const MARGIN: f64 = 24.0;

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn render_svg_with_labels<NodeId: Identifier, EdgeId: Identifier, F>(
    layout: &LayoutResult<NodeId, EdgeId>,
    mut label_for: F,
) -> String
where
    F: FnMut(NodeId) -> String,
{
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for node in layout.nodes.values() {
        min_x = min_x.min(node.x - node.width / 2.0);
        max_x = max_x.max(node.x + node.width / 2.0);
        min_y = min_y.min(node.y - node.height / 2.0);
        max_y = max_y.max(node.y + node.height / 2.0);
    }
    for point in layout.edges.values().flatten() {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        max_x = 0.0;
        min_y = 0.0;
        max_y = 0.0;
    }

    let width = ((max_x - min_x) + MARGIN * 2.0).max(240.0);
    let height = ((max_y - min_y) + MARGIN * 2.0).max(180.0);
    let nx = |x: f64| (x - min_x) + MARGIN;
    let ny = |y: f64| (y - min_y) + MARGIN;

    let mut edges_svg = String::new();
    let mut edges: Vec<_> = layout.edges.iter().collect();
    edges.sort_by_key(|(edge_id, _)| Into::<usize>::into(**edge_id));
    for (edge_id, points) in edges {
        if points.len() < 2 {
            continue;
        }
        let mut d = String::new();
        for (i, p) in points.iter().enumerate() {
            let cmd = if i == 0 { 'M' } else { 'L' };
            d.push_str(&format!("{cmd} {:.2} {:.2} ", nx(p.x), ny(p.y)));
        }
        edges_svg.push_str(&format!(
            "<path data-edge-id=\"{}\" d=\"{}\" fill=\"none\" stroke=\"#6b7280\" stroke-width=\"2\" />",
            Into::<usize>::into(*edge_id),
            d.trim_end()
        ));
    }

    let mut nodes_svg = String::new();
    let mut nodes: Vec<_> = layout.nodes.values().collect();
    nodes.sort_by_key(|node| Into::<usize>::into(node.id));
    for node in nodes {
        let raw = label_for(node.id);
        let lines: Vec<&str> = raw.split('\n').collect();
        let line_height = 12.0;
        let top = ny(node.y) - lines.len() as f64 * line_height / 2.0 + line_height / 2.0;

        nodes_svg.push_str(&format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"4\" fill=\"#111827\" stroke=\"#475569\" stroke-width=\"1.5\" />",
            nx(node.x - node.width / 2.0),
            ny(node.y - node.height / 2.0),
            node.width,
            node.height,
        ));
        let mut tspans = String::new();
        for (i, line) in lines.iter().enumerate() {
            tspans.push_str(&format!(
                "<tspan x=\"{:.2}\" y=\"{:.2}\">{}</tspan>",
                nx(node.x),
                top + i as f64 * line_height,
                escape_html(line),
            ));
        }
        nodes_svg.push_str(&format!(
            "<text text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"sans-serif\" font-size=\"10\" fill=\"#f9fafb\">{tspans}</text>",
        ));
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">{edges_svg}{nodes_svg}</svg>",
    )
}

pub(crate) fn render_svg<NodeId: Identifier, EdgeId: Identifier>(
    layout: &LayoutResult<NodeId, EdgeId>,
) -> String {
    render_svg_with_labels::<NodeId, EdgeId, _>(layout, |node_id: NodeId| {
        Into::<usize>::into(node_id).to_string()
    })
}

pub(crate) fn render_html_with_labels<NodeId: Identifier, EdgeId: Identifier, F>(
    layout: &LayoutResult<NodeId, EdgeId>,
    title: &str,
    label_for: F,
) -> String
where
    F: FnMut(NodeId) -> String,
{
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body>{}</body></html>",
        escape_html(title),
        render_svg_with_labels::<NodeId, EdgeId, F>(layout, label_for),
    )
}

pub(crate) fn render_html<NodeId: Identifier, EdgeId: Identifier>(
    layout: &LayoutResult<NodeId, EdgeId>,
    title: &str,
) -> String {
    render_html_with_labels::<NodeId, EdgeId, _>(layout, title, |node_id: NodeId| {
        Into::<usize>::into(node_id).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashMap as HashMap;

    use crate::triskel::layout::{LayoutNode, Point};

    #[test]
    fn renderers_escape_labels_sort_ids_and_emit_paths() {
        let mut nodes = HashMap::default();
        nodes.insert(
            2_usize,
            LayoutNode {
                id: 2,
                x: 50.0,
                y: 20.0,
                width: 30.0,
                height: 10.0,
            },
        );
        nodes.insert(
            1_usize,
            LayoutNode {
                id: 1,
                x: -10.0,
                y: 0.0,
                width: 20.0,
                height: 20.0,
            },
        );
        let mut edges = HashMap::default();
        edges.insert(
            3_usize,
            vec![Point { x: -10.0, y: 0.0 }, Point { x: 50.0, y: 20.0 }],
        );
        edges.insert(4, vec![Point { x: 0.0, y: 0.0 }]); // ignored: not a path
        let layout = LayoutResult { nodes, edges };

        let svg = render_svg_with_labels(&layout, |id| {
            (if id == 1 { "<&>\"'" } else { "second\nline" }).into()
        });
        assert!(svg.contains("data-edge-id=\"3\""));
        assert!(!svg.contains("data-edge-id=\"4\""));
        assert!(svg.contains("&lt;&amp;&gt;&quot;&#39;"));
        assert!(svg.contains("second</tspan><tspan"));
        assert!(svg.find("<rect").unwrap() < svg.rfind("<rect").unwrap());

        let default_svg = render_svg(&layout);
        assert!(default_svg.contains(">1</tspan>"));
        let html = render_html_with_labels(&layout, "<&>", |_| "label".into());
        assert!(html.contains("<title>&lt;&amp;&gt;</title>"));
        assert!(html.contains("<svg"));
        assert!(render_html(&layout, "plain").contains("<title>plain</title>"));
    }

    #[test]
    fn empty_layout_uses_the_minimum_view_box() {
        let layout = LayoutResult::<usize, usize> {
            nodes: HashMap::default(),
            edges: HashMap::default(),
        };
        let svg = render_svg(&layout);
        assert!(svg.contains("width=\"240\" height=\"180\""));
        assert!(!svg.contains("<rect"));
        assert!(!svg.contains("<path"));
    }
}
