// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::sync::Mutex;

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Router, middleware};
use gwr_track::Id;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Bind and spawn the local HTTP API used by the visualisation frontend.
pub async fn spawn() -> std::io::Result<JoinHandle<()>> {
    spawn_on("127.0.0.1:8000").await
}

async fn spawn_on(address: &str) -> std::io::Result<JoinHandle<()>> {
    let listener = TcpListener::bind(address).await?;
    Ok(tokio::spawn(async move {
        axum::serve(listener, router())
            .await
            .expect("Axum server should run until the Tokio runtime shuts down");
    }))
}

fn router() -> Router {
    Router::new()
        .route("/entities", get(entities))
        .route("/capacities", get(capacities))
        .route("/fullnesses", get(fullnesses))
        .route("/connections", get(connections))
        .route("/select/{id}", get(select))
        .route("/selected", get(selected))
        .route("/position", get(position))
        .route("/seek/{line}", get(seek))
        .layer(middleware::map_response(add_cors_headers))
}

pub(crate) struct SharedState {
    pub(crate) entity_names: Vec<String>,
    pub(crate) capacities: Vec<String>,
    pub(crate) fullnesses: Vec<String>,
    pub(crate) connections: Vec<String>,
    pub(crate) command: Option<String>,
    pub(crate) selected: Option<u64>,
    pub(crate) current_line: usize,
    pub(crate) num_lines: usize,
    pub(crate) current_time_ns: f64,
    pub(crate) seek_line: Option<usize>,
}

impl SharedState {
    const fn new() -> Self {
        Self {
            entity_names: Vec::new(),
            capacities: Vec::new(),
            fullnesses: Vec::new(),
            connections: Vec::new(),
            command: None,
            selected: None,
            current_line: 0,
            num_lines: 0,
            current_time_ns: 0.0,
            seek_line: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        self.entity_names.clear();
        self.capacities.clear();
        self.fullnesses.clear();
        self.connections.clear();
        self.command = None;
        self.selected = None;
        self.current_line = 0;
        self.num_lines = 0;
        self.current_time_ns = 0.0;
        self.seek_line = None;
    }
}

pub(crate) static SHARED_STATE: Mutex<SharedState> = Mutex::new(SharedState::new());

async fn entities() -> String {
    SHARED_STATE.lock().unwrap().entity_names.join("\n")
}

async fn capacities() -> String {
    SHARED_STATE.lock().unwrap().capacities.join("\n")
}

async fn fullnesses() -> String {
    SHARED_STATE.lock().unwrap().fullnesses.join("\n")
}

async fn connections() -> String {
    SHARED_STATE.lock().unwrap().connections.join("\n")
}

async fn select(Path(id): Path<String>) -> Result<String, StatusCode> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let id = id
        .parse::<u64>()
        .map(Id)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    SHARED_STATE.lock().unwrap().command = Some(format!("id={id}"));
    Ok(format!("{id} selected"))
}

async fn selected() -> String {
    match SHARED_STATE.lock().unwrap().selected {
        Some(id) => format!("{id} selected"),
        None => "none".to_string(),
    }
}

async fn position() -> String {
    let state = SHARED_STATE.lock().unwrap();
    format!(
        "line={}\nlines={}\ntime={:.1}",
        state.current_line, state.num_lines, state.current_time_ns
    )
}

async fn seek(Path(line): Path<String>) -> Result<String, StatusCode> {
    let line = line
        .parse::<usize>()
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    SHARED_STATE.lock().unwrap().seek_line = Some(line);
    Ok(format!("seek {line}"))
}

async fn add_cors_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    response
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serial_test::serial;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use super::{router, spawn_on};
    use crate::server_contract::{
        HttpClient, HttpResponse, assert_command_routes, assert_error_routes, assert_read_routes,
    };

    struct AxumClient(Router);

    impl HttpClient for AxumClient {
        async fn get(&self, path: &str) -> HttpResponse {
            let response = self
                .0
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_string(),
                        value.to_str().unwrap().to_string(),
                    )
                })
                .collect();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();

            HttpResponse {
                status,
                headers,
                body,
            }
        }
    }

    fn client() -> AxumClient {
        AxumClient(router())
    }

    #[tokio::test]
    async fn bind_errors_are_returned() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();

        let Err(error) = spawn_on(&address).await else {
            panic!("server unexpectedly bound to an occupied address");
        };

        assert_eq!(error.kind(), ErrorKind::AddrInUse);
    }

    #[tokio::test]
    #[serial]
    async fn read_routes_follow_server_contract() {
        assert_read_routes(&client()).await;
    }

    #[tokio::test]
    #[serial]
    async fn command_routes_follow_server_contract() {
        assert_command_routes(&client()).await;
    }

    #[tokio::test]
    #[serial]
    async fn error_routes_follow_server_contract() {
        assert_error_routes(&client()).await;
    }
}
