pub(crate) mod bindle_util;
pub mod dispatcher;
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

#[cfg(test)]
mod upstream;

#[cfg(test)]
mod test {
    use std::{net::SocketAddr, path::PathBuf};

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
}
