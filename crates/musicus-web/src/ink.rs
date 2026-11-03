//! Drawing annotation layers on a canvas and hit-testing them.
//!
//! Layers store page-relative coordinates (0..=65535 on both axes, see
//! `musicus_api::ink`); this module maps them to the CSS pixels of the page
//! as currently displayed.

use musicus_api::ink::{COORD_MAX, InkObject, PageLayer, Stamp, Stroke, TextNote, Tool};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

/// Size of the displayed page in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PageBox {
    pub width: f64,
    pub height: f64,
}

impl PageBox {
    pub fn to_page(self, x: f64, y: f64) -> (u16, u16) {
        let nx = (x / self.width * COORD_MAX as f64).clamp(0.0, COORD_MAX as f64);
        let ny = (y / self.height * COORD_MAX as f64).clamp(0.0, COORD_MAX as f64);
        (nx.round() as u16, ny.round() as u16)
    }

    pub fn to_css(self, p: (u16, u16)) -> (f64, f64) {
        (p.0 as f64 / COORD_MAX as f64 * self.width, p.1 as f64 / COORD_MAX as f64 * self.height)
    }

    /// A length in page-width units, in CSS pixels.
    pub fn len_css(self, units: u16) -> f64 {
        units as f64 / COORD_MAX as f64 * self.width
    }
}

pub fn css_color(rgba: u32, alpha_scale: f64) -> String {
    let (r, g, b, a) = ((rgba >> 24) & 0xff, (rgba >> 16) & 0xff, (rgba >> 8) & 0xff, rgba & 0xff);
    format!("rgba({r},{g},{b},{:.3})", a as f64 / 255.0 * alpha_scale)
}

/// The 2D context of a canvas, resized to the page box at device resolution
/// and cleared. Returns `None` if the canvas is not in the DOM.
pub fn prepare(canvas_id: &str, page: PageBox) -> Option<CanvasRenderingContext2d> {
    let window = web_sys::window()?;
    let canvas: HtmlCanvasElement = window.document()?.get_element_by_id(canvas_id)?.dyn_into().ok()?;
    let dpr = window.device_pixel_ratio();
    let (w, h) = ((page.width * dpr).round() as u32, (page.height * dpr).round() as u32);
    if canvas.width() != w || canvas.height() != h {
        canvas.set_width(w);
        canvas.set_height(h);
    }
    let style = canvas.style();
    let _ = style.set_property("width", &format!("{}px", page.width));
    let _ = style.set_property("height", &format!("{}px", page.height));
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.clear_rect(0.0, 0.0, page.width, page.height);
    Some(ctx)
}

/// The context without clearing, for incremental live drawing.
pub fn context(canvas_id: &str) -> Option<CanvasRenderingContext2d> {
    let canvas: HtmlCanvasElement = web_sys::window()?.document()?.get_element_by_id(canvas_id)?.dyn_into().ok()?;
    canvas.get_context("2d").ok()??.dyn_into().ok()
}

/// Redraws a whole layer: highlighters first, so pen marks stay on top.
pub fn draw_layer(canvas_id: &str, page: PageBox, layer: &PageLayer) {
    let Some(ctx) = prepare(canvas_id, page) else { return };
    for object in &layer.objects {
        if let InkObject::Stroke(s) = object
            && s.tool == Tool::Highlighter
        {
            draw_stroke(&ctx, page, s);
        }
    }
    for object in &layer.objects {
        match object {
            InkObject::Stroke(s) if s.tool == Tool::Pen => draw_stroke(&ctx, page, s),
            InkObject::Stamp(s) => draw_stamp(&ctx, page, s),
            InkObject::Text(t) => draw_text(&ctx, page, t),
            _ => {}
        }
    }
}

pub fn pen_width_css(page: PageBox, width: u16, pressure: u8) -> f64 {
    // Pressure 0 means "unknown" (mouse); treat it as medium.
    let p = if pressure == 0 { 0.5 } else { pressure as f64 / 255.0 };
    (page.len_css(width) * (0.55 + 0.9 * p)).max(0.6)
}

