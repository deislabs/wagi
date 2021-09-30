pub(crate) mod bindle_util;
pub mod dispatcher;
pub(crate) mod dynamic_route;
pub mod emplacer;
pub(crate) mod handlers;
pub(crate) mod header_util;
mod http_util;
pub (crate) mod module_loader;
mod request;
mod tls;
pub mod version;
pub mod wagi_app;
pub mod wagi_config;
pub mod wagi_server;
mod wasm_module;
pub(crate) mod wasm_runner;

#[cfg(test)]
mod upstream;

#[cfg(test)]
mod test {
    use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

    use crate::{dispatcher::RoutingTable, wagi_app};

    fn test_data_dir() -> PathBuf {
        let project_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        project_path.join("testdata")
    }

    fn test_standalone_bindle_data_dir() -> PathBuf {
        test_data_dir().join("standalone-bindles")
    }

    fn module_map_path(name: &str) -> PathBuf {
        test_data_dir().join("module-maps").join(name)
    }

    fn mock_client_addr() -> SocketAddr {
        "123.4.5.6:7890".parse().expect("Failed to parse mock client address")
    }

    const DYNAMIC_ROUTES_SA_ID: &str = "dynamic-routes/0.1.0";
    const PRINT_ENV_SA_ID: &str = "print-env/0.1.0";
    const TOAST_ON_DEMAND_SA_ID: &str = "itowlson/toast-on-demand/0.1.0-ivan-2021.09.24.17.06.16.069";
    const TEST1_MODULE_MAP_FILE: &str = "test1.toml";

