//! SVG artwork, rendered to dots by resvg - at import, and only then.
//!
//! resvg 0.48 was built for SVG someone chose to open, and its defaults show
//! it: it reads any local file an `<image>` names, relative to the working
//! directory when no resource directory is set; it sizes a pattern tile from
//! the file with no cap, so a few hundred bytes can abort the process; and it
//! reports what it could not draw only to a log. Rendering at import keeps all
//! of that away from a shared label, which carries only the dots. Here, on a
//! file the user picked, it is fenced in:
//!
//! - no file access at all: both image resolvers refuse, and no resource
//!   directory is set;
//! - `<text>`, `<image>`, `<foreignObject>`, `<pattern>`, `<filter>` and
//!   `<marker>` are refused by name before usvg sees them - a marker is
//!   copied once per vertex of every path it decorates, and with visible
//!   overflow those copies need no layer the cap below would count - and any
//!   filter usvg still finds, CSS `filter: blur()` for one, is refused after;
//! - at most 64 clip paths and masks, counted before usvg sees them: usvg
//!   follows a chain of them by recursion, and a long enough chain overflows
//!   the stack, which aborts the process rather than failing;
//! - at most 64 isolated groups (masks, clips, opacity), nested at most 8
//!   deep, because each costs a full offscreen layer;
//! - at most a million dashes over the whole drawing, counted from each
//!   dashed path's length before anything is drawn: tiny-skia gives up past a
//!   million in one path, but not across paths, and a few thousand short
//!   paths each dashed far finer than a dot cost minutes of work;
//! - at most 2 MiB of XML, 100 000 nodes, and 262 144 dots of output.
//!
//! Text is refused rather than drawn: drawing it needs resvg's `text` feature,
//! which lays text out a second way, over `fontdb`, with unhinted glyphs that
//! would not match relabel's own text, and brings a Zlib-only dependency.
//! Converting text to outlines is one menu item in every editor that writes
//! SVG.
//!
//! Edges are crisp unless the file asks otherwise. An anti-aliased hairline
//! half a dot wide is a row of mid-grey dots, and a threshold prints none of
//! them; drawn crisp, it prints as a line.

use crate::artwork::{
    Artwork, ArtworkError, ImportNote, Stats, SvgRefusal, Want, fit_box, luma601, shrink,
};
use crate::document::ImageFit;
use crate::target::Dpmm;
use resvg::usvg;
use tiny_skia::{Color, Pixmap, Transform};

const MAX_BYTES: usize = 2 << 20;
const MAX_NODES: u32 = 100_000;
const MAX_ISOLATED: usize = 64;
const MAX_ISOLATION_DEPTH: usize = 8;
const MAX_DOTS: u64 = crate::artwork::SVG_MAX_DOTS;
const MAX_CLIPS: usize = 64;
/// tiny-skia's own cap for one path (`dash.rs`, `MAX_DASH_COUNT`), here for
/// the whole drawing.
const MAX_DASHES: f64 = 1_000_000.0;
const REFUSED: [&str; 6] = [
    "foreignObject",
    "filter",
    "image",
    "marker",
    "pattern",
    "text",
];

pub(crate) struct Rendered {
    pub art: Artwork,
    pub stats: Stats,
    /// The SVG's own size, in dots at the label's pitch.
    pub natural: (u32, u32),
    pub notes: Vec<ImportNote>,
}

fn refuse(r: SvgRefusal) -> ArtworkError {
    ArtworkError::Svg(r)
}

