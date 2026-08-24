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
