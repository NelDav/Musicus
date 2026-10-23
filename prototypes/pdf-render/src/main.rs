//! Assumption: a pure-Rust renderer (hayro) is fast enough to render sheet
//! music pages on the client, so the Dioxus client would not need pdf.js.
//!
//! Usage: pdf-render <file.pdf> [png-output-of-page-1]

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let data = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let pdf = Pdf::new(data).unwrap();
    println!("{path}: parsed in {:?}, {} pages", t.elapsed(), pdf.pages().len());

    let settings = InterpreterSettings::default();
    let cache = RenderCache::new();
    for scale in [1.5f32, 2.5] {
        let t = Instant::now();
        let mut last = None;
        for page in pdf.pages().iter() {
            let px = render(page, &cache, &settings, &RenderSettings::default(),
                &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE });
            last = Some((px.width(), px.height()));
        }
        let (w, h) = last.unwrap();
        println!("  scale {scale} ({w}x{h} px, {:.0} dpi): {:?} per page", 72.0 * scale, t.elapsed() / pdf.pages().len() as u32);
    }
    if let Some(out) = std::env::args().nth(2) {
        let page = &pdf.pages()[0];
        let px = render(page, &cache, &settings, &RenderSettings::default(),
            &PixmapSettings { x_scale: 1.0, y_scale: 1.0, bg_color: WHITE });
        std::fs::write(out, px.into_png().unwrap()).unwrap();
    }
}
