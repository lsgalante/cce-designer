//! The 2D page context — an image, composited from layers.
//!
//! This is a SECOND context, deliberately not the geometry graph. Its currency
//! is a [`Page`] rather than a `Detail`, its origin is the top-left corner
//! with y running DOWN, and nothing in it has a point id, an attribute or a
//! normal. Smuggling one into the other means a `Detail` that is secretly a
//! raster, so they stay apart: page nodes resolve through [`resolve_page`],
//! never through `generate_single_node_geometry_with_errors`, and contribute
//! no geometry. What the viewport shows of a page is the page itself, as a
//! textured quad standing in the scene ([`PageShown::world_corners`]).
//!
//! A page is stored in inches, because its first reason for existing was
//! paper. What its nodes are WRITTEN in is the page's [`PageUnit`] — inches,
//! millimetres, centimetres or pixels — chosen on the `page` node and carried
//! by the page to every node downstream, so a shape on a 1920 × 1080 image is
//! placed in pixels and the same node on a Letter sheet in inches. In the
//! scene a page is its physical size: the World Unit says what one world unit
//! is, and a sheet 215.9 mm wide is 215.9 of them when that is a millimetre.
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
    /// What the lengths on this page's nodes are written in.
    pub unit: PageUnit,
    /// Where the page's CENTRE stands in the scene, in world units.
    pub origin: [f32; 3],
}

/// What a page's lengths are written in.
///
/// A property of the PAGE, set on the node that makes it, and not of each
/// node drawing on it: a chain whose text was placed in pixels and whose
/// border was inset in inches is a chain nobody can read.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PageUnit {
    Inches,
    Millimetres,
    Centimetres,
    Pixels,
}

impl PageUnit {
    /// The unit a `Units` row names. Anything else is inches, which is what
    /// a page saved before the row existed was written in.
    pub fn parse(name: &str) -> PageUnit {
        match name.trim().to_ascii_lowercase().as_str() {
            "millimetres" | "millimeters" | "mm" => PageUnit::Millimetres,
            "centimetres" | "centimeters" | "cm" => PageUnit::Centimetres,
            "pixels" | "px" => PageUnit::Pixels,
            _ => PageUnit::Inches,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PageUnit::Inches => "in",
            PageUnit::Millimetres => "mm",
            PageUnit::Centimetres => "cm",
            PageUnit::Pixels => "px",
        }
    }

    /// A length in this unit as inches, on a raster of `px_per_inch`.
    pub fn to_inches(self, value: f32, px_per_inch: f32) -> f32 {
        match self {
            PageUnit::Inches => value,
            PageUnit::Millimetres => value / 25.4,
            PageUnit::Centimetres => value / 2.54,
            PageUnit::Pixels => value / px_per_inch.max(1e-6),
        }
    }

    /// The other way: inches as a length in this unit.
    pub fn from_inches(self, inches: f32, px_per_inch: f32) -> f32 {
        match self {
            PageUnit::Inches => inches,
            PageUnit::Millimetres => inches * 25.4,
            PageUnit::Centimetres => inches * 2.54,
            PageUnit::Pixels => inches * px_per_inch,
        }
    }
}

/// A page without its pixels: its size, its raster's size, its unit and
/// where it stands. What placing something ON a page needs, at the cost of
/// walking the chain to the `page` node and of nothing else — the viewport's
/// handles ask for it on every frame they are drawn, and composing the
/// raster to learn how big it is would be a sheet a frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageFrame {
    /// Physical size in inches.
    pub size: [f32; 2],
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    pub unit: PageUnit,
    /// The page's centre in the scene, world units.
    pub origin: [f32; 3],
}

impl PageFrame {
    /// The frame of a sheet `size` inches at `dpi`, clamped as a page is.
    ///
    /// The size is clamped to something a printer could accept rather than
    /// rejected: a page node whose size parameter is being dragged passes
    /// through zero, and a context that returns an error there flickers.
    pub fn new(size: [f32; 2], dpi: u32) -> PageFrame {
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
        PageFrame { size, dpi, width, height, unit: PageUnit::Inches, origin: [0.0; 3] }
    }

    /// Pixels per inch, measured from the raster.
    pub fn scale(&self) -> f32 {
        self.width as f32 / self.size[0]
    }

