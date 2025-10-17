// Copyright (c) 2025 Graphcore Ltd. All rights reserved.
use std::sync::Mutex;

use gwr_track::Id;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::Header;
use rocket::request::FromParam;
use rocket::{Request, Response, get, launch, routes};

pub struct SharedState {
    pub entity_names: Vec<String>,
    pub connections: Vec<String>,
    pub command: Option<String>,
    pub selected: Option<u64>,
}

impl SharedState {
    const fn new() -> Self {
        Self {
            entity_names: Vec::new(),
            connections: Vec::new(),
            command: None,
            selected: None,
        }
    }
}

pub static SHARED_STATE: Mutex<SharedState> = Mutex::new(SharedState::new());

struct RocketId(Id);

/// Error raised when failing to create a ID by parsing a string
///
/// Need to create a local type wrapping the ID in order to implement the
/// FromStr trait
#[derive(Debug, PartialEq, Eq)]
pub struct ParseIdError;

/// Implementation that ensures the ID passed to `select()` is valid
impl<'a> FromParam<'a> for RocketId {
    type Error = &'a str;

    fn from_param(param: &'a str) -> Result<Self, Self::Error> {
        param
            .chars()
            .all(|c| c.is_numeric())
            .then(|| RocketId(param.into()))
            .ok_or(param)
    }
}

/// The server must enable CORS in order to allow access from a browwer on a
/// different local port (see <https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/CORS>).
pub struct CORS;

#[rocket::async_trait]
impl Fairing for CORS {
    fn info(&self) -> Info {
        Info {
            name: "Add CORS headers to responses",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _request: &'r Request<'_>, response: &mut Response<'r>) {
        response.set_header(Header::new("Access-Control-Allow-Origin", "*"));
        response.set_header(Header::new(
            "Access-Control-Allow-Methods",
            "POST, GET, PATCH, OPTIONS",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

#[get("/entities")]
fn entities() -> String {
    SHARED_STATE.lock().unwrap().entity_names.join("\n")
}

#[get("/connections")]
fn connections() -> String {
    SHARED_STATE.lock().unwrap().connections.join("\n")
}

#[get("/select/<id>")]
async fn select(id: RocketId) -> String {
    let mut guard = SHARED_STATE.lock().unwrap();
    guard.command = Some(format!("id={}", id.0).to_string());
    format!("{} selected", id.0)
}

#[get("/selected")]
async fn selected() -> String {
    match SHARED_STATE.lock().unwrap().selected {
        Some(id) => format!("{id} selected"),
        None => "none".to_string(),
    }
}

#[launch]
#[must_use]
pub fn rocket() -> _ {
    rocket::build()
        .attach(CORS)
        .mount("/", routes![entities, connections, select, selected])
}
