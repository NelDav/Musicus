//! Login and invite (first password / password reset) pages.

use crate::{Route, api, session, ui::ErrorBox};
use dioxus::prelude::*;
use musicus_api::{AcceptInvite, InviteInfo, LoginRequest, LoginResponse};

#[component]
pub fn Login() -> Element {
    let s = session();
    let nav = navigator();
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        if s.user.read().is_some() {
            nav.replace(Route::Library {});
        }
    });
    let submit = move |e: FormEvent| async move {
        e.prevent_default();
        busy.set(true);
        let req = LoginRequest { username: username(), password: password(), bearer: false };
        match api::post::<_, LoginResponse>("/api/auth/login", &req).await {
            Ok(r) => {
                let mut user = s.user;
                user.set(Some(r.user));
            }
            Err(e) if e.status == 401 => error.set(Some("Wrong username or password.".into())),
            Err(e) => error.set(Some(e.message)),
        }
        busy.set(false);
    };
    rsx! {
        main { class: "auth",
            form { class: "card", onsubmit: submit,
                h1 { "𝄞 Musicus" }
                if let Some(e) = error() {
                    ErrorBox { message: e }
                }
                label { "Username"
                    input { r#type: "text", autocomplete: "username", autofocus: true, required: true,
                        value: "{username}", oninput: move |e| username.set(e.value()) }
                }
                label { "Password"
                    input { r#type: "password", autocomplete: "current-password", required: true,
                        value: "{password}", oninput: move |e| password.set(e.value()) }
                }
                button { class: "primary", r#type: "submit", disabled: busy(), "Log in" }
                p { class: "muted small", "No account yet? Ask your librarian for an invite link." }
            }
        }
    }
}

#[component]
pub fn Invite(token: String) -> Element {
    let s = session();
    let nav = navigator();
    let token = use_signal(|| token);
    let info =
        use_resource(move || async move { api::get::<InviteInfo>(&format!("/api/auth/invites/{}", token())).await });
    let mut password = use_signal(String::new);
    let mut repeat = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let submit = move |e: FormEvent| async move {
        e.prevent_default();
        if password() != repeat() {
            error.set(Some("The passwords do not match.".into()));
            return;
        }
        match api::post::<_, LoginResponse>(
            &format!("/api/auth/invites/{}", token()),
            &AcceptInvite { password: password() },
        )
        .await
        {
            Ok(r) => {
                let mut user = s.user;
                user.set(Some(r.user));
                nav.replace(Route::Library {});
            }
            Err(e) => error.set(Some(e.message)),
        }
    };
    rsx! {
        main { class: "auth",
            match &*info.read() {
                None => rsx! { p { "Loading…" } },
                Some(Err(_)) => rsx! {
                    div { class: "card",
                        h1 { "Link expired" }
                        p { "This invite link is invalid or has expired. Ask your librarian for a new one." }
                        Link { to: Route::Login {}, "Go to login" }
                    }
                },
                Some(Ok(i)) => rsx! {
                    form { class: "card", onsubmit: submit,
                        h1 { "Welcome, {i.display_name}" }
                        p { "Choose a password for the username " strong { "{i.username}" } "." }
                        if let Some(e) = error() {
                            ErrorBox { message: e }
                        }
                        label { "New password (at least 8 characters)"
                            input { r#type: "password", autocomplete: "new-password", minlength: 8, required: true,
                                value: "{password}", oninput: move |e| password.set(e.value()) }
                        }
                        label { "Repeat password"
                            input { r#type: "password", autocomplete: "new-password", required: true,
                                value: "{repeat}", oninput: move |e| repeat.set(e.value()) }
                        }
                        button { class: "primary", r#type: "submit", "Set password and log in" }
                    }
                },
            }
        }
    }
}