    /// World units to the inch, in a world of `world_unit_mm` millimetres.
    fn world_per_inch(world_unit_mm: f32) -> f32 {
        25.4 / world_unit_mm.max(1e-6)
    }

    /// A place on the page, in the page's unit from its top-left corner, as
    /// a place in the scene. The page's y runs down and the world's up.
    pub fn to_world(&self, at: [f32; 2], world_unit_mm: f32) -> glam::Vec3 {
        let k = Self::world_per_inch(world_unit_mm);
        let s = self.scale();
        let x = self.unit.to_inches(at[0], s) - self.size[0] * 0.5;
        let y = self.size[1] * 0.5 - self.unit.to_inches(at[1], s);
        glam::Vec3::new(self.origin[0] + x * k, self.origin[1] + y * k, self.origin[2])
    }

    /// The other way. A place off the page's plane is the place on it
    /// straight behind: the page faces +Z, and its depth says nothing.
    pub fn from_world(&self, p: glam::Vec3, world_unit_mm: f32) -> [f32; 2] {
        let k = Self::world_per_inch(world_unit_mm);
        let s = self.scale();
        let x = (p.x - self.origin[0]) / k + self.size[0] * 0.5;
        let y = self.size[1] * 0.5 - (p.y - self.origin[1]) / k;
        [self.unit.from_inches(x, s), self.unit.from_inches(y, s)]
    }

    /// A number for a row in the page's unit: whole pixels, and thousandths
    /// of anything longer.
    pub fn row(&self, value: f32) -> String {
        row_text(self.unit, value)
    }
}

/// A number for a row in `unit`: whole pixels, and thousandths of anything
/// longer ([`PageFrame::row`]).
pub fn row_text(unit: PageUnit, value: f32) -> String {
    if unit == PageUnit::Pixels {
        format!("{}", value.round())
    } else {
        let s = format!("{value:.3}");
        let s = s.trim_end_matches('0');
        // Two decimals at the least, as the templates write them.
        let decimals = s.len() - s.find('.').map_or(s.len(), |i| i + 1);
        format!("{s}{}", "0".repeat(2usize.saturating_sub(decimals)))
    }
}

/// The largest page anyone composes by accident: a 1000 DPI A0 sheet is about
/// 1.4 gigapixels, and the honest failure is a clamped resolution rather than
/// an allocation that takes the app down. Chosen as roughly 13 × 19 inches (a
/// large-format print) at 1200 DPI.
const MAX_PIXELS: u64 = 356_000_000;

impl Page {
    /// A blank sheet filled with `color`, clamped as [`PageFrame::new`]
    /// clamps it.
    pub fn new(size: [f32; 2], dpi: u32, color: [f32; 4]) -> Page {
        Page::blank(PageFrame::new(size, dpi), color)
    }

    /// The blank sheet a frame describes.
    pub fn blank(frame: PageFrame, color: [f32; 4]) -> Page {
        Page {
            size: frame.size,
            dpi: frame.dpi,
            width: frame.width,
            height: frame.height,
            pixels: vec![color; (frame.width * frame.height) as usize],
            unit: frame.unit,
            origin: frame.origin,
        }
    }

    /// A length written in the page's unit, in inches.
    pub fn len(&self, value: f32) -> f32 {
        self.unit.to_inches(value, self.scale())
    }

