//! Musicus web client: library, concert programmes, the sheet reader with
//! private annotations, and the management pages for librarians and admins.

mod api;
mod bridge;
mod ink;
mod pages;
mod ui;

use dioxus::prelude::*;
use musicus_api::CurrentUser;
use pages::{
    account::Account,
    library::Library,
    login::{Invite, Login},
    manage::{Manage, ManageGroups, ManageProgramme, ManageScore, ManageUsers},
    programmes::{ProgrammeView, Programmes},
    viewer::{ProgrammeViewer, Viewer},
};
use ui::Shell;

const CSS: Asset = asset!("/assets/main.css");

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[route("/login")]
    Login {},
    #[route("/invite/:token")]
    Invite { token: String },
    #[route("/parts/:id")]
    Viewer { id: i64 },
    #[route("/programmes/:pid/parts/:id")]
    ProgrammeViewer { pid: i64, id: i64 },
    #[layout(Shell)]
        #[route("/")]
        Library {},
        #[route("/programmes")]
        Programmes {},
        #[route("/programmes/:id")]
        ProgrammeView { id: i64 },
        #[route("/account")]
        Account {},
        #[route("/manage")]
        Manage {},
        #[route("/manage/scores/:id")]
        ManageScore { id: i64 },
        #[route("/manage/programmes/:id")]
        ManageProgramme { id: i64 },
        #[route("/manage/groups")]
        ManageGroups {},
        #[route("/manage/users")]
        ManageUsers {},
    #[end_layout]
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

/// Who is logged in. `checked` is false until `/api/auth/me` answered.
#[derive(Clone, Copy)]
pub struct Session {
    pub user: Signal<Option<CurrentUser>>,
    pub checked: Signal<bool>,
}

pub fn session() -> Session {
    use_context::<Session>()
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut user = use_signal(|| None::<CurrentUser>);
    let mut checked = use_signal(|| false);
    use_context_provider(|| Session { user, checked });
    use_future(move || async move {
        if let Ok(me) = api::get::<CurrentUser>("/api/auth/me").await {
            user.set(Some(me));
        }
        checked.set(true);
    });
    rsx! {
        document::Title { "Musicus" }
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1, viewport-fit=cover" }
        document::Meta { name: "theme-color", content: "#1d2433" }
        document::Stylesheet { href: CSS }
        Router::<Route> {}
    }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        main { class: "narrow",
            h1 { "Page not found" }
            p { "/{segments.join(\"/\")} does not exist." }
            Link { to: Route::Library {}, "Back to the library" }
        }
    }
}
