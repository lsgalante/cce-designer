//! The 2D page context — a printed sheet, composited from layers.
//!
//! This is a SECOND context, deliberately not the geometry graph. Its currency
//! is a [`Page`] rather than a `Detail`, its coordinates are inches rather than
//! world units, its origin is the top-left corner with y running DOWN, and
//! nothing in it has a point id, an attribute or a normal. The geometry graph
//! describes a thing you will make; a page describes a thing you will print.
//! Smuggling one into the other means a `Detail` that is secretly a raster and
//! a viewport that has to guess which it is holding, so they stay apart: page
//! nodes resolve through [`resolve_page`], never through
//! `generate_single_node_geometry_with_errors`, and contribute no geometry to
//! the viewport at all.
//!
//! Inches, not millimetres, because the page's own reason for existing is
//! paper, and paper is specified in inches by the sources this came from (8.5 ×
//! 11 is the default everywhere in the family). The World Unit declaration that
//! governs the geometry graph does not reach here — a sheet is a sheet at any
//! model scale.
//!
//! **Resolution is a property of the page, not of the export.** A page carries
//! its DPI, the raster is that many pixels per inch, and the PNG says so in its
//! pHYs chunk — so a printer, a slicer or a browser lays the file out at the
//! physical size it was composed at instead of guessing 96. A page composed at
//! 300 DPI and printed is 8.5 inches wide; the same pixels labelled 72 are
//! nearly four feet.

use std::path::Path;

/// Declare a PNG's pixels to be sRGB, the way the spec asks for.
///
/// `Encoder::set_srgb` did this in one call and is deprecated; its replacement
/// `set_source_srgb` writes ONLY the sRGB chunk, dropping the gAMA and cHRM
/// fallbacks that PNG 11.3.2.5 says to write beside it for decoders that do
/// not understand sRGB. Swapping one call for the other therefore changes the
/// file — silently, and only for old decoders, which is the worst way for a
/// deprecation fix to change behaviour. Both of this app's PNG writers go
/// through here instead, so they agree and neither one drifts.
pub fn mark_srgb<W: std::io::Write>(encoder: &mut png::Encoder<W>) {
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.set_source_gamma(png::ScaledFloat::from_scaled(45455));
    encoder.set_source_chromaticities(png::SourceChromaticities {
        white: (png::ScaledFloat::from_scaled(31270), png::ScaledFloat::from_scaled(32900)),
        red: (png::ScaledFloat::from_scaled(64000), png::ScaledFloat::from_scaled(33000)),
        green: (png::ScaledFloat::from_scaled(30000), png::ScaledFloat::from_scaled(60000)),
        blue: (png::ScaledFloat::from_scaled(15000), png::ScaledFloat::from_scaled(6000)),
    });
}

/// One printed sheet: a physical size, a resolution, and the pixels in between.
///
/// Pixels are straight-alpha linear RGBA. Straight rather than premultiplied
/// because every operation here composites INTO an opaque page, so the extra
/// multiply buys nothing and the stored values stay the ones the parameters
/// asked for.
#[derive(Clone)]
pub struct Page {
    /// Physical size in inches, before orientation is applied.
    pub size: [f32; 2],
    /// Pixels per inch.
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

/// The largest page anyone composes by accident: a 1000 DPI A0 sheet is about
/// 1.4 gigapixels, and the honest failure is a clamped resolution rather than
/// an allocation that takes the app down. Chosen as roughly 13 × 19 inches (a
/// large-format print) at 1200 DPI.
const MAX_PIXELS: u64 = 356_000_000;

impl Page {
    /// A blank sheet filled with `color`.
    ///
    /// The size is clamped to something a printer could accept rather than
    /// rejected: a page node whose size parameter is being dragged passes
    /// through zero, and a context that returns an error there flickers.
    pub fn new(size: [f32; 2], dpi: u32, color: [f32; 4]) -> Page {
        let size = [size[0].max(0.01), size[1].max(0.01)];
        let dpi = dpi.clamp(1, 2400);
        let mut width = (size[0] * dpi as f32).round().max(1.0) as u32;
        let mut height = (size[1] * dpi as f32).round().max(1.0) as u32;
        if width as u64 * height as u64 > MAX_PIXELS {
            // Scale both axes by the same factor so the aspect — the thing the
            // page size actually means — survives the clamp.
            let scale = (MAX_PIXELS as f64 / (width as f64 * height as f64)).sqrt();
            width = ((width as f64 * scale) as u32).max(1);
            height = ((height as f64 * scale) as u32).max(1);
        }
        Page { size, dpi, width, height, pixels: vec![color; (width * height) as usize] }
    }

