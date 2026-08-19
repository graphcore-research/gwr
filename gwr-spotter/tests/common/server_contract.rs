// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;

use crate::TEST_SERVER_STATE;

pub(crate) struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

pub(crate) trait HttpClient {
    async fn get(&self, path: &str) -> HttpResponse;
}

fn assert_cors(response: &HttpResponse) {
    assert_eq!(response.header("access-control-allow-origin"), Some("*"));
    assert_eq!(
        response.header("access-control-allow-methods"),
        Some("GET, HEAD")
    );
    assert_eq!(response.header("access-control-allow-headers"), Some("*"));
    assert_eq!(
        response.header("access-control-allow-credentials"),
        Some("true")
    );
}

fn assert_success(response: &HttpResponse, expected_body: &str) {
    assert_eq!(response.status, 200);
    assert_eq!(response.body, expected_body);
    assert_eq!(
        response.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_cors(response);
}

pub(crate) async fn assert_read_routes(client: &impl HttpClient) {
    {
        let mut state = TEST_SERVER_STATE.lock().unwrap();
        state.reset();
        state.entity_names = vec!["0:root".to_string(), "1:child".to_string()];
        state.capacities = vec!["1:8".to_string(), "2:16".to_string()];
        state.fullnesses = vec!["1:3".to_string(), "2:4".to_string()];
        state.connections = vec!["0->1".to_string(), "1->2".to_string()];
        state.current_line = 12;
        state.num_lines = 34;
        state.current_time_ns = 56.7;
    }

    for (path, expected_body) in [
        ("/entities", "0:root\n1:child"),
        ("/capacities", "1:8\n2:16"),
        ("/fullnesses", "1:3\n2:4"),
        ("/connections", "0->1\n1->2"),
        ("/selected", "none"),
        ("/position", "line=12\nlines=34\ntime=56.7"),
    ] {
        let response = client.get(path).await;
        assert_success(&response, expected_body);
    }

    TEST_SERVER_STATE.lock().unwrap().selected = Some(7);
    let response = client.get("/selected").await;
    assert_success(&response, "7 selected");
}

pub(crate) async fn assert_command_routes(client: &impl HttpClient) {
    TEST_SERVER_STATE.lock().unwrap().reset();

    let response = client.get("/select/42").await;
    assert_success(&response, "42 selected");
    assert_eq!(
        TEST_SERVER_STATE.lock().unwrap().command.as_deref(),
        Some("id=42")
    );

    let response = client.get("/seek/17").await;
    assert_success(&response, "seek 17");
    assert_eq!(TEST_SERVER_STATE.lock().unwrap().seek_line, Some(17));
}

pub(crate) async fn assert_error_routes(client: &impl HttpClient) {
    {
        let mut state = TEST_SERVER_STATE.lock().unwrap();
        state.reset();
        state.command = Some("keep command".to_string());
        state.seek_line = Some(9);
    }

    for path in ["/select/not-a-number", "/seek/not-a-number"] {
        let response = client.get(path).await;
        assert_eq!(response.status, 422);
        assert_cors(&response);
    }

    let response = client.get("/missing").await;
    assert_eq!(response.status, 404);
    assert_cors(&response);

    let state = TEST_SERVER_STATE.lock().unwrap();
    assert_eq!(state.command.as_deref(), Some("keep command"));
    assert_eq!(state.seek_line, Some(9));
}
