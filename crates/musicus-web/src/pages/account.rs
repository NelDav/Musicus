//! The user's own account: password and logged-in devices.

use crate::{
    api, session,
    ui::{ErrorBox, Loading, format_time},
};
use dioxus::prelude::*;
use gloo_net::http::Method;
use musicus_api::{ChangePassword, SessionInfo};

#[component]
pub fn Account() -> Element {
    let s = session();
    let mut sessions = use_resource(|| api::get::<Vec<SessionInfo>>("/api/auth/sessions"));
    let mut current = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut message = use_signal(|| None::<Result<String, String>>);
    let change = move |e: FormEvent| async move {
        e.prevent_default();
        let body = ChangePassword { current_password: current(), new_password: new() };
        match api::call_json(Method::POST, "/api/auth/password", &body).await {
            Ok(()) => {
                current.set(String::new());
                new.set(String::new());
                message.set(Some(Ok("Password changed. Your other devices were logged out.".into())));
                sessions.restart();
            }
            Err(e) => message.set(Some(Err(e.message))),
        }
    };
    let user = s.user.read().clone();
    rsx! {
        main { class: "narrow",
            h1 { "Account" }
            if let Some(u) = user {
                p { "{u.display_name} · username " strong { "{u.username}" } " · role {u.role.as_str()}" }
            }
            section { class: "card",
                h2 { "Change password" }
                match message() {
                    Some(Ok(m)) => rsx! { p { class: "ok", "{m}" } },
                    Some(Err(m)) => rsx! { ErrorBox { message: m } },
                    None => rsx! {},
                }
                form { onsubmit: change,
                    label { "Current password"
                        input { r#type: "password", autocomplete: "current-password", required: true,
                            value: "{current}", oninput: move |e| current.set(e.value()) }
                    }
                    label { "New password (at least 8 characters)"
                        input { r#type: "password", autocomplete: "new-password", minlength: 8, required: true,
                            value: "{new}", oninput: move |e| new.set(e.value()) }
                    }
                    button { class: "primary", r#type: "submit", "Change password" }
                }
            }
            section { class: "card",
                h2 { "Logged-in devices" }
                match &*sessions.read() {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.message.clone() } },
                    Some(Ok(list)) => rsx! {
                        ul { class: "plain",
                            for d in list.iter() {
                                li { key: "{d.id}", class: "row",
                                    div {
                                        div { "{d.user_agent.clone().unwrap_or_else(|| \"Unknown device\".into())}" }
                                        div { class: "muted small",
                                            "Last used {format_time(d.last_seen_at)}"
                                            if d.current { " · this device" }
                                        }
                                    }
                                    if !d.current {
                                        button {
                                            onclick: {
                                                let id = d.id;
                                                move |_| async move {
                                                    if api::call(Method::DELETE, &format!("/api/auth/sessions/{id}")).await.is_ok() {
                                                        sessions.restart();
                                                    }
                                                }
                                            },
                                            "Log out"
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
}