    /// Pixels per inch as a float, measured from the raster rather than read
    /// from `dpi` — after a clamp those disagree, and every drawing operation
    /// wants the one that describes the pixels it is about to touch.
    pub fn scale(&self) -> f32 {
        self.width as f32 / self.size[0]
    }

    /// Paint the whole sheet.
    pub fn fill(&mut self, color: [f32; 4]) {
        for p in &mut self.pixels {
            *p = color;
        }
    }

    /// An axis-aligned rectangle in INCHES from the top-left corner.
    ///
    /// Coverage is exact area, not a coin flip on the pixel centre. A printed
    /// grid is mostly hairlines — a 0.01 inch rule at 300 DPI is three pixels,
    /// at 100 DPI is one — and a binary fill makes every line snap to whole
    /// pixels, so a ruled sheet comes out with lines alternating between one
    /// and two pixels wide down its length. The eye reads that as a wobble in
    /// the paper, not as aliasing, and it survives printing.
    pub fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
        let s = self.scale();
        let (px0, px1) = (x0.min(x1) * s, x0.max(x1) * s);
        let (py0, py1) = (y0.min(y1) * s, y0.max(y1) * s);
        if px1 <= 0.0 || py1 <= 0.0 || px0 >= self.width as f32 || py0 >= self.height as f32 {
            return;
        }
        let i0 = px0.floor().max(0.0) as u32;
        let j0 = py0.floor().max(0.0) as u32;
        let i1 = (px1.ceil() as i64).clamp(0, self.width as i64) as u32;
        let j1 = (py1.ceil() as i64).clamp(0, self.height as i64) as u32;
        for j in j0..j1 {
            let cy = (py1.min(j as f32 + 1.0) - py0.max(j as f32)).clamp(0.0, 1.0);
            if cy <= 0.0 {
                continue;
            }
            for i in i0..i1 {
                let cx = (px1.min(i as f32 + 1.0) - px0.max(i as f32)).clamp(0.0, 1.0);
                if cx > 0.0 {
                    self.blend(i, j, color, cx * cy);
                }
            }
        }
    }

    /// `color` over the pixel, its alpha scaled by `coverage`.
    fn blend(&mut self, x: u32, y: u32, color: [f32; 4], coverage: f32) {
        let a = color[3] * coverage;
        if a <= 0.0 {
            return;
        }
        let dst = &mut self.pixels[(y * self.width + x) as usize];
        let out_a = a + dst[3] * (1.0 - a);
        if out_a <= 0.0 {
            *dst = [0.0; 4];
            return;
        }
        for c in 0..3 {
            // Straight alpha, so the destination's contribution is weighted by
            // its own alpha and the result divided back out.
            dst[c] = (color[c] * a + dst[c] * dst[3] * (1.0 - a)) / out_a;
        }
        dst[3] = out_a;
    }

    /// A ruled grid: cells of `cell` inches filled with `cell_color`, ruled
    /// with lines `thickness` inches wide in `line_color`.
    ///
    /// Lines are centred ON their coordinate, not placed beside it, so a grid
    /// and a second grid at twice the cell size share their rules exactly
    /// instead of straddling them — which is the whole point of drawing two.
    /// The sheet's own edges are ruled too: a grid that stops one line short
    /// looks like a mistake rather than a margin.
    pub fn grid(&mut self, cell: f32, thickness: f32, cell_color: [f32; 4], line_color: [f32; 4]) {
        if cell_color[3] > 0.0 {
            self.rect(0.0, 0.0, self.size[0], self.size[1], cell_color);
        }
        let cell = cell.max(1.0 / self.scale());
        let half = (thickness * 0.5).max(0.25 / self.scale());
        let mut x = 0.0;
        while x <= self.size[0] + 1e-4 {
            self.rect(x - half, 0.0, x + half, self.size[1], line_color);
            x += cell;
        }
        let mut y = 0.0;
        while y <= self.size[1] + 1e-4 {
            self.rect(0.0, y - half, self.size[0], y + half, line_color);
            y += cell;
        }
    }

