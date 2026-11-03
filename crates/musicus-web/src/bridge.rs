//! Typed access to `js/bridge.js` (pdf.js, in-memory prefetch, wake lock).

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/js/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = prefetch)]
    fn js_prefetch(url: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = isCached)]
    pub fn is_cached(url: &str) -> bool;
    #[wasm_bindgen(js_name = pageCount)]
    fn js_page_count(url: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = renderPage)]
    fn js_render_page(url: &str, index: u32, canvas_id: &str, box_width: f64, box_height: f64) -> js_sys::Promise;
    #[wasm_bindgen(js_name = retainOnly)]
    fn js_retain_only(keep: js_sys::Array);
    #[wasm_bindgen(js_name = keepAwake)]
    fn js_keep_awake(on: bool) -> js_sys::Promise;
    #[wasm_bindgen(js_name = toggleFullscreen)]
    pub fn toggle_fullscreen();
    #[wasm_bindgen(js_name = copyText)]
    fn js_copy_text(text: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = setPending)]
    fn js_set_pending(key: &str, url: &str, revision: f64, bytes: &[u8]);
    #[wasm_bindgen(js_name = clearPending)]
    fn js_clear_pending(key: &str);
}

/// Registers the newest unsaved state of an annotation page, to be sent if
/// the tab closes before the regular (debounced) save.
pub fn set_pending(set: i64, page: u32, revision: i64, bytes: &[u8]) {
    let url = format!("/api/annotation-sets/{set}/pages/{page}");
    js_set_pending(&format!("{set}/{page}"), &url, revision as f64, bytes);
}

pub fn clear_pending(set: i64, page: u32) {
    js_clear_pending(&format!("{set}/{page}"));
}

fn message(e: JsValue) -> String {
    e.dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| "unknown error".into())
}

/// Downloads a file into memory (no-op if already there). The download
/// starts immediately, not when the future is first polled.
pub fn prefetch(url: &str) -> impl std::future::Future<Output = Result<(), String>> + use<> {
    let download = JsFuture::from(js_prefetch(url));
    async move { download.await.map(|_| ()).map_err(message) }
}

pub async fn page_count(url: &str) -> Result<u32, String> {
    let n = JsFuture::from(js_page_count(url)).await.map_err(message)?;
    Ok(n.as_f64().unwrap_or(0.0) as u32)
}

/// Renders a page into the canvas with id `canvas_id`, fitted into the box.
/// Returns the CSS size of the drawn page, `None` if superseded.
pub async fn render_page(
    url: &str,
    index: u32,
    canvas_id: &str,
    box_w: f64,
    box_h: f64,
) -> Result<Option<(f64, f64)>, String> {
    let v = JsFuture::from(js_render_page(url, index, canvas_id, box_w, box_h)).await.map_err(message)?;
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    let a = js_sys::Array::from(&v);
    Ok(Some((a.get(0).as_f64().unwrap_or(0.0), a.get(1).as_f64().unwrap_or(0.0))))
}

/// Frees memory of files no longer needed.
pub fn retain_only(urls: &[String]) {
    js_retain_only(urls.iter().map(|u| JsValue::from_str(u)).collect());
}

pub async fn keep_awake(on: bool) {
    if let Some(body) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.body()) {
        let _ = body.set_attribute("data-keep-awake", if on { "on" } else { "off" });
    }
    let _ = JsFuture::from(js_keep_awake(on)).await;
}

pub async fn copy_text(text: &str) -> bool {
    JsFuture::from(js_copy_text(text)).await.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}
