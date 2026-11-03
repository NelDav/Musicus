//! Layout and small shared components.

use crate::{Route, api, session};
use dioxus::prelude::*;
use gloo_net::http::Method;

/// Frame of all pages except the reader: navigation bar plus content. Sends
/// visitors without a session to the login page.
#[component]
pub fn Shell() -> Element {
    let s = session();
    let nav = navigator();
    let mut menu_open = use_signal(|| false);
    use_effect(move || {
        if (s.checked)() && s.user.read().is_none() {
            nav.replace(Route::Login {});
        }
    });
    let Some(user) = s.user.read().clone() else {
        return rsx! { div { class: "loading-page", "Loading…" } };
    };
    let logout = move |_| async move {
        let _ = api::call(Method::POST, "/api/auth/logout").await;
        let mut u = s.user;
        u.set(None);
    };
    rsx! {
        header { class: "topbar",
            Link { class: "brand", to: Route::Library {}, "𝄞 Musicus" }
            button {
                class: "menu-toggle",
                "aria-label": "Menu",
                onclick: move |_| menu_open.toggle(),
                "☰"
            }
            nav { class: if menu_open() { "open" } else { "" }, onclick: move |_| menu_open.set(false),
                Link { to: Route::Library {}, "Library" }
                Link { to: Route::Programmes {}, "Programmes" }
                if user.role.can_manage_library() {
                    Link { to: Route::Manage {}, "Manage" }
                }
                Link { to: Route::Account {}, "{user.display_name}" }
                button { class: "link", onclick: logout, "Log out" }
            }
        }
        Outlet::<Route> {}
    }
}

#[component]
pub fn ErrorBox(message: String) -> Element {
    rsx! { div { class: "error", role: "alert", "{message}" } }
}

#[component]
pub fn Loading() -> Element {
    rsx! { p { class: "muted", "Loading…" } }
}

/// Unix seconds as a local date and time.
pub fn format_time(secs: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0));
    String::from(date.to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}

/// `2026-12-24` as a localised date.
pub fn format_date(iso: &str) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&format!("{iso}T12:00:00")));
    if date.get_time().is_nan() {
        return iso.to_owned();
    }
    String::from(date.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}

pub fn format_size(bytes: i64) -> String {
    if bytes >= 1_000_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{} KB", (bytes + 999) / 1000) }
}

pub fn confirm(message: &str) -> bool {
    web_sys::window().and_then(|w| w.confirm_with_message(message).ok()).unwrap_or(false)
}

pub fn prompt(message: &str, default: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.prompt_with_message_and_default(message, default).ok().flatten())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