    /// Inches as a length in the page's unit — what a node's row would say.
    pub fn in_unit(&self, inches: f32) -> f32 {
        self.unit.from_inches(inches, self.scale())
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

/// The page the viewport is showing, without its pixels: enough to place
/// the image in the scene and to fit a camera to it.
#[derive(Clone, PartialEq, Debug)]
pub struct PageShown {
    /// The displayed page node's id.
    pub node_id: String,
    /// Physical size in inches.
    pub size: [f32; 2],
    /// The raster's size in pixels.
    pub pixels: (u32, u32),
    /// The page's centre in the scene, world units.
    pub origin: [f32; 3],
}

impl PageShown {
    /// The image's size in the scene, in world units of `world_unit_mm`
    /// millimetres each. The World Unit is a declaration, and a page is the
    /// one thing in the scene that knows its own physical size, so this is
    /// the one place a length is converted INTO world units.
    pub fn world_size(&self, world_unit_mm: f32) -> [f32; 2] {
        let k = 25.4 / world_unit_mm.max(1e-6);
        [self.size[0] * k, self.size[1] * k]
    }

    /// The image's corners in the scene — top-left, top-right, bottom-right,
    /// bottom-left — standing in the XY plane about its origin and facing
    /// +Z. The image's y runs down and the world's up, so the top edge is +Y.
    pub fn world_corners(&self, world_unit_mm: f32) -> [[f32; 3]; 4] {
        let [w, h] = self.world_size(world_unit_mm);
        let [x, y, z] = self.origin;
        let (hw, hh) = (w * 0.5, h * 0.5);
        [
            [x - hw, y + hh, z],
            [x + hw, y + hh, z],
            [x + hw, y - hh, z],
            [x - hw, y - hh, z],
        ]
    }
}

/// Which outline a shape is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    /// A straight stroke `size[0]` long, drawn in the stroke colour at the
    /// stroke width, through the centre.
    Line,
    /// A polygon of `sides` corners on the ellipse the box holds, the first
    /// at the top.
    Polygon,
}

impl ShapeKind {
    pub fn parse(name: &str) -> ShapeKind {
        match name.trim().to_ascii_lowercase().as_str() {
            "ellipse" => ShapeKind::Ellipse,
            "line" => ShapeKind::Line,
            "polygon" => ShapeKind::Polygon,
            _ => ShapeKind::Rectangle,
        }
    }
}

/// One shape, in INCHES from the page's top-left corner, as every drawing
/// operation here is; the node converts from the page's unit.
pub struct ShapeSpec {
    pub kind: ShapeKind,
    pub center: [f32; 2],
    /// The box the shape fills, before it is turned.
    pub size: [f32; 2],
    /// Degrees, clockwise as the page is looked at.
    pub rotation: f32,
    /// Rectangle only.
    pub corner_radius: f32,
    /// Polygon only.
    pub sides: u32,
    /// None draws no fill.
    pub fill: Option<[f32; 4]>,
    /// Colour and width; the stroke is centred on the outline, half of it
    /// inside the shape and half out, as a drawing program's is.
    pub stroke: Option<([f32; 4], f32)>,
}

/// Signed distance from `p` to a closed polygon, negative inside.
fn polygon_distance(p: [f32; 2], corners: &[[f32; 2]]) -> f32 {
    let n = corners.len();
    let mut d = f32::MAX;
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (corners[i], corners[(i + 1) % n]);
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let (wx, wy) = (p[0] - a[0], p[1] - a[1]);
        let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey).max(1e-12)).clamp(0.0, 1.0);
        let (dx, dy) = (wx - ex * t, wy - ey * t);
        d = d.min(dx * dx + dy * dy);
        // Even-odd crossing of the ray to the right of `p`.
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * ex;
            if x > p[0] {
                inside = !inside;
            }
        }
    }
    if inside { -d.sqrt() } else { d.sqrt() }
}

