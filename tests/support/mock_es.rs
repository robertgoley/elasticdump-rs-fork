//! Sequential script mock (the original `MockEs` API), now a thin wrapper over
//! [`super::router_mock::RouterMock`] (TST-INF-002).
//!
//! Canned responses are served in arrival order through one catch-all route. Every reply
//! carries `Connection: close`, as before, and any request beyond the script gets a 500.
//! The binary sends requests strictly in sequence, so the order of the script is preserved.

use super::router_mock::{Reply, Route, RouterMock};

/// One scripted HTTP reply: an HTTP status code and a JSON body.
#[derive(Clone)]
pub struct CannedResponse {
    pub status: u16,
    pub body: String,
}

impl CannedResponse {
    /// A canned reply with an explicit status and JSON body.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// A `200 OK` reply with the given JSON body.
    pub fn ok(body: impl Into<String>) -> Self {
        Self::new(200, body)
    }
}

/// Handle to a running mock server. Dropping it stops the server.
pub struct MockEs {
    inner: RouterMock,
}

impl MockEs {
    /// Bind an ephemeral `127.0.0.1` port and start serving `script` in request order.
    pub async fn serve(script: Vec<CannedResponse>) -> MockEs {
        let route = Route::any()
            .replies(script.into_iter().map(|c| Reply::json(c.status, c.body)))
            .once();
        let inner = RouterMock::builder()
            .route(route)
            .fallback(Reply::json(
                500,
                r#"{"error":"mock-es: request beyond scripted responses"}"#,
            ))
            .close_always()
            .start()
            .await;
        MockEs { inner }
    }

    /// Number of HTTP requests received so far.
    pub fn request_count(&self) -> usize {
        self.inner.request_count()
    }

    /// Request line, headers and body of every request received so far.
    #[allow(dead_code)]
    pub fn requests(&self) -> Vec<String> {
        self.inner.requests().iter().map(|r| r.raw_head()).collect()
    }

    /// `http://127.0.0.1:PORT/INDEX` input URL for the binary's `--input`.
    pub fn input_url(&self, index: &str) -> String {
        format!("{}/{}", self.inner.url(), index)
    }
}
