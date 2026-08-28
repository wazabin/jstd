//! Render a DOT graph with Triskel into a PNG.
//!
//! Usage: `triskel-png input.dot output.png [--sese] [--debug] [--straight]
//! [--scale N]`.

use std::{env, fs, path::PathBuf, process};

use image::{Rgb, RgbImage};
use jstd::{
    graph::{Graph, Node, TestGraph},
    triskel::{
        layout::{LayoutBuilder, LayoutMode, LayoutResult, NodeGeometry, Point},
        router::EdgeStyle,
    },
};

const MARGIN: f64 = 32.0;

fn main() {
    let options = match Options::parse() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n\n{}", Options::usage());
            process::exit(2);
        }
    };
    let input = match fs::read_to_string(&options.input) {
        Ok(input) => input,
        Err(error) => fail(format!("cannot read {}: {error}", options.input.display())),
    };
    let normalized = normalize_dot(&input);
    let graph = match TestGraph::parse(&normalized) {
        Ok(graph) if graph.nodes().count() > 0 => graph,
        Ok(_) => fail("DOT graph contains no nodes"),
        Err(error) => fail(format!("cannot parse DOT: {error}")),
    };
    let root = graph.nodes().min_by_key(|node| node.id()).unwrap().id();
    let result = match LayoutBuilder::new(&graph)
        .root(root)
        .mode(if options.sese {
            LayoutMode::Sese
        } else {
            LayoutMode::Flat
        })
        .edge_style(if options.straight {
            EdgeStyle::Straight
        } else {
            EdgeStyle::Orthogonal
        })
        .geometry(|_| NodeGeometry {
            width: 72.0,
            height: 36.0,
        })
        .build()
    {
        Ok(result) => result,
        Err(error) => fail(format!("layout failed: {error}")),
    };
    if options.debug && result.regions.is_empty() {
        eprintln!(
            "debug: no SESE regions were used (the component may have fallen back to flat layout)"
        );
    }
    let image = rasterize(&result, options.debug, options.scale);
    if let Err(error) = image.save(&options.output) {
        fail(format!(
            "cannot write {}: {error}",
            options.output.display()
        ));
    }
    println!(
        "Wrote {} ({} nodes, {} edges, {} debug regions)",
        options.output.display(),
        result.nodes.len(),
        result.edges.len(),
        result.regions.len(),
    );
}

fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("error: {}", message.as_ref());
    process::exit(1)
}

struct Options {
    input: PathBuf,
    output: PathBuf,
    sese: bool,
    debug: bool,
    straight: bool,
    scale: f64,
}

impl Options {
    fn usage() -> &'static str {
        "Usage: triskel-png INPUT.dot OUTPUT.png [--sese] [--debug] [--straight] [--scale N]"
    }

    fn parse() -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut sese = false;
        let mut debug = false;
        let mut straight = false;
        let mut scale: f64 = 2.0;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--sese" => sese = true,
                "--debug" => debug = true,
                "--straight" => straight = true,
                "--scale" => {
                    scale = args
                        .next()
                        .ok_or("--scale requires a positive number")?
                        .parse()
                        .map_err(|_| "--scale requires a positive number")?;
                }
                "--help" | "-h" => return Err(Self::usage().to_string()),
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}")),
                _ => positional.push(PathBuf::from(arg)),
            }
        }
        if positional.len() != 2 || !scale.is_finite() || scale <= 0.0 {
            return Err(Self::usage().to_string());
        }
        Ok(Self {
            input: positional.remove(0),
            output: positional.remove(0),
            sese,
            debug,
            straight,
            scale,
        })
    }
}

/// Small DOT subset adapter. It accepts `digraph` wrappers, semicolon-separated
/// declarations, node attributes, and edge chains; layout deliberately ignores
/// DOT styling and labels.
fn normalize_dot(input: &str) -> String {
    let without_comments: String = input
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(before, _)| before))
        .collect::<Vec<_>>()
        .join("\n");
    let statements = without_comments.replace(['{', '}'], ";");
    let mut output = Vec::new();
    for statement in statements.split(';') {
        let statement = statement.trim();
        if statement.is_empty()
            || statement.starts_with("digraph")
            || statement.starts_with("graph")
            || statement.starts_with("strict")
        {
            continue;
        }
        if statement.contains("->") {
            let names: Vec<_> = statement
                .split("->")
                .map(|name| name.split_once('[').map_or(name, |(head, _)| head).trim())
                .filter(|name| !name.is_empty())
                .collect();
            for pair in names.windows(2) {
                output.push(format!("{} -> {}", pair[0], pair[1]));
            }
        } else {
            let name = statement
                .split_once('[')
                .map_or(statement, |(head, _)| head)
                .trim();
            if !name.is_empty()
                && !name.contains('=')
                && !name.starts_with("subgraph")
                && !matches!(name, "node" | "edge")
            {
                output.push(name.to_string());
            }
        }
    }
    output.join("\n")
}