    /// A border `width` inches wide, drawn INSIDE the sheet's edge.
    ///
    /// Inside rather than centred on the edge, because half a border off the
    /// paper is half a border: the printed page has no bleed here, and a
    /// parameter that says 0.5 inches should put 0.5 inches of ink on the
    /// sheet.
    pub fn border(&mut self, width: f32, inset: f32, color: [f32; 4]) {
        let (w, h) = (self.size[0], self.size[1]);
        let width = width.max(0.0).min(w.min(h) * 0.5);
        let (a, b) = (inset, inset + width);
        self.rect(a, a, w - a, b, color);
        self.rect(a, h - b, w - a, h - a, color);
        self.rect(a, b, b, h - b, color);
        self.rect(w - b, b, w - a, h - b, color);
    }

    /// The page as 8-bit sRGB RGBA, row-major from the top — what both the GPU
    /// upload and the PNG encoder want.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            for c in 0..4 {
                out.push((p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }

    /// Write a PNG that knows its own physical size.
    ///
    /// The pHYs chunk carries pixels per METRE, which is the only unit PNG
    /// offers — so the DPI round-trips through a conversion and comes back a
    /// hair off. That is the format's limit, not a bug to chase: 300 DPI
    /// stores as 11811 px/m and reads back as 299.9994.
    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        mark_srgb(&mut encoder);
        let per_metre = (self.scale() * 39.370_08).round() as u32;
        encoder.set_pixel_dims(Some(png::PixelDimensions {
            xppu: per_metre,
            yppu: per_metre,
            unit: png::Unit::Meter,
        }));
        let mut writer = encoder.write_header().map_err(|e| format!("png header: {e}"))?;
        writer
            .write_image_data(&self.to_rgba8())
            .map_err(|e| format!("png write: {e}"))?;
        writer.finish().map_err(|e| format!("png finish: {e}"))?;
        Ok(())
    }
}

/// Where a run of text sits in the box it is given.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

/// Everything the text operation needs. A struct rather than a dozen arguments
/// because the node has a dozen parameters and threading them positionally is
/// how the wrong two get swapped.
pub struct TextSpec<'a> {
    pub text: &'a str,
    pub font: &'a str,
    /// Cap height in inches — a "0.1 font size" on a printed page means a tenth
    /// of an inch of type, not a tenth of a pixel or of the sheet.
    pub size: f32,
    pub color: [f32; 4],
    /// Where the text box's own origin sits on the sheet, in inches.
    pub at: [f32; 2],
    pub halign: HAlign,
    pub valign: VAlign,
    /// Line spacing as a multiple of the font size.
    pub leading: f32,
}

impl Default for TextSpec<'_> {
    fn default() -> Self {
        TextSpec {
            text: "",
            font: "",
            size: 0.1,
            color: [0.0, 0.0, 0.0, 1.0],
            at: [0.5, 0.5],
            halign: HAlign::Left,
            valign: VAlign::Top,
            leading: 1.25,
        }
    }
}

