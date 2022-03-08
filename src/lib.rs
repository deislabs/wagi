pub(crate) mod bindle_util;
pub mod dispatcher;
pub(crate) mod dynamic_route;
pub mod handler_loader;
pub mod handlers;
pub mod http_util;
mod request;
mod tls;
pub mod version;
pub mod wagi_app;
pub mod wagi_config;
pub mod wagi_server;
pub mod wasm_module;
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

    // This ugliness is because file: URLs in modules.toml are meant to be absolute, but
    // we don't know where the project (and therefore the test WASM modules) will reside
    // on any given machine. So we need to sub in the path where the WASM file will
    // actually be found.
    async fn replace_placeholders(original_map_file: &str, custom_subs: Option<HashMap<String, String>>) -> PathBuf {
        let orig_content = tokio::fs::read(module_map_path(original_map_file)).await
            .expect(&format!("Error reading test file {}", original_map_file));
        let toml_text = std::str::from_utf8(&orig_content)
            .expect(&format!("Error treating test file {} as text", original_map_file));

        let project_root = env!("CARGO_MANIFEST_DIR");
        let timestamp = chrono::Local::now()
            .format("%Y.%m.%d.%H.%M.%S.%3f")
            .to_string();
        let tempfile_dir = PathBuf::from(project_root)
            .join("tests_working_dir")
            .join(timestamp);
        tokio::fs::create_dir_all(&tempfile_dir).await
            .expect("Error creating temp directory");
        let tempfile_path = tempfile_dir.join(original_map_file);

        let mut final_text = toml_text.replace("${PROJECT_ROOT}", &project_root.escape_default().to_string());
        for (k, v) in custom_subs.unwrap_or_default() {
            let pattern = format!("${{{}}}", k);
            final_text = final_text.replace(&pattern, &v.escape_default().to_string());
        }
        tokio::fs::write(&tempfile_path, final_text).await
            .expect("Error saving modified modules file to test working dir");
        tempfile_path
    }

    const DYNAMIC_ROUTES_SA_ID: &str = "dynamic-routes/0.1.0";
    const HTTP_TEST_ID: &str = "http-test/0.2.0";
    const PRINT_ENV_SA_ID: &str = "print-env/0.1.0";
    const TOAST_ON_DEMAND_SA_ID: &str = "itowlson/toast-on-demand/0.1.0-ivan-20210924170616069";
    const TEST1_MODULE_MAP_FILE: &str = "test1.toml";
    #[cfg(target_os = "windows")]
    const TEST2_MODULE_MAP_FILE: &str = "test2.toml";
    const TEST3_MODULE_MAP_FILE: &str = "test3.toml";
    const TEST_HEALTHZ_MODULE_MAP_FILE: &str = "test_healthz_override.toml";
    const TEST_DYNAMIC_ROUTES_MODULE_MAP_FILE: &str = "test_dynamic_routes.toml";

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
        let handlers = crate::handler_loader::load_handlers(&configuration).await
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

    async fn build_routing_table_for_module_map(map_file: &str, custom_subs: Option<HashMap<String, String>>) -> RoutingTable {
        // Clear any env vars that would cause conflicts if set
        std::env::remove_var("BINDLE_URL");

        let modules_toml_path = replace_placeholders(&map_file, custom_subs).await;
        let matches = wagi_app::wagi_app_definition().get_matches_from(vec![
            "wagi",
            "-c", &modules_toml_path.display().to_string(),
        ]);

        let configuration = wagi_app::parse_configuration_from(matches)
            .expect("Fake command line was not valid");
        let handlers = crate::handler_loader::load_handlers(&configuration).await
            .expect("Failed to load handlers");
        crate::dispatcher::RoutingTable::build(&handlers, configuration.request_global_context())
            .expect("Failed to build routing table")
    }

    async fn send_request_to_module_map(map_file: &str, custom_subs: Option<HashMap<String, String>>, request: hyper::http::Result<hyper::Request<hyper::body::Body>>) -> hyper::Response<hyper::body::Body> {
        let routing_table = build_routing_table_for_module_map(map_file, custom_subs).await;

        routing_table.handle_request(
            request.expect("Failed to construct mock request"),
            mock_client_addr()
        ).await
        .expect("Error producing HTTP response")
    }

    async fn get_plain_text_response_from_module_map(map_file: &str, custom_subs: Option<HashMap<String, String>>, route: &str) -> String {
        let empty_body = hyper::body::Body::empty();
        let uri = format!("http://127.0.0.1:3000{}", route);
        let request = hyper::Request::get(&uri).body(empty_body);

        let response = send_request_to_module_map(map_file, custom_subs, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status(), "Non-OK status getting route {}", route);

        // Content-Type could include a charset
        let content_type = response.headers().get("Content-Type").expect("Expected Content-Type header").to_str().unwrap();
        assert!(content_type.starts_with("text/plain"));

        let response_body = hyper::body::to_bytes(response.into_body()).await
            .expect("Could bot get bytes from response body");
        let response_text = std::str::from_utf8(&response_body)
            .expect("Could not read body as string");
        response_text.to_owned()
    }

    async fn get_plain_text_response_from_standalone_bindle(bindle_id: &str, route: &str) -> String {
        let empty_body = hyper::body::Body::empty();
        let uri = format!("http://127.0.0.1:3000{}", route);
        let request = hyper::Request::get(&uri).body(empty_body);

        let response = send_request_to_standalone_bindle(bindle_id, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status(), "Non-OK status getting route {}", route);

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

    async fn get_decription_and_evs_from_module_map(map_file: &str, custom_subs: Option<HashMap<String, String>>, route: &str) -> (String, HashMap<String, String>) {
        let response_text = get_plain_text_response_from_module_map(map_file, custom_subs, route).await;

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
    pub async fn bindle_wildcard_routes_match_only_at_slash() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/blahahahaha").body(empty_body);

        let response = send_request_to_standalone_bindle(TOAST_ON_DEMAND_SA_ID, request).await;

        assert_eq!(hyper::StatusCode::NOT_FOUND, response.status());
    }

    #[tokio::test]
    pub async fn can_serve_from_modules_toml() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/").body(empty_body);

        let response = send_request_to_module_map(TEST1_MODULE_MAP_FILE, None, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status());
        assert_eq!("text/html", response.headers().get("Content-Type").expect("Expected Content-Type header"));
    }

    #[tokio::test]
    pub async fn modules_toml_unmapped_route_returns_not_found() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/does/not/exist").body(empty_body);

        let response = send_request_to_module_map(TEST1_MODULE_MAP_FILE, None, request).await;

        assert_eq!(hyper::StatusCode::NOT_FOUND, response.status());
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    pub async fn can_serve_from_modules_toml_even_if_file_ref_has_all_sorts_of_windows_slashes() {
        use std::iter::FromIterator;

        let wasm_module_path = module_map_path("toast-on-demand.wasm").display().to_string();

        for testcase in possible_slashes_for_paths(wasm_module_path) {
            let subs = HashMap::from_iter(vec![
                ("SLASHY_URL".to_owned(), testcase)
            ]);

            let empty_body = hyper::body::Body::empty();
            let request = hyper::Request::get("http://127.0.0.1:3000/").body(empty_body);

            let response = send_request_to_module_map(TEST2_MODULE_MAP_FILE, Some(subs), request).await;

            assert_eq!(hyper::StatusCode::OK, response.status());
            assert_eq!("text/html", response.headers().get("Content-Type").expect("Expected Content-Type header"));
        }
    }

    #[cfg(target_os = "windows")]
    fn possible_slashes_for_paths(path: String) -> Vec<String> {
        let mut res = vec![];

        // this should transform the initial Windows path coming from
        // the temoporary file to most common ways to define a module
        // in modules.toml.

        res.push(format!("file:{}", path));
        res.push(format!("file:/{}", path));
        res.push(format!("file://{}", path));
        res.push(format!("file:///{}", path));

        let double_backslash = str::replace(path.as_str(), "\\", "\\\\");
        res.push(format!("file:{}", double_backslash));
        res.push(format!("file:/{}", double_backslash));
        res.push(format!("file://{}", double_backslash));
        res.push(format!("file:///{}", double_backslash));

        let forward_slash = str::replace(path.as_str(), "\\", "/");
        res.push(format!("file:{}", forward_slash));
        res.push(format!("file:/{}", forward_slash));
        res.push(format!("file://{}", forward_slash));
        res.push(format!("file:///{}", forward_slash));

        let double_slash = str::replace(path.as_str(), "\\", "//");
        res.push(format!("file:{}", double_slash));
        res.push(format!("file:/{}", double_slash));
        res.push(format!("file://{}", double_slash));
        res.push(format!("file:///{}", double_slash));

        res
    }

    #[tokio::test]
    pub async fn can_serve_multiple_static_entrypoints() {
        {
            let route = "/defaultep";

            let response = get_plain_text_response_from_module_map(TEST3_MODULE_MAP_FILE, None, route).await;
            assert_eq!("Default entrypoint\n", response);
        }

        {
            let route = "/ep1";

            let response = get_plain_text_response_from_module_map(TEST3_MODULE_MAP_FILE, None, route).await;
            assert_eq!("Entrypoint 1\n", response);
        }

        {
            let route = "/ep2";

            let response = get_plain_text_response_from_module_map(TEST3_MODULE_MAP_FILE, None, route).await;
            assert_eq!("Entrypoint 2\n", response);
        }
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
    pub async fn dynamic_routes_set_path_env_vars_correctly_bindle() {
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

    #[tokio::test]
    pub async fn dynamic_routes_set_path_env_vars_correctly_module_map() {
        let map_file = TEST_DYNAMIC_ROUTES_MODULE_MAP_FILE;

        {
            let route = "/";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exact";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exactparent";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
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

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparent/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparent/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exactparent/wildcard/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../wildcard/... handler", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparent/wildcard/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparent/wildcard", parsed_response["SCRIPT_NAME"]);
        }

        {
            // In a _routes scenario, it is not clear how to canonically parse the URL.
            // We choose treat the CGI script name as the *full* routing path, i.e. the
            // *concatenation* of the static parent path and the dynamic _routes-supplied
            // subpath. This is not currently specified by the WAGI spec, though, and earlier
            // implementations of Rust WAGI handled it differently!
            let route = "/exactparentslash/exact";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparentslash/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparentslash/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/exactparentslash/wildcard/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../wildcard/... handler", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/exactparentslash/wildcard/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/exactparentslash/wildcard", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the main entry point", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/exact";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../exact handler", description);
            
            assert_eq!("", parsed_response["PATH_INFO"]);
            assert_eq!("", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/exact", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent/exact", parsed_response["SCRIPT_NAME"]);
        }

        {
            let route = "/wildcardparent/wildcard/fizz/buzz";

            let (description, parsed_response) = get_decription_and_evs_from_module_map(map_file, None, route).await;
            assert_eq!("This is the .../wildcard/... handler", description);
            
            assert_eq!("/fizz/buzz", parsed_response["PATH_INFO"]);
            assert_eq!("/fizz/buzz", parsed_response["PATH_TRANSLATED"]);
            assert_eq!("/wildcardparent/wildcard/...", parsed_response["X_MATCHED_ROUTE"]);
            assert_eq!("/fizz/buzz", parsed_response["X_RAW_PATH_INFO"]);
            assert_eq!("/wildcardparent/wildcard", parsed_response["SCRIPT_NAME"]);
        }
    }

    #[tokio::test]
    pub async fn health_check_builtin_takes_precedence_over_user_routes() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/healthz").body(empty_body);

        let response = send_request_to_module_map(TEST_HEALTHZ_MODULE_MAP_FILE, None, request).await;

        assert_eq!(hyper::StatusCode::OK, response.status());
        let response_body = hyper::body::to_bytes(response.into_body()).await
            .expect("Could not get bytes from response body");
        let response_text = std::str::from_utf8(&response_body)
            .expect("Could not read body as string");
        assert_eq!("OK", response_text);
    }

    // This test is run synchronously because if we use tokio::test, something hangs inside
    // wasi-experimental-http-wasmtime while sending the HTTP request.  (This *doesn't* affect
    // normal use - the library is careful to check for the presence of a Tokio runtime -
    // but something about the test environment was different.)
    #[test]
    pub fn can_perform_http_requests() {
        let empty_body = hyper::body::Body::empty();
        let request = hyper::Request::get("http://127.0.0.1:3000/").body(empty_body);

        let runtime = tokio::runtime::Runtime::new()
            .expect("Could not create Tokio runtime for HTTP test");

        let jh = runtime.spawn(async move {
            let response = send_request_to_standalone_bindle(HTTP_TEST_ID, request).await;

            let status = response.status();
            let response_body = hyper::body::to_bytes(response.into_body()).await
                .expect("Could not get bytes from response body");
            let response_text = std::str::from_utf8(&response_body)
                .expect("Could not read body as string");
            (status, response_text.to_owned())
        });

        let (status, response_text) = futures::executor::block_on(jh).unwrap();

        assert_eq!(hyper::StatusCode::OK, status);
        assert!(response_text.contains("is HEALTHY") ||
            response_text.contains("is UNHEALTHY"));
    }
}