impl Page {
    /// Draw one shape onto the page.
    ///
    /// Coverage comes from the signed DISTANCE to the outline, in pixels: a
    /// pixel whose centre is half a pixel inside is covered, half a pixel
    /// outside is not, and between them it is covered in proportion. That is
    /// what makes a turned rectangle's edge, and a circle's, a clean line
    /// where a test of the pixel centre leaves a staircase.
    pub fn shape(&mut self, spec: &ShapeSpec) {
        let s = self.scale();
        let (cx, cy) = (spec.center[0] * s, spec.center[1] * s);
        let line = spec.kind == ShapeKind::Line;
        // A line is its stroke: a box as long as the line and as thick as
        // the stroke is wide, filled with the stroke's colour.
        let (fill, stroke) = if line {
            (spec.stroke.map(|(c, _)| c), None)
        } else {
            (spec.fill, spec.stroke.filter(|(_, w)| *w > 0.0))
        };
        let thickness = spec.stroke.map(|(_, w)| w).unwrap_or(0.0) * s;
        let hw = (spec.size[0].abs() * s * 0.5).max(0.0);
        let hh = if line { (thickness * 0.5).max(0.5) } else { spec.size[1].abs() * s * 0.5 };
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let stroke_half = stroke.map(|(_, w)| w * s * 0.5).unwrap_or(0.0);

        let corners: Vec<[f32; 2]> = if spec.kind == ShapeKind::Polygon {
            let n = spec.sides.clamp(3, 64);
            (0..n)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / n as f32;
                    [hw * a.sin(), -hh * a.cos()]
                })
                .collect()
        } else {
            Vec::new()
        };
        let radius = spec.corner_radius.max(0.0).min(spec.size[0].abs().min(spec.size[1].abs()) * 0.5) * s;
        let distance = |p: [f32; 2]| -> f32 {
            match spec.kind {
                ShapeKind::Rectangle | ShapeKind::Line => {
                    let r = if line { 0.0 } else { radius };
                    let (qx, qy) = (p[0].abs() - (hw - r), p[1].abs() - (hh - r));
                    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
                }
                ShapeKind::Ellipse => {
                    let (a, b) = (hw.max(1e-3), hh.max(1e-3));
                    let k0 = ((p[0] / a).powi(2) + (p[1] / b).powi(2)).sqrt();
                    let k1 = ((p[0] / (a * a)).powi(2) + (p[1] / (b * b)).powi(2)).sqrt();
                    if k1 < 1e-9 { -a.min(b) } else { k0 * (k0 - 1.0) / k1 }
                }
                ShapeKind::Polygon => polygon_distance(p, &corners),
            }
        };

        let (sin, cos) = spec.rotation.to_radians().sin_cos();
        let reach = (hw * hw + hh * hh).sqrt() + stroke_half + 1.0;
        let i0 = (cx - reach).floor().max(0.0) as i64;
        let j0 = (cy - reach).floor().max(0.0) as i64;
        let i1 = ((cx + reach).ceil() as i64).min(self.width as i64);
        let j1 = ((cy + reach).ceil() as i64).min(self.height as i64);
        for j in j0..j1 {
            for i in i0..i1 {
                let (dx, dy) = (i as f32 + 0.5 - cx, j as f32 + 0.5 - cy);
                // Into the shape's own frame: the page turned back.
                let p = [dx * cos + dy * sin, -dx * sin + dy * cos];
                let d = distance(p);
                if let Some(color) = fill {
                    let c = (0.5 - d).clamp(0.0, 1.0);
                    if c > 0.0 {
                        self.blend(i as u32, j as u32, color, c);
                    }
                }
                if let Some((color, _)) = stroke {
                    let c = (0.5 - (d.abs() - stroke_half)).clamp(0.0, 1.0);
                    if c > 0.0 {
                        self.blend(i as u32, j as u32, color, c);
                    }
                }
            }
        }
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
use crate::geometry::{node_param_f32, node_param_str, node_param_vec3};
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
        "page" | "page_grid" | "page_border" | "page_text" | "page_shape"
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

/// Named raster sizes, in PIXELS, landscape as screens are. What such an
/// image measures in inches is these over the page's Resolution.
fn preset_pixels(name: &str) -> Option<[f32; 2]> {
    match name.trim().to_ascii_lowercase().as_str() {
        "hd" => Some([1920.0, 1080.0]),
        "4k" => Some([3840.0, 2160.0]),
        "square" => Some([1024.0, 1024.0]),
        _ => None,
    }
}

/// A colour row and an opacity row as one straight-alpha colour.
fn color_with(node: &FsNode, name: &str, fallback: Vec3, opacity: &str) -> [f32; 4] {
    let c = node_param_vec3(node, name, fallback);
    [c.x, c.y, c.z, node_param_f32(node, opacity, 1.0).clamp(0.0, 1.0)]
}

fn color_of(node: &FsNode, name: &str, fallback: Vec3) -> [f32; 4] {
    let c = node_param_vec3(node, name, fallback);
    [c.x, c.y, c.z, 1.0]
}

fn toggle_of(node: &FsNode, name: &str) -> bool {
    crate::geometry::node_param_bool(node, name, false)
}

/// The frame a `page` node describes: Width by Height in its Units. The
/// Preset, Orientation and Units rows do not enter into it — they set
/// Width and Height when they are picked ([`follow_page_rows`]).
fn page_node_frame(target: &FsNode) -> PageFrame {
    let unit = PageUnit::parse(&node_param_str(target, "Units", "Inches"));
    let dpi = page_dpi(target);
    let size = [
        unit.to_inches(node_param_f32(target, "Width", 8.5), dpi),
        unit.to_inches(node_param_f32(target, "Height", 11.0), dpi),
    ];
    let mut frame = PageFrame::new(size, dpi as u32);
    frame.unit = unit;
    frame.origin = node_param_vec3(target, "Position", Vec3::ZERO).to_array();
    frame
}