impl Page {
    /// Draw shaped text onto the sheet.
    ///
    /// Shaping and rasterizing both come from cosmic-text, which the toolkit
    /// already owns — the alternative is a second font stack in the same
    /// process disagreeing with the first about what a font is called.
    ///
    /// The size is converted to pixels here, at the page's own scale, so the
    /// same page composed at 150 and at 600 DPI prints identical type at
    /// different sample counts. That is the property that makes resolution a
    /// page parameter rather than an export one.
    pub fn text(
        &mut self,
        fonts: &mut cce_ui::cosmic_text::FontSystem,
        cache: &mut cce_ui::cosmic_text::SwashCache,
        spec: &TextSpec,
    ) {
        use cce_ui::cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
        if spec.text.is_empty() || spec.size <= 0.0 {
            return;
        }
        let px = (spec.size * self.scale()).max(1.0);
        let mut buffer = Buffer::new(fonts, Metrics::new(px, px * spec.leading.max(0.1)));
        // No width or height limit: the box is measured from the shaped text
        // rather than the text wrapped into a box. A printed label that
        // silently wraps is worse than one that runs long, because the run-on
        // is visible and the wrap looks deliberate.
        buffer.set_size(fonts, None, None);
        let attrs = if spec.font.trim().is_empty() {
            Attrs::new()
        } else {
            Attrs::new().family(Family::Name(spec.font.trim()))
        };
        buffer.set_text(fonts, spec.text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(fonts, false);

        // Measure what was actually shaped, so alignment is against the ink
        // rather than against the requested size.
        let mut text_w: f32 = 0.0;
        let mut lines = 0.0f32;
        for run in buffer.layout_runs() {
            text_w = text_w.max(run.line_w);
            lines += 1.0;
        }
        let line_h = px * spec.leading.max(0.1);
        let text_h = lines.max(1.0) * line_h;

        let ox = spec.at[0] * self.scale()
            - match spec.halign {
                HAlign::Left => 0.0,
                HAlign::Center => text_w * 0.5,
                HAlign::Right => text_w,
            };
        let oy = spec.at[1] * self.scale()
            - match spec.valign {
                VAlign::Top => 0.0,
                VAlign::Middle => text_h * 0.5,
                VAlign::Bottom => text_h,
            };

        let color = cce_ui::cosmic_text::Color::rgba(255, 255, 255, 255);
        let (w, h) = (self.width as i32, self.height as i32);
        let mut hits: Vec<(u32, u32, f32)> = Vec::new();
        buffer.draw(fonts, cache, color, |x, y, gw, gh, c| {
            // cosmic-text hands back a filled rect per span; the alpha is the
            // glyph's coverage. The colour it carries is the one passed in,
            // which is why that is opaque white — the page's own colour is
            // applied here, so a coloured glyph is not double-tinted.
            let a = c.a() as f32 / 255.0;
            if a <= 0.0 {
                return;
            }
            for dy in 0..gh as i32 {
                for dx in 0..gw as i32 {
                    let (px, py) = (x + dx + ox as i32, y + dy + oy as i32);
                    if px >= 0 && py >= 0 && px < w && py < h {
                        hits.push((px as u32, py as u32, a));
                    }
                }
            }
        });
        for (x, y, a) in hits {
            self.blend(x, y, spec.color, a);
        }
    }
}

// ---------------------------------------------------------------------------
// The node chain
// ---------------------------------------------------------------------------

use crate::app::FsNode;
use crate::geometry::{find_node_by_name, node_param_f32, node_param_str, node_param_vec3};
use glam::Vec3;

/// Whether a node belongs to the page context rather than the geometry graph.
///
/// The two do not mix, and this is the one place that says so. A page node
/// contributes nothing to the viewport's geometry and a geometry node cannot
/// feed a page, so the network is really two networks sharing an editor — the
/// same way Houdini's contexts do, and for the same reason: a raster and a
/// mesh have no operation in common.
pub fn is_page_node(node_type: &str) -> bool {
    matches!(
        node_type.to_ascii_lowercase().as_str(),
        "page" | "page_grid" | "page_border" | "page_text"
    )
}

/// Named sheet sizes, in inches, portrait.
///
/// A4 is metric and converts to 8.268 × 11.693 — carried at that precision
/// rather than rounded, because a rounded A4 prints with a visible margin
/// error at the bottom of the sheet.
fn preset_size(name: &str) -> Option<[f32; 2]> {
    match name.trim().to_ascii_lowercase().as_str() {
        "letter" => Some([8.5, 11.0]),
        "a4" => Some([8.267_717, 11.692_913]),
        "legal" => Some([8.5, 14.0]),
        "tabloid" => Some([11.0, 17.0]),
        _ => None,
    }
}

fn color_of(node: &FsNode, name: &str, fallback: Vec3) -> [f32; 4] {
    let c = node_param_vec3(node, name, fallback);
    [c.x, c.y, c.z, 1.0]
}

fn toggle_of(node: &FsNode, name: &str) -> bool {
    matches!(node_param_str(node, name, "false").trim().to_ascii_lowercase().as_str(), "true" | "1" | "on")
}

/// Compose the page `target` describes, resolving its input chain.
///
/// `visited` guards cycles by node id exactly as the geometry resolvers do —
/// the page context is a second graph, not a second kind of graph.
pub fn resolve_page(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Page> {
    if visited.contains(&target.id) {
        return None;
    }
    visited.push(target.id.clone());

    let kind = target.node_type.to_ascii_lowercase();
    if kind == "page" {
        let preset = node_param_str(target, "Preset", "Letter");
        let size = preset_size(&preset).unwrap_or([
            node_param_f32(target, "Width", 8.5),
            node_param_f32(target, "Height", 11.0),
        ]);
        // Landscape is the same sheet turned, not a different sheet: swap the
        // axes rather than asking for a second pair of numbers.
        let size = if node_param_str(target, "Orientation", "Portrait").eq_ignore_ascii_case("Landscape")
        {
            [size[1], size[0]]
        } else {
            size
        };
        let dpi = node_param_f32(target, "Resolution", 300.0).round().max(1.0) as u32;
        return Some(Page::new(size, dpi, color_of(target, "Color", Vec3::ONE)));
    }

    // Everything else composites onto its input, so a chain with no page at
    // the bottom of it has nothing to draw on and resolves to nothing. That is
    // the honest answer: a border with no page is not a page with a border —
    // and it is also how an Export node in a GEOMETRY chain falls through to
    // the geometry resolvers rather than being claimed by this one.
    let input = find_node_by_name(root, node_param_str(target, "Input", "").trim())?;
    let mut page = resolve_page(root, input, visited)?;

    match kind.as_str() {
        // Export belongs to neither context and passes through both. What it
        // writes is decided by what reaches it: a page becomes a PNG, geometry
        // becomes the mesh format its Format parameter names.
        "export" => {}
        "page_grid" => {
            let cell_color = if toggle_of(target, "Fill Cells") {
                color_of(target, "Cell Color", Vec3::ONE)
            } else {
                [0.0; 4]
            };
            page.grid(
                node_param_f32(target, "Cell Size", 0.25),
                node_param_f32(target, "Line Width", 0.01),
                cell_color,
                color_of(target, "Line Color", Vec3::ZERO),
            );
        }
        "page_border" => page.border(
            node_param_f32(target, "Width", 0.06),
            node_param_f32(target, "Inset", 0.4),
            color_of(target, "Color", Vec3::ZERO),
        ),
        "page_text" => {
            let text = node_param_str(target, "Text", "");
            let font = node_param_str(target, "Font", "");
            let spec = TextSpec {
                text: &text,
                font: &font,
                size: node_param_f32(target, "Size", 0.25),
                color: color_of(target, "Color", Vec3::ZERO),
                at: [node_param_f32(target, "X", 4.25), node_param_f32(target, "Y", 0.8)],
                halign: match node_param_str(target, "Horizontal", "Center").as_str() {
                    "Left" => HAlign::Left,
                    "Right" => HAlign::Right,
                    _ => HAlign::Center,
                },
                valign: match node_param_str(target, "Vertical", "Top").as_str() {
                    "Middle" => VAlign::Middle,
                    "Bottom" => VAlign::Bottom,
                    _ => VAlign::Top,
                },
                leading: node_param_f32(target, "Leading", 1.25),
            };
            with_fonts(|fonts, cache| page.text(fonts, cache, &spec));
        }
        _ => return None,
    }
    Some(page)
}

/// The page context's font stack, created once.
///
/// System fonts included, because a page node names its font by family — the
/// source family's default is "Lato" — and a font stack that only knows the
/// bundled house faces would silently substitute for every named font a user
/// actually owns. Shaping the app's own widgets stays on the toolkit's
/// geometry font system; this one is for ink on paper.
fn with_fonts<R>(
    f: impl FnOnce(&mut cce_ui::cosmic_text::FontSystem, &mut cce_ui::cosmic_text::SwashCache) -> R,
) -> R {
    use std::sync::{Mutex, OnceLock};
    type Stack = (cce_ui::cosmic_text::FontSystem, cce_ui::cosmic_text::SwashCache);
    static FONTS: OnceLock<Mutex<Stack>> = OnceLock::new();
    let stack = FONTS.get_or_init(|| {
        Mutex::new((
            cce_ui::create_font_system_with_system_fonts(),
            cce_ui::cosmic_text::SwashCache::new(),
        ))
    });
    let mut guard = stack.lock().unwrap();
    let (fonts, cache) = &mut *guard;
    f(fonts, cache)
}

/// The page a network level displays, if it displays one.
///
/// The same rule the viewport follows for geometry: draw what is visible at
/// the level being shown. Several page chains at one level is ambiguous, so
/// the LAST visible page node wins — the one furthest down the roster, which
/// is the one most recently added.
pub fn displayed_page(root: &FsNode, level: &FsNode) -> Option<Page> {
    let target = level
        .children
        .iter()
        .filter(|c| is_page_node(&c.node_type) && c.geometry_visible)
        .next_back()?;
    resolve_page(root, target, &mut Vec::new())
}

/// The font stack, for tests that draw text without a node behind them.
#[cfg(test)]
pub fn with_fonts_for_test<R>(
    f: impl FnOnce(&mut cce_ui::cosmic_text::FontSystem, &mut cce_ui::cosmic_text::SwashCache) -> R,
) -> R {
    with_fonts(f)
}
