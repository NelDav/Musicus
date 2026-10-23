//! Measures what a pure-Rust PDF renderer adds to a web client's download:
//! the smallest possible wasm export that renders one page to RGBA.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Renders `page` and returns the RGBA byte length; pixels stay in a leaked buffer.
#[unsafe(no_mangle)]
pub extern "C" fn render_page(ptr: *mut u8, len: usize, page: usize, scale: f32) -> usize {
    let data = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let Ok(pdf) = Pdf::new(data) else { return 0 };
    let Some(p) = pdf.pages().get(page) else { return 0 };
    let px = render(p, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(),
        &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE });
    let bytes = px.data_as_u8_slice().to_vec();
    let n = bytes.len();
    std::mem::forget(bytes);
    n
}