fn draw_stroke(ctx: &CanvasRenderingContext2d, page: PageBox, s: &Stroke) {
    let points = s.points();
    ctx.set_line_cap("round");
    ctx.set_line_join("round");
    match s.tool {
        Tool::Highlighter => {
            ctx.set_global_alpha(0.35);
            ctx.set_stroke_style_str(&css_color(s.color, 1.0));
            ctx.set_line_width(page.len_css(s.width));
            ctx.begin_path();
            let (x, y) = page.to_css(points[0]);
            ctx.move_to(x, y);
            for p in &points[1..] {
                let (x, y) = page.to_css(*p);
                ctx.line_to(x, y);
            }
            if points.len() == 1 {
                ctx.line_to(x + 0.01, y);
            }
            ctx.stroke();
            ctx.set_global_alpha(1.0);
        }
        Tool::Pen => {
            ctx.set_stroke_style_str(&css_color(s.color, 1.0));
            if points.len() == 1 {
                let (x, y) = page.to_css(points[0]);
                ctx.set_fill_style_str(&css_color(s.color, 1.0));
                ctx.begin_path();
                let _ = ctx.arc(x, y, pen_width_css(page, s.width, s.pressure[0]) / 2.0, 0.0, std::f64::consts::TAU);
                ctx.fill();
                return;
            }
            for (i, w) in points.windows(2).enumerate() {
                segment(ctx, page.to_css(w[0]), page.to_css(w[1]), pen_width_css(page, s.width, s.pressure[i + 1]));
            }
        }
    }
}

/// One pen segment; also used while the pen is still moving.
pub fn segment(ctx: &CanvasRenderingContext2d, a: (f64, f64), b: (f64, f64), width: f64) {
    ctx.set_line_width(width);
    ctx.begin_path();
    ctx.move_to(a.0, a.1);
    ctx.line_to(b.0, b.1);
    ctx.stroke();
}

fn draw_stamp(ctx: &CanvasRenderingContext2d, page: PageBox, s: &Stamp) {
    let (x, y) = page.to_css((s.at[0], s.at[1]));
    let size = page.len_css(s.size);
    ctx.set_fill_style_str(&css_color(s.color, 1.0));
    ctx.set_font(&format!("{size:.1}px serif"));
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text(s.kind.glyph(), x, y);
}

fn draw_text(ctx: &CanvasRenderingContext2d, page: PageBox, t: &TextNote) {
    let (x, y) = page.to_css((t.at[0], t.at[1]));
    let size = page.len_css(t.size);
    ctx.set_fill_style_str(&css_color(t.color, 1.0));
    ctx.set_font(&format!("{size:.1}px sans-serif"));
    ctx.set_text_align("left");
    ctx.set_text_baseline("alphabetic");
    let _ = ctx.fill_text(&t.text, x, y);
}

fn distance_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Ids of the objects under the eraser at `p` (CSS pixels).
pub fn hit(page: PageBox, layer: &PageLayer, p: (f64, f64), radius: f64) -> Vec<u64> {
    layer
        .objects
        .iter()
        .filter(|o| match o {
            InkObject::Stroke(s) => {
                let pts: Vec<(f64, f64)> = s.points().into_iter().map(|q| page.to_css(q)).collect();
                let reach = radius + page.len_css(s.width) / 2.0;
                if pts.len() == 1 {
                    distance_to_segment(p, pts[0], pts[0]) <= reach
                } else {
                    pts.windows(2).any(|w| distance_to_segment(p, w[0], w[1]) <= reach)
                }
            }
            InkObject::Stamp(s) => {
                let c = page.to_css((s.at[0], s.at[1]));
                distance_to_segment(p, c, c) <= radius + page.len_css(s.size) / 2.0
            }
            InkObject::Text(t) => {
                let (x, y) = page.to_css((t.at[0], t.at[1]));
                let size = page.len_css(t.size);
                let w = t.text.chars().count() as f64 * size * 0.55;
                p.0 >= x - radius && p.0 <= x + w + radius && p.1 >= y - size - radius && p.1 <= y + radius
            }
        })
        .map(InkObject::id)
        .collect()
}

/// A random object id (unique enough within one page set).
pub fn new_id() -> u64 {
    let hi = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    let lo = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    (hi << 32) | lo
}