    async fn build_routing_table_for_standalone_bindle(bindle_id: &str) -> RoutingTable {
        // Clear any env vars that would cause conflicts if set
        std::env::remove_var("BINDLE_URL");

        let matches = wagi_app::wagi_app_definition().get_matches_from(vec![
            "wagi",
            "-b", bindle_id,
            "--bindle-path", &test_standalone_bindle_data_dir().display().to_string(),
        ]);
        let configuration = wagi_app::parse_configuration_from(matches)
            .expect("Fake command line was not valid");

        let emplacer = crate::emplacer::Emplacer::new(&configuration).await
            .expect("Failed to create emplacer");
        emplacer.emplace_all().await
            .expect("Failed to emplace bindle data");
        let handlers = configuration.load_handler_configuration(&emplacer).await
            .expect("Failed to load handlers");
        crate::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())
            .expect("Failed to build routing table")
    }

    // Accepting a Result<Request> here reduces noise in the actual tests
    async fn send_request_to_standalone_bindle(bindle_id: &str, request: hyper::http::Result<hyper::Request<hyper::body::Body>>) -> hyper::Response<hyper::body::Body> {
        let routing_table = build_routing_table_for_standalone_bindle(bindle_id).await;

        routing_table.handle_request(
            request.expect("Failed to construct mock request"),
            mock_client_addr()
        ).await
        .expect("Error producing HTTP response")
    }

    async fn build_routing_table_for_module_map(map_file: &str) -> RoutingTable {
        // Clear any env vars that would cause conflicts if set
        std::env::remove_var("BINDLE_URL");

        let matches = wagi_app::wagi_app_definition().get_matches_from(vec![
            "wagi",
            "-c", &module_map_path(map_file).display().to_string(),
        ]);
        let configuration = wagi_app::parse_configuration_from(matches)
            .expect("Fake command line was not valid");

        let emplacer = crate::emplacer::Emplacer::new(&configuration).await
            .expect("Failed to create emplacer");
        emplacer.emplace_all().await
            .expect("Failed to emplace bindle data");
        let handlers = configuration.load_handler_configuration(&emplacer).await
            .expect("Failed to load handlers");
        crate::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())
            .expect("Failed to build routing table")
    }

    async fn send_request_to_module_map(map_file: &str, request: hyper::http::Result<hyper::Request<hyper::body::Body>>) -> hyper::Response<hyper::body::Body> {
        let routing_table = build_routing_table_for_module_map(map_file).await;

        routing_table.handle_request(
            request.expect("Failed to construct mock request"),
            mock_client_addr()
        ).await
        .expect("Error producing HTTP response")
    }

    async fn get_plain_text_response_from_standalone_bindle(bindle_id: &str, route: &str) -> String {
        let empty_body = hyper::body::Body::empty();
        let uri = format!("http://127.0.0.1:3000{}", route);
        let request = hyper::Request::get(&uri).body(empty_body);

        let response = send_request_to_standalone_bindle(bindle_id, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status());

        // Content-Type could include a charset
        let content_type = response.headers().get("Content-Type").expect("Expected Content-Type header").to_str().unwrap();
        assert!(content_type.starts_with("text/plain"));

        let response_body = hyper::body::to_bytes(response.into_body()).await
            .expect("Could bot get bytes from response body");
        let response_text = std::str::from_utf8(&response_body)
            .expect("Could not read body as string");
        response_text.to_owned()
    }

    async fn get_decription_and_evs_from_standalone_bindle(bindle_id: &str, route: &str) -> (String, HashMap<String, String>) {
        let response_text = get_plain_text_response_from_standalone_bindle(bindle_id, route).await;

        let description = response_text.lines().nth(0).unwrap().to_owned();
        
        let env_vars = response_text
            .lines()
            .skip(1)
            .filter_map(|line| parse_ev_line(line))
            .collect::<HashMap<_, _>>();

        (description, env_vars)
    }

    // TODO: set up more specific and focused WASM apps that allow better testing
    // of routes, headers, EVs, and responses.

    #[tokio::test]
    pub async fn can_serve_from_bindle() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/").body(empty_body);

        let response = send_request_to_standalone_bindle(TOAST_ON_DEMAND_SA_ID, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status());
        assert_eq!("text/html", response.headers().get("Content-Type").expect("Expected Content-Type header"));
    }

    #[tokio::test]
    pub async fn bindle_unmapped_route_returns_not_found() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/does/not/exist").body(empty_body);

        let response = send_request_to_standalone_bindle(TOAST_ON_DEMAND_SA_ID, request).await;

        assert_eq!(hyper::StatusCode::NOT_FOUND, response.status());
    }

    #[tokio::test]
    pub async fn can_serve_from_modules_toml() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/").body(empty_body);

        let response = send_request_to_module_map(TEST1_MODULE_MAP_FILE, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status());
        assert_eq!("text/html", response.headers().get("Content-Type").expect("Expected Content-Type header"));
    }

    #[tokio::test]
    pub async fn modules_toml_unmapped_route_returns_not_found() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/does/not/exist").body(empty_body);

        let response = send_request_to_module_map(TEST1_MODULE_MAP_FILE, request).await;

        assert_eq!(hyper::StatusCode::NOT_FOUND, response.status());
    }

    fn parse_ev_line(line: &str) -> Option<(String, String)> {
        line.find('=').and_then(|index| {
            let left = &line[..index];
            let right = &line[(index + 2)..];
            Some((left.trim().to_owned(), right.trim().to_owned()))
        })
    }

    #[tokio::test]
    pub async fn http_settings_are_mapped_to_env_vars() {
        let response_text = get_plain_text_response_from_standalone_bindle(PRINT_ENV_SA_ID, "/").await;
        let parsed_response = response_text
            .lines()
            .filter_map(|line| parse_ev_line(line))
            .collect::<HashMap<_, _>>();

        assert_eq!("", parsed_response["PATH_INFO"]);
        assert_eq!("", parsed_response["PATH_TRANSLATED"]);
        assert_eq!("/", parsed_response["X_MATCHED_ROUTE"]);
        assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
        assert_eq!("/", parsed_response["SCRIPT_NAME"]);
        assert_eq!("123.4.5.6", parsed_response["REMOTE_ADDR"]);
        assert_eq!("GET", parsed_response["REQUEST_METHOD"]);
    }

    #[tokio::test]
    pub async fn http_settings_are_mapped_to_env_vars_wildcard_route() {
        let response_text = get_plain_text_response_from_standalone_bindle(PRINT_ENV_SA_ID, "/test/fizz/buzz").await;
        let parsed_response = response_text
            .lines()
            .filter_map(|line| parse_ev_line(line))
            .collect::<HashMap<_, _>>();

        assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
        assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
        assert_eq!("/test/...", parsed_response["X_MATCHED_ROUTE"]);
        assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
        assert_eq!("/test", parsed_response["SCRIPT_NAME"]);
        assert_eq!("123.4.5.6", parsed_response["REMOTE_ADDR"]);
        assert_eq!("GET", parsed_response["REQUEST_METHOD"]);
    }

    #[tokio::test]
    pub async fn dynamic_routes_set_path_env_vars_correctly() {
        let bindle_id = DYNAMIC_ROUTES_SA_ID;

        {
            let route = "/";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exactparent";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparent", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparent", parsed_response["SCRIPT_NAME"]);
        }

        {
            // In a _routes scenario, it is not clear how to canonically parse the URL.
            // We choose treat the CGI script name as the *full* routing path, i.e. the
            // *concatenation* of the static parent path and the dynamic _routes-supplied
            // subpath. This is not currently specified by the WAGI spec, though, and earlier
            // implementations of Rust WAGI handled it differently!
            let route = "/exactparent/exact";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparent/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparent/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exactparent/wildcard/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the .../wildcard/... handler", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparent/wildcard/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparent/wildcard", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/exact";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/wildcard/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_standalone_bindle(bindle_id, route).await;
            assert_eq!("This is the .../wildcard/... handler", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/wildcard/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent/wildcard", parsed_response["SCRIPT_NAME"]);
        }
    }
}