fn page_dpi(node: &FsNode) -> f32 {
    node_param_f32(node, "Resolution", 300.0).round().clamp(1.0, 2400.0)
}

/// A preset's size in inches: a sheet turned by `landscape`, a raster
/// size as it lies, at `dpi`.
fn preset_inches(name: &str, dpi: f32, landscape: bool) -> Option<[f32; 2]> {
    if let Some([w, h]) = preset_size(name) {
        return Some(if landscape { [h, w] } else { [w, h] });
    }
    preset_pixels(name).map(|[w, h]| [w / dpi, h / dpi])
}

/// What a `page` node's Width and Height become when one of the rows that
/// SET them was just changed, `was` being that row as it stood before:
///
/// - **Preset** writes the preset's size, in the node's Units. A sheet is
///   turned by Orientation; a raster size (HD, 4K, Square) is written as
///   it lies, and Orientation is set to say which way that is.
/// - **Orientation** swaps Width and Height when they lie the other way.
/// - **Units** converts them from the old unit, so the sheet keeps its
///   size and only the numbers change.
///
/// A Width or Height that is an expression is left to it. Returns the rows
/// rewritten, as they were, for undo; empty when `was` is none of the
/// three or nothing moved. There is no Custom preset: a size typed into
/// Width and Height is the size, and Preset names what was last picked.
pub fn follow_page_rows(node: &mut FsNode, was: &crate::app::ParamDef) -> Vec<crate::app::ParamDef> {
    let unit = PageUnit::parse(&node_param_str(node, "Units", "Inches"));
    let dpi = page_dpi(node);
    let (w, h) = (node_param_f32(node, "Width", 8.5), node_param_f32(node, "Height", 11.0));
    let landscape = node_param_str(node, "Orientation", "Portrait").eq_ignore_ascii_case("Landscape");
    let mut orientation = None;
    let (nw, nh) = match was.name.as_str() {
        "Preset" => {
            let preset = node_param_str(node, "Preset", "Letter");
            let Some([iw, ih]) = preset_inches(&preset, dpi, landscape) else { return Vec::new() };
            if preset_size(&preset).is_none() && iw != ih {
                orientation = Some(if iw > ih { "Landscape" } else { "Portrait" });
            }
            (unit.from_inches(iw, dpi), unit.from_inches(ih, dpi))
        }
        "Orientation" if w != h && landscape != (w > h) => (h, w),
        "Units" => {
            let old = PageUnit::parse(was.text());
            (unit.from_inches(old.to_inches(w, dpi), dpi), unit.from_inches(old.to_inches(h, dpi), dpi))
        }
        _ => return Vec::new(),
    };
    let mut rewritten = Vec::new();
    let mut set = |name: &str, text: String| {
        if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
            if !p.is_expr() && p.text() != text {
                rewritten.push(p.clone());
                p.set_text(text);
            }
        }
    };
    set("Width", row_text(unit, nw));
    set("Height", row_text(unit, nh));
    if let Some(o) = orientation {
        set("Orientation", o.to_string());
    }
    rewritten
}

/// A `page` node from before Width and Height were always the size: its
/// named preset is written into them once, and a Custom one — whose Width
/// and Height already were the size — names Letter. Told apart by the
/// Width row's `show_when`, which such a save carries as `Preset ==
/// Custom` until the template merge replaces it.
pub fn migrate_preset_rows(node: &mut FsNode) {
    let old = node.params.iter().any(|p| p.name == "Width" && p.show_when.contains("Custom"));
    if !old {
        return;
    }
    let Some(preset) = node.params.iter().find(|p| p.name == "Preset").cloned() else { return };
    if preset.text().trim().eq_ignore_ascii_case("custom") {
        if let Some(p) = node.params.iter_mut().find(|p| p.name == "Preset") {
            p.set_text("Letter".to_string());
        }
    } else {
        follow_page_rows(node, &preset);
    }
}

