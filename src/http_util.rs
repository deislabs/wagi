//! Utilities for working with HTTP requests and responses.

use std::collections::HashMap;
use std::net::SocketAddr;

use hyper::HeaderMap;
use hyper::{header::HOST, http::request::Parts, Body, Response, StatusCode};

use crate::dispatcher::RoutePattern;
use crate::version::*;

/// Create an HTTP 404 response
pub(crate) fn not_found() -> Response<Body> {
    let mut not_found = Response::default();
    *not_found.status_mut() = StatusCode::NOT_FOUND;
    not_found
}

/// Create an HTTP 500 response
pub(crate) fn internal_error(msg: impl std::string::ToString) -> Response<Body> {
    let message = msg.to_string();
    tracing::error!(error = %message, "HTTP 500 error");
    let mut res = Response::new(Body::from(message));
    *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    res
}

pub(crate) fn parse_cgi_headers(headers: String) -> HashMap<String, String> {
    let mut map = HashMap::new();
    headers.trim().split('\n').for_each(|h| {
        let parts: Vec<&str> = h.splitn(2, ':').collect();
        if parts.len() != 2 {
            tracing::warn!(header = h, "corrupt header");
            return;
        }
        map.insert(parts[0].trim().to_owned(), parts[1].trim().to_owned());
    });
    map
}

