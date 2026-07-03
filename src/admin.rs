use async_trait::async_trait;
use http::{header, Response, StatusCode};
use pingora::{apps::http_app::ServeHttp, protocols::http::ServerSession};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone)]
pub struct AdminApp {
    ready: Arc<AtomicBool>,
}

impl AdminApp {
    pub fn new(ready: Arc<AtomicBool>) -> Self {
        Self { ready }
    }
}

#[async_trait]
impl ServeHttp for AdminApp {
    async fn response(&self, session: &mut ServerSession) -> Response<Vec<u8>> {
        let path = session.req_header().uri.path();
        let (status, body) = match path {
            "/healthz" => (StatusCode::OK, "ok\n"),
            "/readyz" if self.ready.load(Ordering::Acquire) => (StatusCode::OK, "ready\n"),
            "/readyz" => (StatusCode::SERVICE_UNAVAILABLE, "not ready\n"),
            _ => (StatusCode::NOT_FOUND, "not found\n"),
        };
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::CONTENT_LENGTH, body.len())
            .body(body.as_bytes().to_vec())
            .expect("static health response is valid")
    }
}
