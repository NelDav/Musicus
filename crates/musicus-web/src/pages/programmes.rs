//! Concert programmes. Opening one downloads all its sheets into memory, so
//! turning to the next piece on stage needs no network.

use crate::{
    Route, api, bridge,
    pages::library::ScoreCard,
    ui::{ErrorBox, Loading, format_date},
};
use dioxus::prelude::*;
use musicus_api::{Instrument, Programme, ProgrammeSummary};
use std::collections::HashMap;

#[component]
pub fn Programmes() -> Element {
    let list = use_resource(|| api::get::<Vec<ProgrammeSummary>>("/api/programmes"));
    rsx! {
        main {
            div { class: "page-head", h1 { "Programmes" } }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(items)) if items.is_empty() => rsx! { p { class: "muted", "No programmes yet." } },
                Some(Ok(items)) => rsx! {
                    ul { class: "programme-list",
                        for p in items.iter() {
                            li { key: "{p.id}",
                                Link { to: Route::ProgrammeView { id: p.id },
                                    strong { "{p.title}" }
                                    span { class: "muted",
                                        if let Some(d) = &p.date { "{format_date(d)} · " }
                                        "{p.item_count} pieces"
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Sheet urls of a programme in concert order.
pub fn programme_urls(p: &Programme) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for item in &p.items {
        for part in &item.score.parts {
            if let Some(f) = &part.file {
                let url = api::file_url(&f.sha256);
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
    }
    urls
}

#[component]
pub fn ProgrammeView(id: i64) -> Element {
    let programme = use_resource(move || async move { api::get::<Programme>(&format!("/api/programmes/{id}")).await });
    let instruments = use_resource(|| api::get::<Vec<Instrument>>("/api/instruments"));
    let mut loaded = use_signal(|| 0usize);
    let mut failed = use_signal(|| 0usize);
    let mut total = use_signal(|| 0usize);

    // Download every sheet of the programme into memory, a few at a time.
    use_effect(move || {
        let Some(Ok(p)) = &*programme.read() else { return };
        let urls = programme_urls(p);
        // Free the memory of sheets from other programmes.
        bridge::retain_only(&urls);
        total.set(urls.len());
        loaded.set(0);
        failed.set(0);
        spawn(async move {
            for chunk in urls.chunks(3) {
                let results = futures_join(chunk.iter().map(|u| bridge::prefetch(u))).await;
                for r in results {
                    if r.is_ok() {
                        loaded += 1;
                    } else {
                        failed += 1;
                    }
                }
            }
        });
    });

    let names: HashMap<i64, String> = match &*instruments.read() {
        Some(Ok(list)) => list.iter().map(|i| (i.id, i.name.clone())).collect(),
        _ => HashMap::new(),
    };
    rsx! {
        main {
            match &*programme.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(p)) => rsx! {
                    div { class: "page-head",
                        div {
                            h1 { "{p.title}" }
                            if let Some(d) = &p.date { p { class: "muted", "{format_date(d)}" } }
                        }
                        div { class: "prefetch",
                            if total() == 0 {
                                span { class: "muted", "No sheets to download." }
                            } else if loaded() == total() {
                                span { class: "ok", "✓ All {total()} sheets ready offline in this tab" }
                            } else {
                                progress { max: "{total()}", value: "{loaded()}" }
                                span { class: "muted small", " Downloading {loaded()} / {total()}" }
                            }
                            if failed() > 0 {
                                span { class: "error-text", " {failed()} failed" }
                            }
                        }
                    }
                    if let Some(n) = &p.notes { p { class: "notes", "{n}" } }
                    div { class: "score-list",
                        for item in p.items.iter() {
                            div { key: "{item.position}", class: "programme-item",
                                span { class: "position", "{item.position + 1}" }
                                ScoreCard { score: item.score.clone(), names: names.clone(), programme: Some(p.id) }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// Awaits futures whose work already runs (bridge downloads start eagerly),
/// so awaiting them in order still downloads in parallel.
async fn futures_join<F: std::future::Future>(futures: impl Iterator<Item = F>) -> Vec<F::Output> {
    let handles: Vec<F> = futures.collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await);
    }
    out
}
