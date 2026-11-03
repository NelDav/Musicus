//! Management pages for librarians (library, programmes, instruments,
//! grants) and admins (users, groups).

use crate::{
    Route, api, bridge, session,
    ui::{ErrorBox, Loading, confirm, format_date, format_size, format_time},
};
use dioxus::prelude::*;
use gloo_net::http::Method;
use musicus_api::{
    CreateUser, CreatedUser, Group, GroupInput, IdList, Instrument, InstrumentInput, Invite, Part, PartInput,
    Programme, ProgrammeInput, ProgrammeSummary, Role, Score, ScoreInput, UpdateUser, UserSummary,
};

fn none_if_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_owned()) }
}

fn tags_from(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

#[component]
fn ManageNav() -> Element {
    let is_admin = session().user.read().as_ref().is_some_and(|u| u.role.is_admin());
    rsx! {
        nav { class: "subnav",
            Link { to: Route::Manage {}, "Library & programmes" }
            Link { to: Route::ManageGroups {}, "Groups & access" }
            if is_admin {
                Link { to: Route::ManageUsers {}, "Users" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Library, programmes and instruments
// ---------------------------------------------------------------------------

#[component]
pub fn Manage() -> Element {
    let nav = navigator();
    let mut scores = use_resource(|| api::get::<Vec<Score>>("/api/scores"));
    let mut programmes = use_resource(|| api::get::<Vec<ProgrammeSummary>>("/api/programmes"));
    let mut error = use_signal(|| None::<String>);
    let mut title = use_signal(String::new);
    let mut composer = use_signal(String::new);
    let mut prog_title = use_signal(String::new);
    let mut prog_date = use_signal(String::new);
    let mut filter = use_signal(String::new);

    let create_score = move |e: FormEvent| async move {
        e.prevent_default();
        let input = ScoreInput { title: title(), composer: none_if_empty(composer()), ..Default::default() };
        match api::post::<_, Score>("/api/scores", &input).await {
            Ok(s) => {
                scores.restart();
                nav.push(Route::ManageScore { id: s.id });
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let create_programme = move |e: FormEvent| async move {
        e.prevent_default();
        let input = ProgrammeInput { title: prog_title(), date: none_if_empty(prog_date()), notes: None };
        match api::post::<_, Programme>("/api/programmes", &input).await {
            Ok(p) => {
                programmes.restart();
                nav.push(Route::ManageProgramme { id: p.id });
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let needle = filter().to_lowercase();
    rsx! {
        main {
            ManageNav {}
            if let Some(e) = error() { ErrorBox { message: e } }
            div { class: "columns",
                section { class: "card",
                    h2 { "Scores" }
                    form { class: "inline", onsubmit: create_score,
                        input { placeholder: "Title", required: true, value: "{title}", oninput: move |e| title.set(e.value()) }
                        input { placeholder: "Composer", value: "{composer}", oninput: move |e| composer.set(e.value()) }
                        button { class: "primary", r#type: "submit", "Add score" }
                    }
                    input { class: "search", r#type: "search", placeholder: "Filter…", value: "{filter}",
                        oninput: move |e| filter.set(e.value()) }
                    match &*scores.read() {
                        None => rsx! { Loading {} },
                        Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                        Some(Ok(list)) => rsx! {
                            ul { class: "plain",
                                for s in list.iter().filter(|s| needle.is_empty() || s.title.to_lowercase().contains(&needle)
                                    || s.composer.as_deref().unwrap_or("").to_lowercase().contains(&needle)) {
                                    li { key: "{s.id}", class: "row",
                                        Link { to: Route::ManageScore { id: s.id }, "{s.title}" }
                                        span { class: "muted small",
                                            "{s.parts.len()} parts"
                                            if s.parts.iter().any(|p| p.file.is_none()) { " · files missing" }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
                div {
                    section { class: "card",
                        h2 { "Programmes" }
                        form { class: "inline", onsubmit: create_programme,
                            input { placeholder: "Title", required: true, value: "{prog_title}", oninput: move |e| prog_title.set(e.value()) }
                            input { r#type: "date", value: "{prog_date}", oninput: move |e| prog_date.set(e.value()) }
                            button { class: "primary", r#type: "submit", "Add programme" }
                        }
                        match &*programmes.read() {
                            None => rsx! { Loading {} },
                            Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                            Some(Ok(list)) => rsx! {
                                ul { class: "plain",
                                    for p in list.iter() {
                                        li { key: "{p.id}", class: "row",
                                            Link { to: Route::ManageProgramme { id: p.id }, "{p.title}" }
                                            span { class: "muted small",
                                                if let Some(d) = &p.date { "{format_date(d)} · " }
                                                "{p.item_count} pieces"
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                    Instruments {}
                }
            }
        }
    }
}

#[component]
fn Instruments() -> Element {
    let mut list = use_resource(|| api::get::<Vec<Instrument>>("/api/instruments"));
    let mut name = use_signal(String::new);
    let mut family = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let add = move |e: FormEvent| async move {
        e.prevent_default();
        let input = InstrumentInput { name: name(), family: none_if_empty(family()) };
        match api::post::<_, Instrument>("/api/instruments", &input).await {
            Ok(_) => {
                name.set(String::new());
                list.restart();
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    rsx! {
        section { class: "card",
            h2 { "Instruments" }
            p { class: "muted small", "Parts are tagged with instruments; groups get access by instrument." }
            if let Some(e) = error() { ErrorBox { message: e } }
            form { class: "inline", onsubmit: add,
                input { placeholder: "Instrument, e.g. Trumpet", required: true, value: "{name}", oninput: move |e| name.set(e.value()) }
                input { placeholder: "Family, e.g. Brass", value: "{family}", oninput: move |e| family.set(e.value()) }
                button { r#type: "submit", "Add" }
            }
            match &*list.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(items)) => rsx! {
                    ul { class: "chips",
                        for i in items.iter() {
                            li { key: "{i.id}",
                                "{i.name}"
                                if let Some(f) = &i.family { span { class: "muted small", " ({f})" } }
                                button {
                                    class: "link",
                                    title: "Delete",
                                    onclick: {
                                        let id = i.id;
                                        move |_| async move {
                                            match api::call(Method::DELETE, &format!("/api/instruments/{id}")).await {
                                                Ok(()) => list.restart(),
                                                Err(e) => error.set(Some(e.message)),
                                            }
                                        }
                                    },
                                    "×"
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

#[component]
pub fn ManageScore(id: i64) -> Element {
    let nav = navigator();
    let mut score = use_resource(move || async move { api::get::<Score>(&format!("/api/scores/{id}")).await });
    let instruments = use_resource(|| api::get::<Vec<Instrument>>("/api/instruments"));
    let mut error = use_signal(|| None::<String>);
    let mut saved = use_signal(|| false);
    let mut form = use_signal(ScoreInput::default);
    let mut tags = use_signal(String::new);
    let mut new_label = use_signal(String::new);

    // Fill the form once the score arrived.
    use_effect(move || {
        if let Some(Ok(s)) = &*score.read() {
            form.set(ScoreInput {
                title: s.title.clone(),
                subtitle: s.subtitle.clone(),
                composer: s.composer.clone(),
                arranger: s.arranger.clone(),
                genre: s.genre.clone(),
                tags: s.tags.clone(),
                notes: s.notes.clone(),
            });
            tags.set(s.tags.join(", "));
        }
    });

    let save = move |e: FormEvent| async move {
        e.prevent_default();
        let mut input = form();
        input.tags = tags_from(&tags());
        for field in [&mut input.subtitle, &mut input.composer, &mut input.arranger, &mut input.genre, &mut input.notes]
        {
            *field = field.take().and_then(none_if_empty);
        }
        match api::put::<_, Score>(&format!("/api/scores/{id}"), &input).await {
            Ok(_) => {
                saved.set(true);
                error.set(None);
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let delete = move |_| async move {
        if !confirm("Delete this score with all parts? Everyone's annotations on it are deleted too.") {
            return;
        }
        match api::call(Method::DELETE, &format!("/api/scores/{id}")).await {
            Ok(()) => {
                nav.push(Route::Manage {});
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let add_part = move |e: FormEvent| async move {
        e.prevent_default();
        let input = PartInput { label: new_label(), ..Default::default() };
        match api::post::<_, Part>(&format!("/api/scores/{id}/parts"), &input).await {
            Ok(_) => {
                new_label.set(String::new());
                score.restart();
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let all_instruments = match &*instruments.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let f = form();
    rsx! {
        main {
            ManageNav {}
            Link { to: Route::Manage {}, "‹ All scores" }
            if let Some(e) = error() { ErrorBox { message: e } }
            match &*score.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(s)) => rsx! {
                    section { class: "card",
                        h1 { "{s.title}" }
                        form { class: "grid", onsubmit: save, oninput: move |_| saved.set(false),
                            label { "Title" input { required: true, value: "{f.title}",
                                oninput: move |e| form.write().title = e.value() } }
                            label { "Subtitle" input { value: "{f.subtitle.clone().unwrap_or_default()}",
                                oninput: move |e| form.write().subtitle = Some(e.value()) } }
                            label { "Composer" input { value: "{f.composer.clone().unwrap_or_default()}",
                                oninput: move |e| form.write().composer = Some(e.value()) } }
                            label { "Arranger" input { value: "{f.arranger.clone().unwrap_or_default()}",
                                oninput: move |e| form.write().arranger = Some(e.value()) } }
                            label { "Genre" input { placeholder: "March, polka, concert…", value: "{f.genre.clone().unwrap_or_default()}",
                                oninput: move |e| form.write().genre = Some(e.value()) } }
                            label { "Tags (comma separated)" input { value: "{tags}", oninput: move |e| tags.set(e.value()) } }
                            label { class: "wide", "Notes" textarea { rows: 3, value: "{f.notes.clone().unwrap_or_default()}",
                                oninput: move |e| form.write().notes = Some(e.value()) } }
                            div { class: "wide actions",
                                button { class: "primary", r#type: "submit", "Save" }
                                if saved() { span { class: "ok", "Saved" } }
                                button { class: "danger", r#type: "button", onclick: delete, "Delete score" }
                            }
                        }
                    }
                    section { class: "card",
                        h2 { "Parts" }
                        for p in s.parts.iter() {
                            PartEditor { key: "{p.id}", part: p.clone(), instruments: all_instruments.clone(),
                                onchange: move |_| score.restart() }
                        }
                        form { class: "inline", onsubmit: add_part,
                            input { placeholder: "New part, e.g. 2nd Trumpet in Bb", required: true, value: "{new_label}",
                                oninput: move |e| new_label.set(e.value()) }
                            button { r#type: "submit", "Add part" }
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn PartEditor(part: Part, instruments: Vec<Instrument>, onchange: EventHandler<()>) -> Element {
    let id = part.id;
    let mut label = use_signal(|| part.label.clone());
    let mut voice = use_signal(|| part.voice.map(|v| v.to_string()).unwrap_or_default());
    let mut transposition = use_signal(|| part.transposition.clone().unwrap_or_default());
    let mut selected = use_signal(|| part.instrument_ids.clone());
    let mut status = use_signal(|| None::<Result<String, String>>);
    let mut uploading = use_signal(|| false);

    let save = move |_| async move {
        let input = PartInput {
            label: label(),
            instrument_ids: selected(),
            voice: voice().trim().parse().ok(),
            transposition: none_if_empty(transposition()),
            notes: None,
        };
        match api::put::<_, Part>(&format!("/api/parts/{id}"), &input).await {
            Ok(_) => {
                status.set(Some(Ok("Saved".into())));
                onchange.call(());
            }
            Err(e) => status.set(Some(Err(e.message))),
        }
    };
    let delete = move |_| async move {
        if !confirm("Delete this part? Everyone's annotations on it are deleted too.") {
            return;
        }
        match api::call(Method::DELETE, &format!("/api/parts/{id}")).await {
            Ok(()) => onchange.call(()),
            Err(e) => status.set(Some(Err(e.message))),
        }
    };
    let upload = move |e: FormEvent| async move {
        let Some(file) = e.files().into_iter().next() else { return };
        uploading.set(true);
        status.set(Some(Ok(format!("Uploading {}…", file.name()))));
        let result = match file.read_bytes().await {
            Ok(bytes) => api::upload::<Part>(&format!("/api/parts/{id}/file"), &bytes, "application/pdf")
                .await
                .map_err(|e| e.message),
            Err(e) => Err(format!("Could not read the file: {e}")),
        };
        uploading.set(false);
        match result {
            Ok(_) => {
                status.set(Some(Ok("Uploaded".into())));
                onchange.call(());
            }
            Err(e) => status.set(Some(Err(e))),
        }
    };
    rsx! {
        div { class: "part-editor",
            div { class: "part-fields",
                input { class: "grow", value: "{label}", oninput: move |e| label.set(e.value()), aria_label: "Label" }
                input { class: "short", placeholder: "Voice", value: "{voice}", oninput: move |e| voice.set(e.value()), aria_label: "Voice" }
                input { class: "short", placeholder: "Key (Bb)", value: "{transposition}", oninput: move |e| transposition.set(e.value()), aria_label: "Transposition" }
            }
            div { class: "checks",
                if instruments.is_empty() { span { class: "muted small", "Add instruments first." } }
                for i in instruments.iter() {
                    label { key: "{i.id}", class: "check",
                        input {
                            r#type: "checkbox",
                            checked: selected.read().contains(&i.id),
                            onchange: {
                                let iid = i.id;
                                move |e: FormEvent| {
                                    let mut s = selected.write();
                                    s.retain(|x| *x != iid);
                                    if e.checked() {
                                        s.push(iid);
                                    }
                                }
                            },
                        }
                        "{i.name}"
                    }
                }
            }
            div { class: "part-file",
                match &part.file {
                    Some(f) => rsx! {
                        span { class: "muted small",
                            "Version {f.version} · {format_size(f.size_bytes)}"
                            if let Some(n) = f.page_count { " · {n} pages" }
                            " · uploaded {format_time(f.uploaded_at)} "
                        }
                        Link { to: Route::Viewer { id }, "Open" }
                    },
                    None => rsx! { span { class: "warn-text small", "No file yet" } },
                }
                label { class: "button", class: if uploading() { "disabled" },
                    if part.file.is_some() { "Replace PDF…" } else { "Upload PDF…" }
                    input { r#type: "file", accept: "application/pdf,.pdf", hidden: true, disabled: uploading(), onchange: upload }
                }
            }
            div { class: "actions",
                button { onclick: save, "Save part" }
                button { class: "danger", onclick: delete, "Delete" }
                match status() {
                    Some(Ok(m)) => rsx! { span { class: "ok", "{m}" } },
                    Some(Err(m)) => rsx! { span { class: "error-text", "{m}" } },
                    None => rsx! {},
                }
            }
        }
    }
}

#[component]
pub fn ManageProgramme(id: i64) -> Element {
    let nav = navigator();
    let programme = use_resource(move || async move { api::get::<Programme>(&format!("/api/programmes/{id}")).await });
    let scores = use_resource(|| api::get::<Vec<Score>>("/api/scores"));
    let mut title = use_signal(String::new);
    let mut date = use_signal(String::new);
    let mut notes = use_signal(String::new);
    let mut items = use_signal(Vec::<(i64, String)>::new);
    let mut pick = use_signal(String::new);
    let mut message = use_signal(|| None::<Result<String, String>>);

    use_effect(move || {
        if let Some(Ok(p)) = &*programme.read() {
            title.set(p.title.clone());
            date.set(p.date.clone().unwrap_or_default());
            notes.set(p.notes.clone().unwrap_or_default());
            items.set(p.items.iter().map(|i| (i.score.id, i.score.title.clone())).collect());
        }
    });
    let save = move |e: FormEvent| async move {
        e.prevent_default();
        let input = ProgrammeInput { title: title(), date: none_if_empty(date()), notes: none_if_empty(notes()) };
        let result = async {
            api::put::<_, Programme>(&format!("/api/programmes/{id}"), &input).await?;
            let ids = IdList { ids: items().iter().map(|i| i.0).collect() };
            api::put::<_, Programme>(&format!("/api/programmes/{id}/items"), &ids).await
        }
        .await;
        message.set(Some(result.map(|_| "Saved".into()).map_err(|e| e.message)));
    };
    let delete = move |_| async move {
        if confirm("Delete this programme? The scores stay in the library.")
            && api::call(Method::DELETE, &format!("/api/programmes/{id}")).await.is_ok()
        {
            nav.push(Route::Manage {});
        }
    };
    let all_scores = match &*scores.read() {
        Some(Ok(list)) => list.clone(),
        _ => Vec::new(),
    };
    let count = items.read().len();
    rsx! {
        main {
            ManageNav {}
            Link { to: Route::Manage {}, "‹ Library & programmes" }
            match &*programme.read() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                Some(Ok(_)) => rsx! {
                    form { class: "card", onsubmit: save,
                        h1 { "Programme" }
                        div { class: "grid",
                            label { "Title" input { required: true, value: "{title}", oninput: move |e| title.set(e.value()) } }
                            label { "Date" input { r#type: "date", value: "{date}", oninput: move |e| date.set(e.value()) } }
                            label { class: "wide", "Notes" textarea { rows: 2, value: "{notes}", oninput: move |e| notes.set(e.value()) } }
                        }
                        h2 { "Pieces in concert order" }
                        ol { class: "items",
                            for (index, (score_id, name)) in items.read().iter().cloned().enumerate() {
                                li { key: "{index}-{score_id}",
                                    span { class: "grow", "{name}" }
                                    button { r#type: "button", disabled: index == 0, title: "Move up",
                                        onclick: move |_| items.write().swap(index, index - 1), "↑" }
                                    button { r#type: "button", disabled: index + 1 == count, title: "Move down",
                                        onclick: move |_| items.write().swap(index, index + 1), "↓" }
                                    button { r#type: "button", class: "danger", title: "Remove",
                                        onclick: move |_| { items.write().remove(index); }, "×" }
                                }
                            }
                        }
                        div { class: "inline",
                            select { value: "{pick}", onchange: move |e| pick.set(e.value()),
                                option { value: "", "Add a piece…" }
                                for s in all_scores.iter() {
                                    option { key: "{s.id}", value: "{s.id}", "{s.title}" }
                                }
                            }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    if let Ok(sid) = pick().parse::<i64>()
                                        && let Some(s) = all_scores.iter().find(|s| s.id == sid)
                                    {
                                        items.write().push((s.id, s.title.clone()));
                                    }
                                },
                                "Add"
                            }
                        }
                        div { class: "actions",
                            button { class: "primary", r#type: "submit", "Save programme" }
                            match message() {
                                Some(Ok(m)) => rsx! { span { class: "ok", "{m}" } },
                                Some(Err(m)) => rsx! { span { class: "error-text", "{m}" } },
                                None => rsx! {},
                            }
                            button { class: "danger", r#type: "button", onclick: delete, "Delete programme" }
                        }
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Groups and grants
// ---------------------------------------------------------------------------

#[component]
pub fn ManageGroups() -> Element {
    let is_admin = session().user.read().as_ref().is_some_and(|u| u.role.is_admin());
    let mut groups = use_resource(|| api::get::<Vec<Group>>("/api/groups"));
    let instruments = use_resource(|| api::get::<Vec<Instrument>>("/api/instruments"));
    let scores = use_resource(|| api::get::<Vec<Score>>("/api/scores"));
    let users = use_resource(move || async move {
        if is_admin { api::get::<Vec<UserSummary>>("/api/users").await } else { Ok(Vec::new()) }
    });
    let mut name = use_signal(String::new);
    let mut selected = use_signal(|| None::<i64>);
    let mut error = use_signal(|| None::<String>);

    let create = move |e: FormEvent| async move {
        e.prevent_default();
        match api::post::<_, Group>("/api/groups", &GroupInput { name: name() }).await {
            Ok(g) => {
                name.set(String::new());
                selected.set(Some(g.id));
                groups.restart();
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    let list = match &*groups.read() {
        Some(Ok(l)) => l.clone(),
        Some(Err(e)) => return rsx! { main { ManageNav {} ErrorBox { message: e.message.clone() } } },
        None => return rsx! { main { ManageNav {} Loading {} } },
    };
    let current = selected().and_then(|id| list.iter().find(|g| g.id == id).cloned());
    rsx! {
        main {
            ManageNav {}
            if let Some(e) = error() { ErrorBox { message: e } }
            div { class: "columns narrow-left",
                section { class: "card",
                    h2 { "Groups" }
                    ul { class: "plain selectable",
                        for g in list.iter() {
                            li { key: "{g.id}", class: if selected() == Some(g.id) { "row on" } else { "row" },
                                onclick: { let id = g.id; move |_| selected.set(Some(id)) },
                                span { "{g.name}" }
                                span { class: "muted small", "{g.member_ids.len()} members" }
                            }
                        }
                    }
                    if is_admin {
                        form { class: "inline", onsubmit: create,
                            input { placeholder: "New group, e.g. Trumpets", required: true, value: "{name}", oninput: move |e| name.set(e.value()) }
                            button { r#type: "submit", "Add" }
                        }
                    }
                }
                match current {
                    None => rsx! { section { class: "card muted", "Select a group to edit its members and access." } },
                    Some(g) => rsx! {
                        GroupEditor {
                            key: "{g.id}",
                            group: g,
                            is_admin,
                            users: match &*users.read() { Some(Ok(u)) => u.clone(), _ => Vec::new() },
                            instruments: match &*instruments.read() { Some(Ok(i)) => i.clone(), _ => Vec::new() },
                            scores: match &*scores.read() { Some(Ok(s)) => s.clone(), _ => Vec::new() },
                            onchange: move |_| groups.restart(),
                            ondelete: move |_| { selected.set(None); groups.restart(); },
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn GroupEditor(
    group: Group,
    is_admin: bool,
    users: Vec<UserSummary>,
    instruments: Vec<Instrument>,
    scores: Vec<Score>,
    onchange: EventHandler<()>,
    ondelete: EventHandler<()>,
) -> Element {
    let id = group.id;
    let members = use_signal(|| group.member_ids.clone());
    let granted_instruments = use_signal(|| group.instrument_ids.clone());
    let granted_scores = use_signal(|| group.score_ids.clone());
    let mut score_filter = use_signal(String::new);
    let mut message = use_signal(|| None::<Result<String, String>>);

    let save = move |_| async move {
        let result = async {
            if is_admin {
                api::put::<_, Group>(&format!("/api/groups/{id}/members"), &IdList { ids: members() }).await?;
            }
            api::put::<_, Group>(
                &format!("/api/groups/{id}/instrument-grants"),
                &IdList { ids: granted_instruments() },
            )
            .await?;
            api::put::<_, Group>(&format!("/api/groups/{id}/score-grants"), &IdList { ids: granted_scores() }).await
        }
        .await;
        message.set(Some(result.map(|_| "Saved".into()).map_err(|e| e.message)));
        onchange.call(());
    };
    let delete = move |_| async move {
        if confirm("Delete this group? Its members lose the access it granted.") {
            match api::call(Method::DELETE, &format!("/api/groups/{id}")).await {
                Ok(()) => ondelete.call(()),
                Err(e) => message.set(Some(Err(e.message))),
            }
        }
    };
    let toggle = |mut list: Signal<Vec<i64>>, item: i64| {
        move |e: FormEvent| {
            let mut l = list.write();
            l.retain(|x| *x != item);
            if e.checked() {
                l.push(item);
            }
        }
    };
    let needle = score_filter().to_lowercase();
    rsx! {
        section { class: "card",
            h2 { "{group.name}" }
            p { class: "muted small",
                "Members can read every part tagged with a granted instrument, plus every part of a granted score."
            }
            if is_admin {
                h3 { "Members" }
                div { class: "checks",
                    for u in users.iter() {
                        label { key: "{u.id}", class: "check",
                            input { r#type: "checkbox", checked: members.read().contains(&u.id), onchange: toggle(members, u.id) }
                            "{u.display_name}"
                        }
                    }
                }
            }
            h3 { "Instruments" }
            div { class: "checks",
                for i in instruments.iter() {
                    label { key: "{i.id}", class: "check",
                        input { r#type: "checkbox", checked: granted_instruments.read().contains(&i.id), onchange: toggle(granted_instruments, i.id) }
                        "{i.name}"
                    }
                }
            }
            h3 { "Whole scores (e.g. for the conductor)" }
            input { class: "search", r#type: "search", placeholder: "Filter scores…", value: "{score_filter}",
                oninput: move |e| score_filter.set(e.value()) }
            div { class: "checks scroll",
                for s in scores.iter().filter(|s| needle.is_empty() || s.title.to_lowercase().contains(&needle) || granted_scores.read().contains(&s.id)) {
                    label { key: "{s.id}", class: "check",
                        input { r#type: "checkbox", checked: granted_scores.read().contains(&s.id), onchange: toggle(granted_scores, s.id) }
                        "{s.title}"
                    }
                }
            }
            div { class: "actions",
                button { class: "primary", onclick: save, "Save" }
                match message() {
                    Some(Ok(m)) => rsx! { span { class: "ok", "{m}" } },
                    Some(Err(m)) => rsx! { span { class: "error-text", "{m}" } },
                    None => rsx! {},
                }
                if is_admin {
                    button { class: "danger", onclick: delete, "Delete group" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Users (admin)
// ---------------------------------------------------------------------------

fn invite_link(invite: &Invite) -> String {
    let origin = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
    format!("{origin}/invite/{}", invite.token)
}

#[component]
pub fn ManageUsers() -> Element {
    let mut users = use_resource(|| api::get::<Vec<UserSummary>>("/api/users"));
    let mut username = use_signal(String::new);
    let mut display = use_signal(String::new);
    let mut role = use_signal(|| Role::Member);
    let mut link = use_signal(|| None::<(String, String)>);
    let mut error = use_signal(|| None::<String>);

    let create = move |e: FormEvent| async move {
        e.prevent_default();
        let input = CreateUser { username: username(), display_name: display(), role: role() };
        match api::post::<_, CreatedUser>("/api/users", &input).await {
            Ok(c) => {
                link.set(Some((c.user.display_name.clone(), invite_link(&c.invite))));
                username.set(String::new());
                display.set(String::new());
                users.restart();
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    rsx! {
        main {
            ManageNav {}
            if let Some(e) = error() { ErrorBox { message: e } }
            if let Some((name, url)) = link() {
                div { class: "card highlight",
                    p { "Send this one-time link to " strong { "{name}" } ". It is valid for 7 days and lets them choose a password." }
                    div { class: "inline",
                        input { class: "grow", readonly: true, value: "{url}" }
                        button { onclick: move |_| { let url = url.clone(); async move { bridge::copy_text(&url).await; } }, "Copy" }
                    }
                }
            }
            section { class: "card",
                h2 { "New user" }
                form { class: "inline", onsubmit: create,
                    input { placeholder: "Username", required: true, value: "{username}", oninput: move |e| username.set(e.value()) }
                    input { placeholder: "Display name", required: true, value: "{display}", oninput: move |e| display.set(e.value()) }
                    select { onchange: move |e| role.set(Role::parse(&e.value()).unwrap_or(Role::Member)),
                        option { value: "member", selected: role() == Role::Member, "Member" }
                        option { value: "librarian", selected: role() == Role::Librarian, "Librarian" }
                        option { value: "admin", selected: role() == Role::Admin, "Admin" }
                    }
                    button { class: "primary", r#type: "submit", "Create and get invite link" }
                }
            }
            section { class: "card",
                h2 { "Users" }
                match &*users.read() {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                    Some(Ok(list)) => rsx! {
                        table { class: "table",
                            thead { tr { th { "Name" } th { "Username" } th { "Role" } th { "Status" } th {} } }
                            tbody {
                                for u in list.iter().cloned() {
                                    UserRow { key: "{u.id}", user: u,
                                        onchange: move |_| users.restart(),
                                        oninvite: move |l: (String, String)| link.set(Some(l)),
                                        onerror: move |m: String| error.set(Some(m)) }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn UserRow(
    user: UserSummary,
    onchange: EventHandler<()>,
    oninvite: EventHandler<(String, String)>,
    onerror: EventHandler<String>,
) -> Element {
    let id = user.id;
    let name = user.display_name.clone();
    let update = move |body: UpdateUser| async move {
        match api::patch::<_, UserSummary>(&format!("/api/users/{id}"), &body).await {
            Ok(_) => onchange.call(()),
            Err(e) => onerror.call(e.message),
        }
    };
    let invite = move |_| {
        let name = name.clone();
        async move {
            match api::post::<_, Invite>(&format!("/api/users/{id}/invite"), &serde_json::json!({})).await {
                Ok(i) => oninvite.call((name, invite_link(&i))),
                Err(e) => onerror.call(e.message),
            }
        }
    };
    let disabled = user.disabled;
    rsx! {
        tr { class: if disabled { "disabled" },
            td { "{user.display_name}" }
            td { "{user.username}" }
            td {
                select {
                    onchange: move |e| update(UpdateUser { role: Role::parse(&e.value()), ..Default::default() }),
                    option { value: "member", selected: user.role == Role::Member, "Member" }
                    option { value: "librarian", selected: user.role == Role::Librarian, "Librarian" }
                    option { value: "admin", selected: user.role == Role::Admin, "Admin" }
                }
            }
            td {
                if disabled { "Disabled" } else if user.has_password { "Active" } else { span { class: "muted", "Invited" } }
            }
            td { class: "actions",
                if !disabled {
                    button { onclick: invite, if user.has_password { "Reset link" } else { "New invite link" } }
                }
                button {
                    onclick: move |_| update(UpdateUser { disabled: Some(!disabled), ..Default::default() }),
                    if disabled { "Enable" } else { "Disable" }
                }
            }
        }
    }
}