pub(crate) fn rasterise(
    bytes: &[u8],
    want: Want<'_>,
    dpmm: Dpmm,
) -> Result<Rendered, ArtworkError> {
    if bytes.len() > MAX_BYTES {
        return Err(refuse(SvgRefusal::TooLarge { bytes: bytes.len() }));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| refuse(SvgRefusal::NotUtf8))?;
    // Parsed here rather than by `Tree::from_str`, which cannot be given a node
    // limit: roxmltree's own default is four billion, and a 30 MB file of
    // rectangles built 393 MB of tree before usvg refused it.
    let doc = usvg::roxmltree::Document::parse_with_options(
        text,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_NODES,
            ..Default::default()
        },
    )
    .map_err(|e| refuse(SvgRefusal::Xml(e.to_string())))?;
    // By local name, so a prefixed element is caught too.
    let mut found: Vec<&str> = doc
        .descendants()
        .filter(|n| n.is_element())
        .map(|n| n.tag_name().name())
        .filter(|n| REFUSED.contains(n))
        .collect();
    found.sort_unstable();
    found.dedup();
    if !found.is_empty() {
        let advice = |n: &str| match n {
            "text" => "convert text to outlines",
            "image" => "import the picture on its own",
            "foreignObject" => "remove it",
            _ => "expand it into plain shapes",
        };
        let names: Vec<String> = found
            .iter()
            .map(|n| format!("<{n}> ({})", advice(n)))
            .collect();
        return Err(refuse(SvgRefusal::Elements(names.join(", "))));
    }
    let clips = doc
        .descendants()
        .filter(|n| n.is_element() && matches!(n.tag_name().name(), "clipPath" | "mask"))
        .count();
    if clips > MAX_CLIPS {
        return Err(refuse(SvgRefusal::TooManyClips(clips)));
    }

    let tree = usvg::Tree::from_xmltree(&doc, &options(dpmm))
        .map_err(|e| refuse(SvgRefusal::Parse(e.to_string())))?;
    if !tree.filters().is_empty() {
        return Err(refuse(SvgRefusal::Filters));
    }
    let (mut groups, mut deepest) = (0, 0);
    isolation(tree.root(), 0, &mut groups, &mut deepest);
    if groups > MAX_ISOLATED {
        return Err(refuse(SvgRefusal::TooManyGroups(groups)));
    }
    if deepest > MAX_ISOLATION_DEPTH {
        return Err(refuse(SvgRefusal::TooDeep(deepest)));
    }
    let mut dashes = 0.0;
    count_dashes(tree.root(), &mut dashes);
    // A NaN is refused too.
    if dashes.is_nan() || dashes > MAX_DASHES {
        return Err(refuse(SvgRefusal::TooManyDashes(
            dashes.min(u64::MAX as f64) as u64,
        )));
    }

    let size = tree.size();
    let natural = (dots(size.width()), dots(size.height()));
    let mut notes = Vec::new();
    // The raster's size, the box the whole SVG is scaled to, and where that
    // box starts: at or left of and above the raster's corner, for a cover or
    // an overflowing one-to-one.
    let (raster, scaled, shift) = match want {
        Want::Within(max) => {
            let s = shrink(natural, max);
            (s, s, (0, 0))
        }
        Want::Frame(frame, fit) => {
            let p = fit_box(natural, frame, fit).ok_or(ArtworkError::EmptyFrame)?;
            let raster = (p.bw.min(frame.0), p.bh.min(frame.1));
            if *fit == ImageFit::Dots && raster != natural {
                notes.push(ImportNote::Cropped {
                    from: natural,
                    to: raster,
                });
            }
            if *fit == ImageFit::Cover && raster != (p.bw, p.bh) {
                notes.push(ImportNote::CoverCropped {
                    from: (p.bw, p.bh),
                    to: raster,
                });
            }
            (raster, (p.bw, p.bh), (p.ox.min(0), p.oy.min(0)))
        }
    };
    if u64::from(raster.0) * u64::from(raster.1) > MAX_DOTS {
        return Err(refuse(SvgRefusal::TooManyDots {
            w: raster.0,
            h: raster.1,
        }));
    }
    let mut pm = Pixmap::new(raster.0, raster.1).ok_or(ArtworkError::EmptyFrame)?;
    pm.fill(Color::WHITE);
    // Scale and translate only, never rotate: a turned element is turned by
    // index when it is drawn.
    let t = Transform::from_row(
        scaled.0 as f32 / size.width(),
        0.0,
        0.0,
        scaled.1 as f32 / size.height(),
        shift.0 as f32,
        shift.1 as f32,
    );
    resvg::render(&tree, t, &mut pm.as_mut());

    // Rendered onto opaque white, every pixel stays opaque, so the
    // premultiplied channels are the colour.
    let mut stats = Stats::default();
    let px: Vec<u8> = pm
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let y = luma601(p[0], p[1], p[2]);
            stats.add(y, 255);
            y
        })
        .collect();
    if tree.root().has_children() && px.iter().all(|&v| v == 255) {
        notes.push(ImportNote::SvgDrewNothing);
    }
    Ok(Rendered {
        art: Artwork {
            w: raster.0,
            h: raster.1,
            px,
        },
        stats,
        natural,
        notes,
    })
}