// TODO: doesn't properly belong here - more about parsing headers into
// WAGI env vars
pub fn build_headers(
    route: &RoutePattern,
    req: &Parts,
    content_length: usize,
    client_addr: SocketAddr,
    default_host: &str,
    use_tls: bool,
    environment: &HashMap<String, String>,
) -> HashMap<String, String> {
    let (host, port) = parse_host_header_uri(&req.headers, &req.uri, default_host);
    let path_info = route.relative_path(req.uri.path());

    // Note that we put these first so that there is no chance that they overwrite
    // the built-in vars. IMPORTANT: This is also why some values have empty strings
    // deliberately set (as opposed to omiting the pair altogether).
    let mut headers = environment.clone();

    // CGI headers from RFC
    headers.insert("AUTH_TYPE".to_owned(), "".to_owned()); // Not currently supported

    // CONTENT_LENGTH (from the spec)
    // The server MUST set this meta-variable if and only if the request is
    // accompanied by a message-body entity.  The CONTENT_LENGTH value must
    // reflect the length of the message-body after the server has removed
    // any transfer-codings or content-codings.
    headers.insert("CONTENT_LENGTH".to_owned(), format!("{}", content_length));

    // CONTENT_TYPE (from the spec)
    // The server MUST set this meta-variable if an HTTP Content-Type field is present
    // in the client request header.  If the server receives a request with an
    // attached entity but no Content-Type header field, it MAY attempt to determine
    // the correct content type, otherwise it should omit this meta-variable.
    //
    // Right now, we don't attempt to determine a media type if none is presented.
    //
    // The spec seems to indicate that if CONTENT_LENGTH > 0, this may be set
    // to "application/octet-stream" if no type is otherwise set. Not sure that is
    // a good idea.
    headers.insert(
        "CONTENT_TYPE".to_owned(),
        req.headers
            .get("CONTENT_TYPE")
            .map(|c| c.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned(),
    );

    let protocol = if use_tls { "https" } else { "http" };

    // Since this is not in the specification, an X_ is prepended, per spec.
    // NB: It is strange that there is not a way to do this already. The Display impl
    // seems to only provide the path.
    let uri = req.uri.clone();
    headers.insert(
        "X_FULL_URL".to_owned(),
        format!(
            "{}://{}:{}{}",
            protocol,
            host,
            port,
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("")
        ),
    );

    headers.insert("GATEWAY_INTERFACE".to_owned(), WAGI_VERSION.to_owned());

    // This is the Wagi route. This is different from PATH_INFO in that it may
    // have a trailing '/...'
    headers.insert("X_MATCHED_ROUTE".to_owned(), route.original_text());

    headers.insert(
        "QUERY_STRING".to_owned(),
        req.uri.query().unwrap_or("").to_owned(),
    );

    headers.insert("REMOTE_ADDR".to_owned(), client_addr.ip().to_string());
    headers.insert("REMOTE_HOST".to_owned(), client_addr.ip().to_string()); // The server MAY substitute it with REMOTE_ADDR
    headers.insert("REMOTE_USER".to_owned(), "".to_owned()); // TODO: Parse this out of uri.authority?
    headers.insert("REQUEST_METHOD".to_owned(), req.method.to_string());

    // The Path component is /$SCRIPT_NAME/$PATH_INFO
    // SCRIPT_NAME is the route that matched.
    // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.13
    let script_name = route.script_name();
    headers.insert("SCRIPT_NAME".to_owned(), script_name);
    // PATH_INFO is any path information after SCRIPT_NAME
    //
    // I am intentionally ignoring the PATH_INFO rule that says that a PATH_INFO
    // cannot have a path seperator in it. If it becomes important to distinguish
    // between what was decoded out of the path and what is encoded in the path,
    // the X_RAW_PATH_INFO can be used.
    //
    // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.5
    let pathsegment = path_info;
    let pathinfo = url_escape::decode(&pathsegment);
    headers.insert("X_RAW_PATH_INFO".to_owned(), pathsegment.clone());
    headers.insert("PATH_INFO".to_owned(), pathinfo.to_string());
    // PATH_TRANSLATED is the url-decoded version of PATH_INFO
    // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.6
    headers.insert("PATH_TRANSLATED".to_owned(), pathinfo.to_string());

    // From the spec: "the server would use the contents of the request's Host header
    // field to select the correct virtual host."
    headers.insert("SERVER_NAME".to_owned(), host);
    headers.insert("SERVER_PORT".to_owned(), port);
    headers.insert("SERVER_PROTOCOL".to_owned(), format!("{:?}", req.version));

    headers.insert(
        "SERVER_SOFTWARE".to_owned(),
        SERVER_SOFTWARE_VERSION.to_owned(),
    );

    // Normalize incoming HTTP headers. The spec says:
    // "The HTTP header field name is converted to upper case, has all
    // occurrences of "-" replaced with "_" and has "HTTP_" prepended to
    // give the meta-variable name."
    req.headers.iter().for_each(|header| {
        let key = format!(
            "HTTP_{}",
            header.0.as_str().to_uppercase().replace("-", "_")
        );
        // Per spec 4.1.18, skip some headers
        if key == "HTTP_AUTHORIZATION" || key == "HTTP_CONNECTION" {
            return;
        }
        let val = header.1.to_str().unwrap_or("CORRUPT VALUE").to_owned();
        headers.insert(key, val);
    });

    headers
}

/// Internal utility function for parsing a host header.
///
/// This attempts to use three sources to construct a definitive host/port pair, ordering
/// by precedent.
///
/// - The content of the host header is considered most authoritative.
/// - Next most authoritative is self.host, which is set at the CLI or in the config
/// - As a last resort, we use the host/port that Hyper gives us.
/// - If none of these provide sufficient data, which is definitely a possiblity,
///   we go with `localhost` as host and `80` as port. This, of course, is problematic,
///   but should only manifest if both the server and the client are behaving badly.
fn parse_host_header_uri(
    headers: &HeaderMap,
    uri: &hyper::Uri,
    default_host: &str,
) -> (String, String) {
    let host_header = headers.get(HOST).and_then(|v| match v.to_str() {
        Err(_) => None,
        Ok(s) => Some(s.to_owned()),
    });

    let mut host = uri
        .host()
        .map(|h| h.to_string())
        .unwrap_or_else(|| "localhost".to_owned());
    let mut port = uri.port_u16().unwrap_or(80).to_string();

    let mut parse_host = |hdr: String| {
        let mut parts = hdr.splitn(2, ':');
        match parts.next() {
            Some(h) if !h.is_empty() => host = h.to_owned(),
            _ => {}
        }
        match parts.next() {
            Some(p) if !p.is_empty() => {
                tracing::debug!(port = p, "Overriding port");
                port = p.to_owned()
            }
            _ => {}
        }
    };

    // Override with local host field if set.
    if !default_host.is_empty() {
        parse_host(default_host.to_owned());
    }

    // Finally, the value of the HOST header is considered authoritative.
    // When it comes to port number, the HOST header isn't necessarily 100% trustworthy.
    // But it appears that this is still the best behavior for the CGI spec.
    if let Some(hdr) = host_header {
        parse_host(hdr);
    }

    (host, port)
}

#[cfg(test)]
mod test {
    use super::*;

    use hyper::http::request::Request;
    use std::str::FromStr;

    #[test]
    fn test_parse_host_header_uri() {
        // let module = Module::new("/base".to_string(), "file:///no/such/path.wasm".to_owned());

        let hmap = |val: &str| {
            let mut hm = hyper::HeaderMap::new();
            hm.insert(
                "HOST",
                hyper::header::HeaderValue::from_str(val).expect("Made a header value"),
            );
            hm
        };

        let default_host = "example.com:1234";

        {
            // All should come from HOST header
            let headers = hmap("wagi.net:31337");
            let uri = hyper::Uri::from_str("http://localhost:443/foo/bar").expect("parsed URI");

            let (host, port) = parse_host_header_uri(&headers, &uri, default_host);
            assert_eq!("wagi.net", host);
            assert_eq!("31337", port);
        }
        {
            // Name should come from HOST, port should come from self.host
            let headers = hmap("wagi.net");
            let uri = hyper::Uri::from_str("http://localhost:443/foo/bar").expect("parsed URI");

            let (host, port) = parse_host_header_uri(&headers, &uri, default_host);
            assert_eq!("wagi.net", host);
            assert_eq!("1234", port)
        }
        {
            // Host and domain should come from default_host
            let headers = hyper::HeaderMap::new();
            let uri = hyper::Uri::from_str("http://localhost:8080/foo/bar").expect("parsed URI");

            let (host, port) = parse_host_header_uri(&headers, &uri, default_host);

            assert_eq!("example.com", host);
            assert_eq!("1234", port)
        }
        {
            // Host and port should come from URI
            let empty_host = "";
            let headers = hyper::HeaderMap::new();
            let uri = hyper::Uri::from_str("http://localhost:8080/foo/bar").expect("parsed URI");

            let (host, port) = parse_host_header_uri(&headers, &uri, empty_host);

            assert_eq!("localhost", host);
            assert_eq!("8080", port)
        }
    }

    #[test]
    fn test_headers() {
        let route = RoutePattern::parse("/path/...");
        // "file:///no/such/path.wasm".to_owned(),
        let (req, _) = Request::builder()
            .uri("https://example.com:3000/path/test%3brun?foo=bar")
            .header("X-Test-Header", "hello")
            .header("Accept", "text/html")
            .header("User-agent", "test")
            .header("Host", "example.com:3000")
            .header("Authorization", "supersecret")
            .header("Connection", "sensitive")
            .method("POST")
            .body(())
            .unwrap()
            .into_parts();
        let content_length = 1234;
        let client_addr = "192.168.0.1:3000".parse().expect("Should parse IP");
        let default_host = "example.com:3000";
        let use_tls = true;
        let env = std::collections::HashMap::with_capacity(0);
        let headers = build_headers(
            &route,
            &req,
            content_length,
            client_addr,
            default_host,
            use_tls,
            &env,
        );

        let want = |key: &str, expect: &str| {
            let v = headers
                .get(&key.to_owned())
                .unwrap_or_else(|| panic!("expected to find key {}", key));

            assert_eq!(expect, v, "Key: {}", key)
        };

        // Content-type is set on output, so we don't test here.
        want("X_MATCHED_ROUTE", "/path/...");
        want("HTTP_ACCEPT", "text/html");
        want("REQUEST_METHOD", "POST");
        want("SERVER_PROTOCOL", "HTTP/1.1");
        want("HTTP_USER_AGENT", "test");
        want("SCRIPT_NAME", "/path");
        want("SERVER_SOFTWARE", "WAGI/1");
        want("SERVER_PORT", "3000");
        want("SERVER_NAME", "example.com");
        want("AUTH_TYPE", "");
        want("REMOTE_ADDR", "192.168.0.1");
        want("REMOTE_ADDR", "192.168.0.1");
        want("PATH_INFO", "/test;run");
        want("PATH_TRANSLATED", "/test;run");
        want("QUERY_STRING", "foo=bar");
        want("CONTENT_LENGTH", "1234");
        want("HTTP_HOST", "example.com:3000");
        want("GATEWAY_INTERFACE", "CGI/1.1");
        want("REMOTE_USER", "");
        want(
            "X_FULL_URL",
            "https://example.com:3000/path/test%3brun?foo=bar",
        );

        // Extra header should be passed through
        want("HTTP_X_TEST_HEADER", "hello");

        // Finally, security-sensitive headers should be removed.
        assert!(headers.get("HTTP_AUTHORIZATION").is_none());
        assert!(headers.get("HTTP_CONNECTION").is_none());
    }
}
