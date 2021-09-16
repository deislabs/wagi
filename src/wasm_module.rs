use std::sync::Arc;

use hyper::HeaderMap;
use hyper::{
    header::HOST,
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    Body, Request, Response, StatusCode,
};
use tracing::{debug, instrument};

use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

#[derive(Clone)]
pub enum WasmModuleSource {
    Blob(Arc<Vec<u8>>),
}