fn rasterize(result: &LayoutResult<usize, usize>, debug: bool, scale: f64) -> RgbImage {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut include = |x: f64, y: f64| {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    };
    for node in result.nodes.values() {
        include(node.x - node.width / 2.0, node.y - node.height / 2.0);
        include(node.x + node.width / 2.0, node.y + node.height / 2.0);
    }
    for point in result.edges.values().flatten() {
        include(point.x, point.y);
    }
    if debug {
        for region in &result.regions {
            include(
                region.x - region.width / 2.0,
                region.y - region.height / 2.0,
            );
            include(
                region.x + region.width / 2.0,
                region.y + region.height / 2.0,
            );
        }
    }
    if !min_x.is_finite() {
        min_x = 0.0;
        max_x = 1.0;
        min_y = 0.0;
        max_y = 1.0;
    }
    let width = ((max_x - min_x + MARGIN * 2.0) * scale).ceil().max(1.0) as u32;
    let height = ((max_y - min_y + MARGIN * 2.0) * scale).ceil().max(1.0) as u32;
    let map = |point: Point| -> (i32, i32) {
        (
            ((point.x - min_x + MARGIN) * scale).round() as i32,
            ((point.y - min_y + MARGIN) * scale).round() as i32,
        )
    };
    let mut image = RgbImage::from_pixel(width, height, Rgb([250, 250, 250]));
    if debug {
        let palette = [
            [220, 38, 38],
            [37, 99, 235],
            [22, 163, 74],
            [147, 51, 234],
            [217, 119, 6],
        ];
        for region in &result.regions {
            let (x0, y0) = map(Point {
                x: region.x - region.width / 2.0,
                y: region.y - region.height / 2.0,
            });
            let (x1, y1) = map(Point {
                x: region.x + region.width / 2.0,
                y: region.y + region.height / 2.0,
            });
            draw_rect_outline(
                &mut image,
                x0,
                y0,
                x1,
                y1,
                Rgb(palette[region.id % palette.len()]),
            );
        }
    }
    for points in result.edges.values() {
        for segment in points.windows(2) {
            let (x0, y0) = map(segment[0]);
            let (x1, y1) = map(segment[1]);
            draw_line(&mut image, x0, y0, x1, y1, Rgb([75, 85, 99]));
        }
    }
    for node in result.nodes.values() {
        let (x0, y0) = map(Point {
            x: node.x - node.width / 2.0,
            y: node.y - node.height / 2.0,
        });
        let (x1, y1) = map(Point {
            x: node.x + node.width / 2.0,
            y: node.y + node.height / 2.0,
        });
        fill_rect(&mut image, x0, y0, x1, y1, Rgb([17, 24, 39]));
        draw_rect_outline(&mut image, x0, y0, x1, y1, Rgb([71, 85, 105]));
    }
    image
}

fn draw_line(image: &mut RgbImage, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        put(image, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let twice = 2 * error;
        if twice >= dy {
            error += dy;
            x0 += sx;
        }
        if twice <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

fn fill_rect(image: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    for y in y0.min(y1)..=y0.max(y1) {
        for x in x0.min(x1)..=x0.max(x1) {
            put(image, x, y, color);
        }
    }
}

fn draw_rect_outline(image: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    draw_line(image, x0, y0, x1, y0, color);
    draw_line(image, x1, y0, x1, y1, color);
    draw_line(image, x1, y1, x0, y1, color);
    draw_line(image, x0, y1, x0, y0, color);
}

fn put(image: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
        image.put_pixel(x as u32, y as u32, color);
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_dot;
    use jstd::graph::{Graph, TestGraph};

    #[test]
    fn normalizes_wrapped_dot_attributes_and_edge_chains() {
        let source = r#"digraph sample {
            // ignored
            a [label="A"];
            a -> b -> c [color=red];
        }"#;
        let normalized = normalize_dot(source);
        let graph = TestGraph::parse(&normalized).unwrap();
        assert_eq!(graph.nodes().count(), 3);
        assert_eq!(graph.edges().count(), 2);
    }
}
