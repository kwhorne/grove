//! Small shared helpers for shaping HTTP responses.

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::{Response, StatusCode};

/// Unified, fully-streaming response body used across the tunnel.
pub type Body = BoxBody<Bytes, hyper::Error>;

/// Build a plain-text response (for errors and status pages).
pub fn text(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(body.to_string()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("static response is always valid")
}
