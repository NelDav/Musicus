//! The reader: one page at a time, fitted to the screen, with the user's
//! private annotation layer drawn on a canvas above the PDF.
//!
//! - Read mode: tap the left/right third, arrow keys, Page Up/Down or a
//!   Bluetooth foot pedal turn pages; past the last page the next piece of
//!   the programme opens.
//! - Draw mode: pen, highlighter, eraser, stamps and text. A pen in use
//!   disables finger drawing (palm rejection).
//!
//! Layers are saved per page 1.5 s after the last change and when turning
//! the page or leaving; a conflict with another device is merged by object
//! id (see `musicus_api::ink::merge`).

use crate::{
    Route,
    api::{self, PageBytes, SaveOutcome},
    bridge,
    ink::{self, PageBox},
    pages::library::part_route,
    ui::{ErrorBox, confirm, prompt},
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use gloo_net::http::Method;
use musicus_api::{
    AnnotationSet, NewAnnotationSet, Part, Programme, Score, UpdateAnnotationSet,
    ink::{self as model, InkObject, PageLayer, Stamp, StampKind, Stroke, TextNote, Tool},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
use wasm_bindgen::JsCast;

const PAGE_CANVAS: &str = "page-canvas";
const INK_CANVAS: &str = "ink-canvas";
const SAVE_DELAY_MS: u32 = 1500;
const ERASER_RADIUS: f64 = 10.0;

const COLORS: [(u32, &str); 4] =
    [(0xd0021bff, "Red"), (0x1f5fd6ff, "Blue"), (0x111111ff, "Black"), (0x1a8f3cff, "Green")];
const HIGHLIGHTS: [(u32, &str); 3] = [(0xffd60aff, "Yellow"), (0x7ee081ff, "Green"), (0xff8fd8ff, "Pink")];
/// Pen widths in page units (1/65535 of the page width).
const PEN_WIDTHS: [(u16, &str); 3] = [(110, "Fine"), (200, "Medium"), (380, "Bold")];
const HIGHLIGHT_WIDTH: u16 = 1400;
const STAMP_SIZE: u16 = 2200;
const TEXT_SIZE: u16 = 1300;

#[component]
pub fn Viewer(id: i64) -> Element {
    rsx! { ViewerPage { key: "{id}", id, pid: None } }
}

#[component]
pub fn ProgrammeViewer(pid: i64, id: i64) -> Element {
    rsx! { ViewerPage { key: "{pid}-{id}", id, pid: Some(pid) } }
}

#[derive(Clone, PartialEq)]
struct Loaded {
    part: Part,
    score: Score,
    sets: Vec<AnnotationSet>,
    programme: Option<Programme>,
}

async fn load(id: i64, pid: Option<i64>) -> api::ApiResult<Loaded> {
    let part: Part = api::get(&format!("/api/parts/{id}")).await?;
    let score: Score = api::get(&format!("/api/scores/{}", part.score_id)).await?;
    let sets: Vec<AnnotationSet> = api::get(&format!("/api/parts/{id}/annotation-sets")).await?;
    let programme = match pid {
        Some(p) => Some(api::get::<Programme>(&format!("/api/programmes/{p}")).await?),
        None => None,
    };
    Ok(Loaded { part, score, sets, programme })
}

#[component]
fn ViewerPage(id: i64, pid: Option<i64>) -> Element {
    let data = use_resource(move || load(id, pid));
    match &*data.read() {
        None => rsx! { div { class: "reader", div { class: "loading-page", "Loading…" } } },
        Some(Err(e)) => rsx! {
            main { class: "narrow",
                ErrorBox { message: e.message.clone() }
                Link { to: Route::Library {}, "Back to the library" }
            }
        },
        Some(Ok(d)) if d.part.file.is_none() => rsx! {
            main { class: "narrow",
                p { "No file has been uploaded for this part yet." }
                Link { to: Route::Library {}, "Back to the library" }
            }
        },
        Some(Ok(d)) => rsx! { Reader { data: d.clone() } },
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Read,
    Draw,
}

#[derive(Clone, Copy, PartialEq)]
enum ToolKind {
    Pen,
    Highlighter,
    Eraser,
    Stamp(StampKind),
    Text,
}

#[derive(Clone, PartialEq)]
enum SaveStatus {
    Idle,
    Pending,
    Saving,
    Saved,
    Failed(String),
}

enum Action {
    Add(InkObject),
    /// Removed objects with their former index.
    Remove(Vec<(usize, InkObject)>),
}

/// One page of one annotation set, as known to this tab.
#[derive(Default)]
struct PageState {
    layer: PageLayer,
    /// Server revision `layer` is based on (0: never saved).
    revision: i64,
    /// Object ids of that revision, the base of a three-way merge.
    base_ids: HashSet<u64>,
    loaded: bool,
    /// Bumped by every local change; equal to `saved_generation` when clean.
    generation: u64,
    saved_generation: u64,
    saving: bool,
    undo: Vec<Action>,
    redo: Vec<Action>,
}

/// (annotation set id, page index)
type Key = (i64, u32);

/// Annotation state outside the signal system, so that pending saves can
/// still be flushed when the reader closes.
#[derive(Default)]
struct Store {
    pages: HashMap<Key, PageState>,
}

type SharedStore = Rc<RefCell<Store>>;

fn ids(layer: &PageLayer) -> HashSet<u64> {
    layer.objects.iter().map(InkObject::id).collect()
}

async fn load_page(store: SharedStore, key: Key) -> Result<(), String> {
    if store.borrow().pages.get(&key).is_some_and(|s| s.loaded) {
        return Ok(());
    }
    let (server, revision) = match api::get_page(key.0, key.1 as i64).await.map_err(|e| e.message)? {
        PageBytes::Found { bytes, revision } => (model::decode(&bytes).map_err(|e| e.to_string())?, revision),
        PageBytes::Missing => (PageLayer::default(), 0),
    };
    let mut s = store.borrow_mut();
    let st = s.pages.entry(key).or_default();
    if !st.loaded {
        // Anything drawn before the load finished is kept.
        let local = std::mem::take(&mut st.layer);
        st.base_ids = ids(&server);
        st.layer = if local.objects.is_empty() { server } else { model::merge(&HashSet::new(), &local, &server) };
        st.revision = revision;
        st.loaded = true;
    }
    Ok(())
}

/// Saves a page until it is clean, merging on conflicts. Touches no
/// signals, so it may run after the reader was closed.
async fn save_page(store: SharedStore, key: Key) -> Result<(), String> {
    for _ in 0..5 {
        let (layer, revision, generation) = {
            let mut s = store.borrow_mut();
            let Some(st) = s.pages.get_mut(&key) else { return Ok(()) };
            if st.generation == st.saved_generation || st.saving || !st.loaded {
                return Ok(());
            }
            st.saving = true;
            (st.layer.clone(), st.revision, st.generation)
        };
        let outcome = api::put_page(key.0, key.1 as i64, revision, &model::encode(&layer)).await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                if let Some(st) = store.borrow_mut().pages.get_mut(&key) {
                    st.saving = false;
                }
                return Err(e.message);
            }
        };
        match outcome {
            SaveOutcome::Saved(new_revision) => {
                let mut s = store.borrow_mut();
                let st = s.pages.get_mut(&key).expect("page exists");
                st.saving = false;
                st.revision = new_revision;
                st.base_ids = ids(&layer);
                st.saved_generation = generation;
                let clean = st.generation == generation;
                drop(s);
                sync_pending(&store, key);
                if clean {
                    return Ok(());
                }
            }
            SaveOutcome::Conflict => {
                // Another device saved: fetch, merge, then save the merge.
                let server = api::get_page(key.0, key.1 as i64).await.map_err(|e| e.message);
                let mut s = store.borrow_mut();
                let st = s.pages.get_mut(&key).expect("page exists");
                st.saving = false;
                let (server_layer, server_revision) = match server? {
                    PageBytes::Found { bytes, revision } => {
                        (model::decode(&bytes).map_err(|e| e.to_string())?, revision)
                    }
                    PageBytes::Missing => (PageLayer::default(), 0),
                };
                st.layer = model::merge(&st.base_ids, &st.layer, &server_layer);
                st.base_ids = ids(&server_layer);
                st.revision = server_revision;
                st.generation += 1;
                drop(s);
                sync_pending(&store, key);
            }
        }
    }
    Err("could not save after several attempts".into())
}

/// Keeps the bridge's "send on tab close" copy of a page in sync.
fn sync_pending(store: &SharedStore, key: Key) {
    let s = store.borrow();
    match s.pages.get(&key) {
        Some(st) if st.generation != st.saved_generation => {
            bridge::set_pending(key.0, key.1, st.revision, &model::encode(&st.layer))
        }
        _ => bridge::clear_pending(key.0, key.1),
    }
}

fn dirty_keys(store: &SharedStore) -> Vec<Key> {
    store.borrow().pages.iter().filter(|(_, s)| s.generation != s.saved_generation).map(|(k, _)| *k).collect()
}

/// The set to show first: the one linked to the programme, else the default.
fn choose_set(sets: &[AnnotationSet], programme: Option<i64>) -> Option<i64> {
    programme
        .and_then(|p| sets.iter().find(|s| s.programme_id == Some(p)))
        .or_else(|| sets.iter().find(|s| s.is_default))
        .or_else(|| sets.first())
        .map(|s| s.id)
}

/// Previous and next piece of the programme, preferring the part that
/// shares an instrument with the current one.
fn neighbours(programme: &Programme, part: &Part) -> (Option<Route>, Option<Route>) {
    let Some(pos) = programme.items.iter().position(|i| i.score.parts.iter().any(|p| p.id == part.id)) else {
        return (None, None);
    };
    let pick = |idx: usize| -> Option<Route> {
        let parts: Vec<&Part> = programme.items.get(idx)?.score.parts.iter().filter(|p| p.file.is_some()).collect();
        let chosen = parts
            .iter()
            .find(|p| p.instrument_ids.iter().any(|i| part.instrument_ids.contains(i)))
            .or_else(|| parts.first())?;
        Some(part_route(chosen, Some(programme.id)))
    };
    let prev = (0..pos).rev().find_map(pick);
    let next = (pos + 1..programme.items.len()).find_map(pick);
    (prev, next)
}

#[derive(Default)]
struct Live {
    drawing: bool,
    erasing: bool,
    pointer: i32,
    points: Vec<(f64, f64, u8)>,
    pen_seen: bool,
}

#[component]
fn Reader(data: Loaded) -> Element {
    let nav = navigator();
    let part = data.part.clone();
    let current_version = part.file.as_ref().map(|f| f.version).unwrap_or(0);
    let programme_id = data.programme.as_ref().map(|p| p.id);
    let (prev_piece, next_piece) = data.programme.as_ref().map(|p| neighbours(p, &part)).unwrap_or((None, None));
    let prev_piece = use_signal(|| prev_piece);
    let next_piece = use_signal(|| next_piece);

    let mut sets = use_signal(|| data.sets.clone());
    let mut active = use_signal(|| choose_set(&data.sets, programme_id));
    let mut page = use_signal(|| 0u32);
    let mut page_total = use_signal(|| 0u32);
    let mut stage = use_signal(|| (0.0f64, 0.0f64));
    let mut page_box = use_signal(|| None::<PageBox>);
    let mut mode = use_signal(|| Mode::Read);
    let mut tool = use_signal(|| ToolKind::Pen);
    let mut color = use_signal(|| COLORS[0].0);
    let mut highlight = use_signal(|| HIGHLIGHTS[0].0);
    let mut pen_width = use_signal(|| PEN_WIDTHS[1].0);
    let mut show_ink = use_signal(|| true);
    let mut status = use_signal(|| SaveStatus::Idle);
    let mut error = use_signal(|| None::<String>);
    let mut menu = use_signal(|| false);
    // Bumped whenever the store changes, to redraw.
    let mut version = use_signal(|| 0u64);
    let store: SharedStore = use_hook(|| Rc::new(RefCell::new(Store::default())));
    let live = use_hook(|| Rc::new(RefCell::new(Live::default())));

    let current_sha = part.file.as_ref().map(|f| f.sha256.clone());
    let url = use_memo(move || {
        let sets = sets.read();
        active()
            .and_then(|a| sets.iter().find(|s| s.id == a))
            .map(|s| s.file_sha256.clone())
            .or_else(|| current_sha.clone())
            .map(|sha| api::file_url(&sha))
    });

    // Keep the screen on and prefetch the next piece.
    let next_url = data.programme.as_ref().and_then(|p| {
        let Some(Route::ProgrammeViewer { id, .. }) = next_piece.peek().clone() else { return None };
        p.items
            .iter()
            .flat_map(|i| i.score.parts.iter())
            .find(|q| q.id == id)?
            .file
            .as_ref()
            .map(|f| api::file_url(&f.sha256))
    });
    use_hook(move || {
        spawn(bridge::keep_awake(true));
        if let Some(u) = next_url {
            spawn(async move {
                let _ = bridge::prefetch(&u).await;
            });
        }
    });

    // Flush unsaved pages when the reader closes.
    use_drop({
        let store = store.clone();
        move || {
            let keys = dirty_keys(&store);
            let store = store.clone();
            spawn_forever(async move {
                for key in keys {
                    let _ = save_page(store.clone(), key).await;
                }
                bridge::keep_awake(false).await;
            });
        }
    });

    // Page count of the displayed file.
    use_effect(move || {
        let Some(u) = url() else { return };
        spawn(async move {
            match bridge::page_count(&u).await {
                Ok(n) => {
                    page_total.set(n);
                    if *page.peek() >= n {
                        page.set(n.saturating_sub(1));
                    }
                }
                Err(e) => error.set(Some(format!("Could not open the sheet: {e}"))),
            }
        });
    });

    // Render the PDF page whenever file, page or available space change.
    use_effect(move || {
        let (Some(u), (w, h), p) = (url(), stage(), page()) else { return };
        if w < 20.0 || h < 20.0 {
            return;
        }
        spawn(async move {
            match bridge::render_page(&u, p, PAGE_CANVAS, w - 8.0, h - 8.0).await {
                Ok(Some((width, height))) => page_box.set(Some(PageBox { width, height })),
                Ok(None) => {}
                Err(e) => error.set(Some(format!("Could not render the page: {e}"))),
            }
        });
    });

    // Load the annotation layer of the current page.
    use_effect({
        let store = store.clone();
        move || {
            let (Some(set), p) = (active(), page()) else { return };
            let store = store.clone();
            spawn(async move {
                match load_page(store, (set, p)).await {
                    Ok(()) => version += 1,
                    Err(e) => error.set(Some(format!("Could not load annotations: {e}"))),
                }
            });
        }
    });

    // Redraw the annotation layer.
    use_effect({
        let store = store.clone();
        move || {
            let _ = version();
            let Some(b) = page_box() else { return };
            let empty = PageLayer::default();
            let s = store.borrow();
            let layer = match (show_ink(), active()) {
                (true, Some(set)) => s.pages.get(&(set, page())).map(|p| &p.layer).unwrap_or(&empty),
                _ => &empty,
            };
            ink::draw_layer(INK_CANVAS, b, layer);
        }
    });

    // Saves one page now or after the debounce delay.
    let save = {
        let store = store.clone();
        move |key: Key, delay: bool| {
            let store = store.clone();
            let generation = store.borrow().pages.get(&key).map(|s| s.generation);
            status.set(SaveStatus::Pending);
            spawn(async move {
                if delay {
                    gloo_timers::future::TimeoutFuture::new(SAVE_DELAY_MS).await;
                    // A newer change scheduled its own save.
                    if store.borrow().pages.get(&key).map(|s| s.generation) != generation {
                        return;
                    }
                }
                status.set(SaveStatus::Saving);
                match save_page(store.clone(), key).await {
                    Ok(()) => status.set(SaveStatus::Saved),
                    Err(e) => status.set(SaveStatus::Failed(e)),
                }
                version += 1;
            });
        }
    };

    // Applies a change to the current page and schedules its save.
    let change = {
        let store = store.clone();
        let mut save = save.clone();
        move |key: Key, f: &dyn Fn(&mut PageState) -> Option<Action>| {
            let changed = {
                let mut s = store.borrow_mut();
                let st = s.pages.entry(key).or_default();
                match f(st) {
                    Some(action) => {
                        st.undo.push(action);
                        st.redo.clear();
                        st.generation += 1;
                        true
                    }
                    None => false,
                }
            };
            if changed {
                sync_pending(&store, key);
                version += 1;
                save(key, true);
            }
        }
    };

    // Adds an object; creates a first annotation set if there is none.
    let add_object = {
        let mut change = change.clone();
        let store = store.clone();
        let programme_title = data.programme.as_ref().map(|p| p.title.clone());
        let part_id = part.id;
        move |object: InkObject| {
            let p = page();
            if let Some(set) = active() {
                change((set, p), &|st| {
                    st.layer.objects.push(object.clone());
                    Some(Action::Add(object.clone()))
                });
                return;
            }
            let name = programme_title.clone().unwrap_or_else(|| "My annotations".into());
            let mut change = change.clone();
            let store = store.clone();
            spawn(async move {
                let req = NewAnnotationSet { name, programme_id, copy_from: None };
                match api::post::<_, AnnotationSet>(&format!("/api/parts/{part_id}/annotation-sets"), &req).await {
                    Ok(set) => {
                        let id = set.id;
                        sets.write().push(set);
                        store.borrow_mut().pages.entry((id, p)).or_default().loaded = true;
                        active.set(Some(id));
                        change((id, p), &|st| {
                            st.layer.objects.push(object.clone());
                            Some(Action::Add(object.clone()))
                        });
                    }
                    Err(e) => error.set(Some(format!("Could not create an annotation set: {}", e.message))),
                }
            });
        }
    };

    let erase_at = {
        let mut change = change.clone();
        let store = store.clone();
        move |x: f64, y: f64| {
            let (Some(set), Some(b)) = (active(), page_box()) else { return };
            let key = (set, page());
            let hits = store.borrow().pages.get(&key).map(|st| ink::hit(b, &st.layer, (x, y), ERASER_RADIUS));
            let Some(hits) = hits.filter(|h| !h.is_empty()) else { return };
            change(key, &|st| {
                let mut removed = Vec::new();
                let mut i = 0;
                st.layer.objects.retain(|o| {
                    let keep = !hits.contains(&o.id());
                    if !keep {
                        removed.push((i, o.clone()));
                    }
                    i += 1;
                    keep
                });
                Some(Action::Remove(removed))
            });
        }
    };

    let undo_redo = {
        let store = store.clone();
        let mut save = save.clone();
        move |redo: bool| {
            let Some(set) = active() else { return };
            let key = (set, page());
            {
                let mut s = store.borrow_mut();
                let Some(st) = s.pages.get_mut(&key) else { return };
                let Some(action) = (if redo { st.redo.pop() } else { st.undo.pop() }) else { return };
                let inverse = match action {
                    Action::Add(o) => {
                        let id = o.id();
                        let index = st.layer.objects.iter().position(|x| x.id() == id);
                        match index {
                            Some(i) => Action::Remove(vec![(i, st.layer.objects.remove(i))]),
                            None => return,
                        }
                    }
                    Action::Remove(list) => {
                        for (i, o) in &list {
                            let at = (*i).min(st.layer.objects.len());
                            st.layer.objects.insert(at, o.clone());
                        }
                        // Re-adding several objects is undone by removing them all.
                        match list.len() {
                            1 => Action::Add(list.into_iter().next().expect("one").1),
                            _ => Action::Remove(
                                list.into_iter()
                                    .map(|(_, o)| {
                                        let i = st.layer.objects.iter().position(|x| x.id() == o.id()).unwrap_or(0);
                                        (i, o)
                                    })
                                    .collect(),
                            ),
                        }
                    }
                };
                if redo {
                    st.undo.push(inverse)
                } else {
                    st.redo.push(inverse)
                }
                st.generation += 1;
            }
            sync_pending(&store, key);
            version += 1;
            save(key, true);
        }
    };

    // Page turning, also across pieces of a programme.
    let turn = {
        let store = store.clone();
        let mut save = save.clone();
        move |delta: i32| {
            let target = page() as i32 + delta;
            for key in dirty_keys(&store) {
                save(key, false);
            }
            if target < 0 {
                if let Some(r) = prev_piece() {
                    nav.push(r);
                }
            } else if target >= page_total() as i32 {
                if let Some(r) = next_piece() {
                    nav.push(r);
                }
            } else {
                page.set(target as u32);
            }
        }
    };

    // Keyboard and foot pedals (which send arrow / page keys).
    use_future({
        let turn = turn.clone();
        move || {
            let mut turn = turn.clone();
            async move {
                let mut keys = document::eval(
                    r#"
                    const wanted = ['ArrowRight', 'ArrowLeft', 'ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', ' '];
                    const handler = (e) => {
                        const tag = e.target && e.target.tagName;
                        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || !wanted.includes(e.key)) return;
                        e.preventDefault();
                        dioxus.send(e.key);
                    };
                    window.__musicusKeys && window.removeEventListener('keydown', window.__musicusKeys);
                    window.__musicusKeys = handler;
                    window.addEventListener('keydown', handler);
                    await new Promise(() => {});
                    "#,
                );
                while let Ok(key) = keys.recv::<String>().await {
                    match key.as_str() {
                        "ArrowLeft" | "ArrowUp" | "PageUp" => turn(-1),
                        _ => turn(1),
                    }
                }
            }
        }
    });
    use_drop(|| {
        document::eval(
            "window.__musicusKeys && window.removeEventListener('keydown', window.__musicusKeys); window.__musicusKeys = null;",
        );
    });

    // --- Pointer input on the ink canvas --------------------------------

    let pointer_down = {
        let live = live.clone();
        let mut add_object = add_object.clone();
        let mut erase_at = erase_at.clone();
        move |e: PointerEvent| {
            let kind = e.pointer_type();
            let mut l = live.borrow_mut();
            if kind == "pen" {
                l.pen_seen = true;
            } else if kind == "touch" && l.pen_seen {
                return; // palm rejection
            }
            let web = e.as_web_event();
            if let Some(target) = web.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                let _ = target.set_pointer_capture(e.pointer_id());
            }
            web.prevent_default();
            let p = e.element_coordinates();
            let (x, y) = (p.x, p.y);
            let Some(b) = page_box() else { return };
            match tool() {
                ToolKind::Pen | ToolKind::Highlighter => {
                    let pressure =
                        if kind == "mouse" { 0 } else { (e.pressure() * 255.0).round().clamp(1.0, 255.0) as u8 };
                    l.drawing = true;
                    l.pointer = e.pointer_id();
                    l.points = vec![(x, y, pressure)];
                }
                ToolKind::Eraser => {
                    l.erasing = true;
                    l.pointer = e.pointer_id();
                    drop(l);
                    erase_at(x, y);
                }
                ToolKind::Stamp(kind) => {
                    drop(l);
                    let at = b.to_page(x, y);
                    add_object(InkObject::Stamp(Stamp {
                        id: ink::new_id(),
                        kind,
                        at: [at.0, at.1],
                        size: STAMP_SIZE,
                        color: color(),
                    }));
                }
                ToolKind::Text => {
                    drop(l);
                    if let Some(text) = prompt("Text", "") {
                        let at = b.to_page(x, y);
                        add_object(InkObject::Text(TextNote {
                            id: ink::new_id(),
                            at: [at.0, at.1],
                            size: TEXT_SIZE,
                            color: color(),
                            text: text.chars().take(model::MAX_TEXT_CHARS).collect(),
                        }));
                    }
                }
            }
        }
    };

    let pointer_move = {
        let live = live.clone();
        let mut erase_at = erase_at.clone();
        move |e: PointerEvent| {
            let mut l = live.borrow_mut();
            if e.pointer_id() != l.pointer || !(l.drawing || l.erasing) {
                return;
            }
            let p = e.element_coordinates();
            if l.erasing {
                drop(l);
                erase_at(p.x, p.y);
                return;
            }
            let Some(b) = page_box() else { return };
            let kind = e.pointer_type();
            let pressure = if kind == "mouse" { 0 } else { (e.pressure() * 255.0).round().clamp(1.0, 255.0) as u8 };
            let last = *l.points.last().expect("stroke has a first point");
            if (last.0 - p.x).abs() < 0.5 && (last.1 - p.y).abs() < 0.5 {
                return;
            }
            l.points.push((p.x, p.y, pressure));
            if let Some(ctx) = ink::context(INK_CANVAS) {
                ctx.set_line_cap("round");
                if tool() == ToolKind::Highlighter {
                    ctx.set_global_alpha(0.35);
                    ctx.set_stroke_style_str(&ink::css_color(highlight(), 1.0));
                    ink::segment(&ctx, (last.0, last.1), (p.x, p.y), b.len_css(HIGHLIGHT_WIDTH));
                    ctx.set_global_alpha(1.0);
                } else {
                    ctx.set_stroke_style_str(&ink::css_color(color(), 1.0));
                    ink::segment(&ctx, (last.0, last.1), (p.x, p.y), ink::pen_width_css(b, pen_width(), pressure));
                }
            }
        }
    };

    let pointer_up = {
        let live = live.clone();
        let mut add_object = add_object.clone();
        move |e: PointerEvent| {
            let mut l = live.borrow_mut();
            if e.pointer_id() != l.pointer {
                return;
            }
            l.erasing = false;
            if !l.drawing {
                return;
            }
            l.drawing = false;
            let points = std::mem::take(&mut l.points);
            drop(l);
            let Some(b) = page_box() else { return };
            let mut page_points: Vec<(u16, u16, u8)> = Vec::with_capacity(points.len());
            for (x, y, pr) in points.into_iter().take(model::MAX_POINTS_PER_STROKE) {
                let (px, py) = b.to_page(x, y);
                if page_points.last().is_none_or(|q| (q.0, q.1) != (px, py)) {
                    page_points.push((px, py, pr));
                }
            }
            let (stroke_tool, c, w) = match tool() {
                ToolKind::Highlighter => (Tool::Highlighter, highlight(), HIGHLIGHT_WIDTH),
                _ => (Tool::Pen, color(), pen_width()),
            };
            add_object(InkObject::Stroke(Stroke::from_points(ink::new_id(), stroke_tool, c, w, &page_points)));
        }
    };

    // --- Annotation set management ---------------------------------------

    let mut replace_set = move |updated: AnnotationSet| {
        let mut list = sets.write();
        if updated.is_default {
            for s in list.iter_mut() {
                s.is_default = false;
            }
        }
        match list.iter_mut().find(|s| s.id == updated.id) {
            Some(s) => *s = updated,
            None => list.push(updated),
        }
    };
    let part_id = part.id;
    let active_set = active().and_then(|a| sets.read().iter().find(|s| s.id == a).cloned());

    let new_set = move |copy: bool| async move {
        let source = if copy { active() } else { None };
        let suggestion = if copy { "Copy" } else { "Concert" };
        let Some(name) = prompt("Name of the new annotation set", suggestion) else { return };
        let req = NewAnnotationSet { name, programme_id, copy_from: source };
        match api::post::<_, AnnotationSet>(&format!("/api/parts/{part_id}/annotation-sets"), &req).await {
            Ok(set) => {
                let id = set.id;
                replace_set(set);
                active.set(Some(id));
                menu.set(false);
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let update_set = move |body: UpdateAnnotationSet| async move {
        let Some(id) = active() else { return };
        match api::patch::<_, AnnotationSet>(&format!("/api/annotation-sets/{id}"), &body).await {
            Ok(set) => replace_set(set),
            Err(e) => error.set(Some(e.message)),
        }
    };
    let delete_set = move |_| async move {
        let Some(id) = active() else { return };
        if !confirm("Delete this annotation set with all its drawings?") {
            return;
        }
        match api::call(Method::DELETE, &format!("/api/annotation-sets/{id}")).await {
            Ok(()) => {
                sets.write().retain(|s| s.id != id);
                active.set(choose_set(&sets.read(), programme_id));
                menu.set(false);
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let migrate_set = move |_| async move {
        let Some(id) = active() else { return };
        match api::post::<_, AnnotationSet>(&format!("/api/annotation-sets/{id}/migrate"), &serde_json::json!({})).await
        {
            Ok(set) => replace_set(set),
            Err(e) => error.set(Some(e.message)),
        }
    };

    // --- View ------------------------------------------------------------

    let title = format!("{} — {}", data.score.title, part.label);
    let back = match programme_id {
        Some(id) => Route::ProgrammeView { id },
        None => Route::Library {},
    };
    let at_end = page_total() > 0 && page() + 1 >= page_total();
    let status_text = match status() {
        SaveStatus::Idle => String::new(),
        SaveStatus::Pending => "Unsaved".into(),
        SaveStatus::Saving => "Saving…".into(),
        SaveStatus::Saved => "Saved".into(),
        SaveStatus::Failed(e) => format!("Not saved: {e}"),
    };
    let drawing = mode() == Mode::Draw;
    let ink_class = if drawing { "ink active" } else { "ink" };

    rsx! {
        div { class: "reader",
            header { class: "reader-bar",
                Link { class: "icon", to: back, title: "Back", "‹" }
                span { class: "reader-title", title: "{title}", "{title}" }
                span { class: "page-no", if page_total() > 0 { "{page() + 1} / {page_total()}" } }
                button {
                    class: if drawing { "toggle on" } else { "toggle" },
                    title: "Draw (annotations are private)",
                    onclick: move |_| mode.set(if drawing { Mode::Read } else { Mode::Draw }),
                    if drawing { "✓ Done" } else { "✎ Draw" }
                }
                button { class: "icon", title: "Annotation sets", onclick: move |_| menu.toggle(), "☰" }
                button {
                    class: "icon",
                    title: if show_ink() { "Hide annotations" } else { "Show annotations" },
                    onclick: move |_| show_ink.toggle(),
                    if show_ink() { "◉" } else { "○" }
                }
                button { class: "icon", title: "Full screen", onclick: |_| bridge::toggle_fullscreen(), "⛶" }
            }
            if drawing {
                div { class: "tool-bar",
                    ToolButton { label: "Pen", icon: "✒", active: tool() == ToolKind::Pen, onclick: move |_| tool.set(ToolKind::Pen) }
                    ToolButton { label: "Highlighter", icon: "▮", active: tool() == ToolKind::Highlighter, onclick: move |_| tool.set(ToolKind::Highlighter) }
                    ToolButton { label: "Eraser", icon: "⌫", active: tool() == ToolKind::Eraser, onclick: move |_| tool.set(ToolKind::Eraser) }
                    ToolButton { label: "Text", icon: "T", active: tool() == ToolKind::Text, onclick: move |_| tool.set(ToolKind::Text) }
                    select {
                        class: if matches!(tool(), ToolKind::Stamp(_)) { "stamps on" } else { "stamps" },
                        title: "Stamp",
                        onchange: move |e| {
                            if let Ok(i) = e.value().parse::<usize>() {
                                tool.set(ToolKind::Stamp(StampKind::ALL[i]));
                            }
                        },
                        option { value: "", "Stamp…" }
                        for (i, k) in StampKind::ALL.iter().enumerate() {
                            option { value: "{i}", selected: tool() == ToolKind::Stamp(*k), "{k.glyph()}" }
                        }
                    }
                    span { class: "sep" }
                    if tool() == ToolKind::Highlighter {
                        for (c, name) in HIGHLIGHTS {
                            button { class: if highlight() == c { "swatch on" } else { "swatch" }, title: name,
                                style: "background: {ink::css_color(c, 0.6)}", onclick: move |_| highlight.set(c) }
                        }
                    } else {
                        for (c, name) in COLORS {
                            button { class: if color() == c { "swatch on" } else { "swatch" }, title: name,
                                style: "background: {ink::css_color(c, 1.0)}", onclick: move |_| color.set(c) }
                        }
                        span { class: "sep" }
                        for (w, name) in PEN_WIDTHS {
                            button { class: if pen_width() == w { "width on" } else { "width" }, title: name,
                                onclick: move |_| pen_width.set(w),
                                span { class: "dot", style: "width: {w / 30}px; height: {w / 30}px" }
                            }
                        }
                    }
                    span { class: "sep" }
                    button { class: "icon", title: "Undo", onclick: { let mut u = undo_redo.clone(); move |_| u(false) }, "↶" }
                    button { class: "icon", title: "Redo", onclick: { let mut u = undo_redo.clone(); move |_| u(true) }, "↷" }
                    span { class: "save-status", "{status_text}" }
                }
            }
            if menu() {
                div { class: "set-menu card",
                    h3 { "Annotation sets" }
                    if sets.read().is_empty() {
                        p { class: "muted small", "None yet. Drawing creates one." }
                    }
                    for s in sets.read().iter().cloned() {
                        label { key: "{s.id}", class: "set-option",
                            input { r#type: "radio", name: "set", checked: active() == Some(s.id),
                                onchange: move |_| active.set(Some(s.id)) }
                            span { "{s.name}" }
                            if s.is_default { span { class: "tag", "default" } }
                            if s.file_version != current_version { span { class: "tag warn", "v{s.file_version}" } }
                        }
                    }
                    label { class: "set-option",
                        input { r#type: "radio", name: "set", checked: active().is_none(), onchange: move |_| active.set(None) }
                        span { class: "muted", "No annotations" }
                    }
                    div { class: "menu-actions",
                        button { onclick: move |_| new_set(false), "New set" }
                        if let Some(s) = active_set.clone() {
                            button { onclick: move |_| new_set(true), "Duplicate" }
                            button {
                                onclick: move |_| {
                                    let current = s.name.clone();
                                    async move {
                                        if let Some(name) = prompt("New name", &current) {
                                            update_set(UpdateAnnotationSet { name: Some(name), ..Default::default() }).await;
                                        }
                                    }
                                },
                                "Rename"
                            }
                            if !s.is_default {
                                button { onclick: move |_| update_set(UpdateAnnotationSet { is_default: Some(true), ..Default::default() }), "Make default" }
                            }
                            if let Some(pid) = programme_id {
                                if s.programme_id != Some(pid) {
                                    button { onclick: move |_| update_set(UpdateAnnotationSet { programme_id: Some(Some(pid)), ..Default::default() }), "Use for this programme" }
                                }
                            }
                            button { class: "danger", onclick: delete_set, "Delete" }
                        }
                    }
                    button { class: "link close", onclick: move |_| menu.set(false), "Close" }
                }
            }
            if let Some(s) = active_set.clone().filter(|s| s.file_version != current_version) {
                div { class: "banner",
                    "These annotations belong to version {s.file_version} of the sheet; the current version is {current_version}. "
                    button { onclick: migrate_set, "Move them to the current version" }
                }
            }
            if let Some(e) = error() {
                div { class: "banner error", "{e} " button { class: "link", onclick: move |_| error.set(None), "Dismiss" } }
            }
            div {
                class: "stage",
                onresize: move |e| {
                    if let Ok(size) = e.get_content_box_size() {
                        stage.set((size.width, size.height));
                    }
                },
                onclick: {
                    let mut turn = turn.clone();
                    move |e: MouseEvent| {
                        if mode() == Mode::Read {
                            let x = e.client_coordinates().x;
                            let w = stage().0.max(1.0);
                            if x < w / 3.0 {
                                turn(-1);
                            } else if x > w * 2.0 / 3.0 {
                                turn(1);
                            }
                        }
                    }
                },
                div { class: "sheet",
                    canvas { id: PAGE_CANVAS }
                    canvas {
                        id: INK_CANVAS,
                        class: ink_class,
                        onpointerdown: pointer_down,
                        onpointermove: pointer_move,
                        onpointerup: pointer_up.clone(),
                        onpointercancel: pointer_up,
                    }
                }
                if at_end && mode() == Mode::Read {
                    if let Some(r) = next_piece() {
                        Link { class: "next-piece", to: r, onclick: move |e: MouseEvent| e.stop_propagation(), "Next piece ›" }
                    }
                }
            }
        }
    }
}

#[component]
fn ToolButton(label: &'static str, icon: &'static str, active: bool, onclick: EventHandler<MouseEvent>) -> Element {
    rsx! {
        button { class: if active { "tool on" } else { "tool" }, title: label, onclick: move |e| onclick.call(e),
            span { class: "tool-icon", "{icon}" }
            span { class: "tool-label", "{label}" }
        }
    }
}
