//! The library: every score the user has parts for, with search.

use crate::{
    Route, api,
    ui::{ErrorBox, Loading},
};
use dioxus::prelude::*;
use musicus_api::{Instrument, Part, Score};
use std::collections::HashMap;

/// Builds the query string for `/api/scores`.
fn scores_path(q: &str) -> String {
    let q = q.trim();
    if q.is_empty() { "/api/scores".into() } else { format!("/api/scores?q={}", js_sys::encode_uri_component(q)) }
}

#[component]
pub fn Library() -> Element {
    let mut query = use_signal(String::new);
    let scores = use_resource(move || {
        let path = scores_path(&query());
        async move {
            // Let typing settle before searching.
            gloo_timers::future::TimeoutFuture::new(150).await;
            api::get::<Vec<Score>>(&path).await
        }
    });
    let instruments = use_resource(|| api::get::<Vec<Instrument>>("/api/instruments"));
    let names: HashMap<i64, String> = match &*instruments.read() {
        Some(Ok(list)) => list.iter().map(|i| (i.id, i.name.clone())).collect(),
        _ => HashMap::new(),
    };
    rsx! {
        main {
            div { class: "page-head",
                h1 { "Library" }
                input { class: "search", r#type: "search", placeholder: "Search title, composer, tag…",
                    value: "{query}", oninput: move |e| query.set(e.value()) }
            }
            match &*scores.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(list)) if list.is_empty() => rsx! {
                    p { class: "muted",
                        if query().trim().is_empty() { "No sheets have been shared with you yet." } else { "Nothing found." }
                    }
                },
                Some(Ok(list)) => rsx! {
                    div { class: "score-list",
                        for score in list.iter() {
                            ScoreCard { key: "{score.id}", score: score.clone(), names: names.clone(), programme: None }
                        }
                    }
                },
            }
        }
    }
}

pub fn score_byline(score: &Score) -> String {
    let mut parts = Vec::new();
    if let Some(c) = &score.composer {
        parts.push(c.clone());
    }
    if let Some(a) = &score.arranger {
        parts.push(format!("arr. {a}"));
    }
    parts.join(" · ")
}

pub fn part_route(part: &Part, programme: Option<i64>) -> Route {
    match programme {
        Some(pid) => Route::ProgrammeViewer { pid, id: part.id },
        None => Route::Viewer { id: part.id },
    }
}

#[component]
pub fn ScoreCard(score: Score, names: HashMap<i64, String>, programme: Option<i64>) -> Element {
    let byline = score_byline(&score);
    rsx! {
        article { class: "score",
            div { class: "score-title",
                h2 { "{score.title}" }
                if let Some(sub) = &score.subtitle {
                    span { class: "muted", "{sub}" }
                }
                if !byline.is_empty() {
                    p { class: "muted small", "{byline}" }
                }
                div { class: "tags",
                    if let Some(g) = &score.genre {
                        span { class: "tag genre", "{g}" }
                    }
                    for t in score.tags.iter() {
                        span { class: "tag", "{t}" }
                    }
                }
            }
            div { class: "parts",
                for part in score.parts.iter() {
                    PartTile { key: "{part.id}", part: part.clone(), names: names.clone(), programme }
                }
            }
        }
    }
}

#[component]
fn PartTile(part: Part, names: HashMap<i64, String>, programme: Option<i64>) -> Element {
    let instruments: Vec<String> = part.instrument_ids.iter().filter_map(|i| names.get(i).cloned()).collect();
    let Some(file) = part.file.clone() else {
        return rsx! {
            div { class: "part disabled", title: "No file uploaded yet",
                div { class: "thumb empty", "—" }
                span { "{part.label}" }
            }
        };
    };
    rsx! {
        Link { class: "part", to: part_route(&part, programme),
            if file.has_thumbnail {
                img { class: "thumb", src: api::thumbnail_url(&file.sha256), alt: "", loading: "lazy" }
            } else {
                div { class: "thumb empty", "PDF" }
            }
            span { class: "part-label", "{part.label}" }
            if !instruments.is_empty() {
                span { class: "muted small", "{instruments.join(\", \")}" }
            }
        }
    }
}