fn options(dpmm: Dpmm) -> usvg::Options<'static> {
    usvg::Options {
        // Millimetres and inches land on the dot grid: 40 x 12 mm is 320 x 96
        // at 8 dots per millimetre.
        dpi: (25.4 * f64::from(dpmm.num) / f64::from(dpmm.den.max(1))) as f32,
        resources_dir: None,
        // Only where the file says `auto`; an explicit choice in the file
        // stands.
        shape_rendering: usvg::ShapeRendering::CrispEdges,
        // A second guard behind the refusal of `<image>`: nothing is ever
        // loaded, from a file or from a data URL.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// Count the groups resvg must draw into a layer of their own, and how deeply
/// they nest, including those inside clip paths and masks.
fn isolation(g: &usvg::Group, depth: usize, count: &mut usize, deepest: &mut usize) {
    let depth = if g.should_isolate() {
        *count += 1;
        depth + 1
    } else {
        depth
    };
    *deepest = (*deepest).max(depth);
    for child in g.children() {
        if let usvg::Node::Group(cg) = child {
            isolation(cg, depth, count, deepest);
        }
        child.subroots(|sg| isolation(sg, depth, count, deepest));
    }
}

/// Add up the dashes every dashed stroke will be cut into, as tiny-skia counts
/// them: a path's length over its dash period, times the dashes in a period.
/// Both are in the path's own units, so no transform changes the count. The
/// length is each segment's control polygon, which is never shorter than the
/// curve, so a count that passes here passes there.
fn count_dashes(g: &usvg::Group, total: &mut f64) {
    for child in g.children() {
        match child {
            usvg::Node::Group(cg) => count_dashes(cg, total),
            usvg::Node::Path(p) => {
                if let Some(array) = p.stroke().and_then(|s| s.dasharray()) {
                    let period: f64 = array.iter().map(|&v| f64::from(v)).sum();
                    let per = (array.len() / 2) as f64;
                    *total += outline_length(p.data()) * per / period;
                }
            }
            _ => {}
        }
        child.subroots(|sg| count_dashes(sg, total));
    }
}

/// The length of every segment's control polygon, closing lines included.
fn outline_length(path: &usvg::tiny_skia_path::Path) -> f64 {
    use usvg::tiny_skia_path::{PathSegment, Point};
    let dist = |a: Point, b: Point| f64::from(a.distance(b));
    let (mut len, mut at, mut start) = (0.0, Point::zero(), Point::zero());
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(p) => {
                (at, start) = (p, p);
            }
            PathSegment::LineTo(p) => {
                len += dist(at, p);
                at = p;
            }
            PathSegment::QuadTo(c, p) => {
                len += dist(at, c) + dist(c, p);
                at = p;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                len += dist(at, c1) + dist(c1, c2) + dist(c2, p);
                at = p;
            }
            PathSegment::Close => {
                len += dist(at, start);
                at = start;
            }
        }
    }
    len
}

/// A length in dots, at least one; `as` saturates and takes NaN to zero.
fn dots(v: f32) -> u32 {
    (v.round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// The second guard behind the refusal of `<image>`, reached directly:
    /// with these options, nothing is loaded from a data URL or a file.
    #[test]
    fn options_load_nothing() {
        let png = Artwork {
            w: 1,
            h: 1,
            px: vec![0],
        }
        .to_png()
        .unwrap();
        let file = std::env::temp_dir().join(format!("relabel-svg-{}.png", std::process::id()));
        std::fs::write(&file, &png).unwrap();
        let data = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        for href in [
            data.as_str(),
            file.to_str().unwrap(),
            "relabel-svg-nearby.png",
        ] {
            let text = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="{href}" width="8" height="8"/></svg>"#
            );
            let doc = usvg::roxmltree::Document::parse(&text).unwrap();
            let tree = usvg::Tree::from_xmltree(&doc, &options(Dpmm::DPI_203)).unwrap();
            assert!(!tree.root().has_children(), "{href} was loaded");
        }
        let _ = std::fs::remove_file(&file);
    }
}