/// The frame of the page `target` draws on: [`resolve_page`]'s walk up the
/// chain, without the drawing. None where that would compose nothing — no
/// page at the bottom, or a wire that comes back to itself.
pub fn resolve_frame(root: &FsNode, target: &FsNode) -> Option<PageFrame> {
    let mut visited: Vec<&str> = Vec::new();
    let mut node = target;
    loop {
        if visited.contains(&node.id.as_str()) {
            return None;
        }
        visited.push(&node.id);
        let kind = node.node_type.to_ascii_lowercase();
        if kind == "page" && !crate::geometry::is_bypassed(node) {
            return Some(page_node_frame(node));
        }
        if !is_page_node(&kind) && kind != "export" {
            return None;
        }
        node = crate::geometry::param_node(root, node, "Input")?;
    }
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

    // Bypassed, a page node passes the sheet it was handed, and a sheet that
    // is bypassed is no sheet.
    if crate::geometry::is_bypassed(target) {
        let input = crate::geometry::param_node(root, target, "Input")?;
        return resolve_page(root, input, visited);
    }

    let kind = target.node_type.to_ascii_lowercase();
    if kind == "page" {
        return Some(Page::blank(
            page_node_frame(target),
            color_with(target, "Color", Vec3::ONE, "Opacity"),
        ));
    }

    // Everything else composites onto its input, so a chain with no page at
    // the bottom of it has nothing to draw on and resolves to nothing. That is
    // the honest answer: a border with no page is not a page with a border —
    // and it is also how an Export node in a GEOMETRY chain falls through to
    // the geometry resolvers rather than being claimed by this one.
    // Sibling-first like every geometry wire; a whole-tree search by name
    // found the first same-named page node anywhere.
    let input = crate::geometry::param_node(root, target, "Input")?;
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
                page.len(node_param_f32(target, "Cell Size", 0.25)),
                page.len(node_param_f32(target, "Line Width", 0.01)),
                cell_color,
                color_of(target, "Line Color", Vec3::ZERO),
            );
        }
        "page_border" => page.border(
            page.len(node_param_f32(target, "Width", 0.06)),
            page.len(node_param_f32(target, "Inset", 0.4)),
            color_of(target, "Color", Vec3::ZERO),
        ),
        "page_shape" => {
            let spec = ShapeSpec {
                kind: ShapeKind::parse(&node_param_str(target, "Shape", "Rectangle")),
                center: [
                    page.len(node_param_f32(target, "X", 0.0)),
                    page.len(node_param_f32(target, "Y", 0.0)),
                ],
                size: [
                    page.len(node_param_f32(target, "Width", 1.0)),
                    page.len(node_param_f32(target, "Height", 1.0)),
                ],
                rotation: node_param_f32(target, "Rotation", 0.0),
                corner_radius: page.len(node_param_f32(target, "Corner Radius", 0.0)),
                sides: node_param_f32(target, "Sides", 3.0).round().max(3.0) as u32,
                fill: toggle_of(target, "Fill")
                    .then(|| color_with(target, "Fill Color", Vec3::splat(0.5), "Fill Opacity")),
                stroke: toggle_of(target, "Stroke").then(|| {
                    (
                        color_with(target, "Stroke Color", Vec3::ZERO, "Stroke Opacity"),
                        page.len(node_param_f32(target, "Stroke Width", 0.02)),
                    )
                }),
            };
            page.shape(&spec);
        }
        "page_text" => {
            let text = node_param_str(target, "Text", "");
            let font = node_param_str(target, "Font", "");
            let spec = TextSpec {
                text: &text,
                font: &font,
                size: page.len(node_param_f32(target, "Size", 0.25)),
                color: color_of(target, "Color", Vec3::ZERO),
                at: [
                    page.len(node_param_f32(target, "X", 4.25)),
                    page.len(node_param_f32(target, "Y", 0.8)),
                ],
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
    resolve_page(root, displayed_page_node(level)?, &mut Vec::new())
}

/// The node [`displayed_page`] draws.
pub fn displayed_page_node(level: &FsNode) -> Option<&FsNode> {
    level
        .children
        .iter()
        .filter(|c| is_page_node(&c.node_type) && c.geometry_visible)
        .next_back()
}

/// The font stack, for tests that draw text without a node behind them.
#[cfg(test)]
pub fn with_fonts_for_test<R>(
    f: impl FnOnce(&mut cce_ui::cosmic_text::FontSystem, &mut cce_ui::cosmic_text::SwashCache) -> R,
) -> R {
    with_fonts(f)
}
