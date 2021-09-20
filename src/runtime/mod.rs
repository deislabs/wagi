//! The tools for executing WAGI modules, and managing the lifecycle of a request.

use std::sync::{Arc};
use std::{collections::HashMap};
use std::{
    hash::{Hash, Hasher},
};

// use oci_distribution::client::{Client, ClientConfig};
// use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;
// use docker_credential;
// use docker_credential::DockerCredential;

use crate::dispatcher::{RouteHandler, RoutePattern, RoutingTableEntry, WasmRouteHandler};
use crate::wasm_module::WasmModuleSource;

pub mod bindle;

/// The default Bindle server URL.
pub const DEFAULT_BINDLE_SERVER: &str = "http://localhost:8080/v1";

// const WASM_LAYER_CONTENT_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";
// const STDERR_FILE: &str = "module.stderr";

/// An internal representation of a mapping from a URI fragment to a function in a module.
#[derive(Clone)]
pub struct RouteEntry {
    pub path: String,
    pub entrypoint: String,
}

/// A handler contains all of the information necessary to execute the correct function on a module.
#[derive(Clone, Debug)]
pub struct Handler {
    /// A reference to the module for this handler.
    pub module: Module,
    /// The function that should be called to handle this path.
    pub entrypoint: String,
    /// The path pattern that this handler answers.
    ///
    // For example, an exact path `/foo/bar` may be returned, as may a wildcard path such as `/foo/...`
    //
    // This path is the _fully constructed_ path. That is, if a module config declares its path as `/base`,
    // and the module registers the path `/foo/...`, the value of this would be `/base/foo/...`.
    pub path: String,
}

impl Handler {
    /// Given a module and a route entry, create a new handler.
    pub fn new(entry: RouteEntry, module: Module) -> Self {
        Handler {
            path: entry.path,
            entrypoint: entry.entrypoint,
            module,
        }
    }
}

/// Description of a single WAGI module
#[derive(Clone, Debug, Deserialize)]
pub struct Module {
    #[serde(skip)]
    rte: RoutingTableEntry,

    /// The route, begining with a leading slash.
    ///
    /// The suffix "/..." means "this route and all sub-paths". For example, the route
    /// "/foo/..." will match "/foo" as well as "/foo/bar" and "/foo/bar/baz"
    pub route: String,
    /// The path to the module that will be loaded.
    ///
    /// This should be an absolute path. It must point to a WASM+WASI 32-bit program
    /// with the read bit set.
    pub module: String,
    /// Directories on the local filesystem that can be opened by this module
    /// The key (left value) is the name of the directory INSIDE the WASM. The value is
    /// the location OUTSIDE the WASM. Two inside locations can map to the same outside
    /// location.
    pub volumes: Option<HashMap<String, String>>,
    /// The name of the function that is the entrypoint for executing the module.
    /// The default is `_start`.
    pub entrypoint: Option<String>,
    /// The URL fragment for the bindle server.
    ///
    /// If none is supplied, then http://localhost:8080/v1 is used
    pub bindle_server: Option<String>,

    /// List of hosts that the guest module is allowed to make HTTP requests to.
    /// If none or an empty vector is supplied, the guest module cannot send
    /// requests to any server.
    pub allowed_hosts: Option<Vec<String>>,

    /// Max http concurrency that the guest module configures for the HTTP
    /// client. If none, the guest module uses the default concurrency provided
    /// by the WASM HTTP client module.
    pub http_max_concurrency: Option<u32>,
}

// For hashing, we don't need all of the fields to hash. A wasm module (not a `Module`) can be used
// multiple times and configured different ways, but the route can only be used once per WAGI
// instance
impl Hash for Module {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.route.hash(state);
    }
}

impl PartialEq for Module {
    fn eq(&self, other: &Self) -> bool {
        self.route == other.route
    }
}

impl Eq for Module {}

impl Module {
    pub fn new(route: String, module_uri: String) -> Self {
        // TODO: OH GOSH NO
        let module_file_path = match Url::parse(&module_uri) {
            Err(_) => module_uri.clone(),
            Ok(u) => match u.scheme() {
                "file" => String::from(u.to_file_path().unwrap().to_string_lossy()),
                s => panic!("Not doing module ref scheme {} during transition", s),
            }
        };
        let parcel_bytes = Arc::new(std::fs::read(&module_file_path).unwrap());
        Module {
            rte: RoutingTableEntry {
                route_pattern: RoutePattern::parse(&route),
                handler_info: RouteHandler::Wasm(WasmRouteHandler {
                    wasm_module_source: WasmModuleSource::Blob(parcel_bytes),
                    entrypoint: "_start".to_owned(),
                    volumes: HashMap::new(),
                    allowed_hosts: None,
                    http_max_concurrency: None,
                }),
                handler_name: "TODO TODO TODO".to_owned(),
            },
            route,
            module: module_uri,
            volumes: None,
            entrypoint: None,
            allowed_hosts: None,
            bindle_server: None,
            http_max_concurrency: None,
        }
    }

    // /// Execute the WASM module in a WAGI
    // ///
    // /// The given `base_log_dir` should be a directory where all module logs will be stored. When
    // /// executing a module, a subdirectory will be created in this directory with the ID (from the
    // /// [`id` method](Module::id)) for its name. The log will be placed in that directory at
    // /// `module.stderr`
    // #[allow(clippy::too_many_arguments)]
    // #[instrument(level = "trace", skip(self, req, request_context, global_context), fields(route = %self.route, module = %self.module))]
    // pub async fn execute(
    //     &self,
    //     req: Request<Body>,
    //     request_context: RequestContext,
    //     global_context: RequestGlobalContext,
    // ) -> Response<Body> {
    //     // Read the parts in here
    //     let (parts, body) = req.into_parts();
    //     let data = hyper::body::to_bytes(body)
    //         .await
    //         .unwrap_or_default()
    //         .to_vec();
    //     self.rte.handle_request(&parts, data, &request_context, &global_context)

    //     // let me = self.clone();
    //     // let res = match tokio::task::spawn_blocking(move ||
    //     //         me.run_wasm(&parts, data, &request_context, &route_context, &global_context)
    //     // ).await {
    //     //     Ok(res) => res,
    //     //     Err(e) if e.is_panic() => {
    //     //         tracing::error!(error = %e, "Recoverable panic on Wasm Runner thread");
    //     //         return internal_error("Module run error");
    //     //     }
    //     //     Err(e) => {
    //     //         tracing::error!(error = %e, "Recoverable panic on Wasm Runner thread");
    //     //         return internal_error("module run was cancelled");
    //     //     }
    //     // };
    //     // match res {
    //     //     Ok(res) => res,
    //     //     Err(e) => {
    //     //         tracing::error!(error = %e, "error running WASM module");
    //     //         // A 500 error makes sense here
    //     //         let mut srv_err = Response::default();
    //     //         *srv_err.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    //     //         srv_err
    //     //     }
    //     // }
    // }

    /// Returns the unique ID of the module.
    ///
    /// This is the SHA256 sum of the following data, written into the hasher in the following order
    /// (skipping any `None`s):
    ///
    /// - route
    /// - host
    pub fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.route);
        format!("{:x}", hasher.finalize())
    }

    /// Examine the given module to see if it has any routes.
    ///
    /// If it has any routes, add them to the vector and return it. The given `base_log_dir` should
    /// be a directory where all module logs will be stored. When executing a module, a subdirectory
    /// will be created in this directory with the ID (from the [`id` method](Module::id)) for its
    /// name. The log will be placed in that directory at `module.stderr`
    /*
    #[instrument(
        level = "trace",
        skip(self, cache_config_path, module_cache_dir, base_log_dir)
    )]
    pub(crate) fn load_routes(
        &self,
        cache_config_path: &Path,
        module_cache_dir: &Path,
        base_log_dir: &Path,
    ) -> Result<Vec<RouteEntry>, anyhow::Error> {
        let startup_span = tracing::info_span!("route_instantiation").entered();

        let prefix = self
            .route
            .strip_suffix("/...")
            .unwrap_or_else(|| self.route.as_str());
        let mut routes = vec![RouteEntry {
            path: self.route.to_owned(), // We don't use prefix because prefix has been normalized.
            entrypoint: self
                .entrypoint
                .clone()
                .unwrap_or_else(|| "_start".to_string()),
        }];

        // TODO: We should dedup this code somewhere because there are plenty of similarities to
        // `run_wasm`

        // Make sure the directory exists
        let log_dir = base_log_dir.join(self.id());
        std::fs::create_dir_all(&log_dir)?;
        // Open a file for appending. Right now this will just keep appending as there is no log
        // rotation or cleanup
        let stderr = cap_std::fs::File::from_std(
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(log_dir.join(STDERR_FILE))?,
            ambient_authority(),
        );
        let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);

        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());

        let ctx = WasiCtxBuilder::new()
            .stderr(Box::new(stderr))
            .stdout(Box::new(stdout))
            .build();

        let (mut store, engine) = self.new_store_and_engine(cache_config_path, ctx)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

        let http = wasi_experimental_http_wasmtime::HttpCtx::new(None, None)?;
        http.add_to_linker(&mut linker)?;

        let module = self.load_cached_module(&store, module_cache_dir)?;
        let instance = linker.instantiate(&mut store, &module)?;

        // Manually drop the span to get the instantiation time
        drop(startup_span);

        match instance.get_func(&mut store, "_routes") {
            Some(func) => {
                func.call(&mut store, &[])?;
            }
            None => return Ok(routes),
        }

        let out = stdout_mutex.read().unwrap();
        out.lines().for_each(|line_result| {
            if let Ok(line) = line_result {
                // Split line into parts
                let parts: Vec<&str> = line.trim().split_whitespace().collect();

                if parts.is_empty() {
                    return;
                }

                let key = parts.get(0).unwrap_or(&"/").to_string();
                let val = parts.get(1).unwrap_or(&"_start").to_string();
                routes.push(RouteEntry {
                    path: format!("{}{}", prefix, key),
                    entrypoint: val,
                });
            }
        });
        // We reverse the routes so that the top-level routes are evaluated last.
        // This gives a predictable order for traversing routes. Because the base path
        // is the last one evaluated, if the base path is /..., it will match when no
        // other more specific route lasts.
        //
        // Additionally, when Wasm authors create their _routes() callback, they can
        // organize their outputs to match according to their own precedence merely by
        // putting the higher precedence routes at the end of the output.
        routes.reverse();
        Ok(routes)
    }
    */

    /// Resolve a relative path from the end of the matched path to the end of the string.
    ///
    /// For example, if the match is `/foo/...` and the path is `/foo/bar`, it should return `"bar"`,
    /// but if the match is `/foo/bar` and the path is `/foo/bar`, it should return `""`.
    pub fn path_info(&self, uri_path: &str) -> String {
        uri_path
            .strip_prefix(
                // Chop the `/...` off of the end if there is one.
                self.route
                    .strip_suffix("/...")
                    .unwrap_or_else(|| self.route.as_str()),
            )
            // It is possible that a root path request matching /... returns a None here,
            // so in that case the appropriate return is "".
            .unwrap_or("")
            .to_owned()
    }

    /*
    /// Determine the source of the module, and read it from that source.
    ///
    /// Modules can be stored locally, or they can be stored in external sources like
    /// Bindle. WAGI determines the source by looking at the URI of the module.
    ///
    /// - If `file:` is specified, or no schema is specified, this loads from the local filesystem
    /// - If `bindle:` is specified, this will retrieve the module from the configured Bindle server
    /// - If `oci:` is specified, this will retrieve the module from an OCI Distribution registry
    ///
    /// While `file` is a little lenient in its adherence to the URL spec, `bindle` and `oci` are not.
    /// For example, an `oci` URL that references `alpine:latest` should be `oci:alpine:latest`.
    /// It should NOT be `oci://alpine:latest` because `alpine` is not a host name.
    async fn load_module(
        &self,
        store: &Store<WasiCtx>,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        tracing::trace!(
            module = %self.module,
            "Loading from source"
        );
        match Url::parse(self.module.as_str()) {
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "Error parsing module URI. Assuming this is a local file"
                );
                wasmtime::Module::from_file(store.engine(), self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => {
                    match uri.to_file_path() {
                        Ok(p) => return wasmtime::Module::from_file(store.engine(), p),
                        Err(e) => anyhow::bail!("Cannot get path to file: {:#?}", e),
                    };
                }
                "bindle" => self.load_bindle(&uri, store.engine(), cache).await,
                "parcel" => self.load_parcel(&uri, store.engine(), cache).await,
                "oci" => self.load_oci(&uri, store.engine(), cache).await,
                s => anyhow::bail!("Unknown scheme {}", s),
            },
        }
    }
    */

    /*
    /// Load a cached module from the filesystem.
    ///
    /// This is synchronous right now because Wasmtime on the runner needs to be run synchronously.
    /// This will change when the new version of Wasmtime adds Send + Sync to all the things.
    /// Then we can just do `load_module` or refactor this to be async.
    #[instrument(level = "info", skip(self, store, cache_dir), fields(cache = %cache_dir.display(), module = %self.module))]
    fn load_cached_module(
        &self,
        store: &Store<WasiCtx>,
        cache_dir: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        let canonical_path = match Url::parse(self.module.as_str()) {
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "Error parsing module URI. Assuming this is a local file."
                );
                PathBuf::from(self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => match uri.to_file_path() {
                    Ok(p) => p,
                    Err(e) => anyhow::bail!("Cannot get path to file: {:#?}", e),
                },
                "bindle" => cache_dir.join(bindle_cache_key(&uri)),
                "parcel" => {
                    // parcel: bindle_uri#SHA256 becomes cache_dir/SHA256
                    let cache_file = uri.fragment().unwrap_or_else(|| uri.path()); // should always have fragment
                    cache_dir.join(cache_file)
                }
                "oci" => cache_dir.join(self.hash_name()),
                s => {
                    tracing::error!(scheme = s, "unknown scheme in module");
                    anyhow::bail!("Unknown scheme {}", s)
                }
            },
        };
        tracing::trace!(?canonical_path);

        // If there is a module at this path, load it.
        // Right now, _any_ problem loading the module will result in us trying to
        // re-fetch it.
        match wasmtime::Module::from_file(store.engine(), canonical_path) {
            Ok(module) => Ok(module),
            Err(_e) => {
                tracing::debug!("module cache miss. Loading module from remote.");
                // TODO: This could be reallllllllly dangerous as we are for sure going to block at this
                // point on this current thread. This _should_ be ok given that we run this as a
                // spawn_blocking, but those sound like famous last words waiting to happen. Refactor this
                // sooner rather than later
                futures::executor::block_on(self.load_module(&store, cache_dir))
            }
        }
    }
    */

    /*
    #[instrument(level = "info", skip(self, engine, cache))]
    async fn load_oci(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        let config = ClientConfig {
            protocol: oci_distribution::client::ClientProtocol::HttpsExcept(vec![
                "localhost:5000".to_owned(),
                "127.0.0.1:5000".to_owned(),
            ]),
        };
        let mut oc = Client::new(config);

        let mut auth = RegistryAuth::Anonymous;

        if let Ok(credential) = docker_credential::get_credential(uri.as_str()) {
            if let DockerCredential::UsernamePassword(user_name, password) = credential {
                auth = RegistryAuth::Basic(user_name, password);
            };
        };

        let img = url_to_oci(uri).map_err(|e| {
            tracing::error!(
                error = %e,
                "Could not convert uri to OCI reference"
            );
            e
        })?;
        let data = oc
            .pull(&img, &auth, vec![WASM_LAYER_CONTENT_TYPE])
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Pull failed");
                e
            })?;
        if data.layers.is_empty() {
            tracing::error!(image = %img, "Image has no layers");
            anyhow::bail!("image has no layers");
        }
        let first_layer = data.layers.get(0).unwrap();

        // If a cache write fails, log it but continue on.
        tracing::trace!("writing layer to module cache");
        if let Err(e) =
            tokio::fs::write(cache.join(self.hash_name()), first_layer.data.as_slice()).await
        {
            tracing::warn!(error = %e, "failed to write module to cache");
        }
        let module = wasmtime::Module::new(engine, first_layer.data.as_slice())?;
        Ok(module)
    }
    */
}

/// Build the image name from the URL passed in.
/// So oci://example.com/foo:latest will become example.com/foo:latest
///
/// If parsing fails, this will emit an error.
fn url_to_oci(uri: &Url) -> anyhow::Result<Reference> {
    let name = uri.path().trim_start_matches('/');
    let port = uri.port().map(|p| format!(":{}", p)).unwrap_or_default();
    let r: Reference = match uri.host() {
        Some(host) => format!("{}{}/{}", host, port, name).parse(),
        None => name.parse(),
    }?;
    Ok(r) // Because who doesn't love OKRs.
}

#[cfg(test)]
mod test {
    use super::{url_to_oci};

    use std::io::Write;
    use tempfile::NamedTempFile;

    // const ROUTES_WAT: &str = r#"
    // (module
    //     (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
    //     (memory 1)
    //     (export "memory" (memory 0))

    //     (data (i32.const 8) "/one one\n/two/... two\n")

    //     (func $main (export "_routes")
    //         (i32.store (i32.const 0) (i32.const 8))
    //         (i32.store (i32.const 4) (i32.const 22))

    //         (call $fd_write
    //             (i32.const 1)
    //             (i32.const 0)
    //             (i32.const 1)
    //             (i32.const 20)
    //         )
    //         drop
    //     )
    // )
    // "#;

    // fn write_temp_wat(data: &str) -> anyhow::Result<NamedTempFile> {
    //     let mut tf = tempfile::NamedTempFile::new()?;
    //     write!(tf, "{}", data)?;
    //     Ok(tf)
    // }

    // TODO: rebuild this test
    // #[tokio::test]
    // async fn load_routes_from_wasm() {
    //     let tf = write_temp_wat(ROUTES_WAT).expect("created tempfile");
    //     let urlish = format!("file:{}", tf.path().to_string_lossy());

    //     let cache = PathBuf::from("cache.toml");

    //     // We should be able to mount the same wasm at a separate route.
    //     let module = Module::new("/base".to_string(), urlish.clone());
    //     let module2 = Module::new("/another/...".to_string(), urlish);

    //     let mut mc = ModuleConfig {
    //         modules: vec![module.clone(), module2.clone()].into_iter().collect(),
    //         route_cache: None,
    //     };

    //     let log_tempdir = tempfile::tempdir().expect("Unable to create tempdir");
    //     let cache_tempdir = tempfile::tempdir().expect("new cache temp dir");
    //     mc.build_registry(&cache, cache_tempdir.path(), log_tempdir.path())
    //         .await
    //         .expect("registry build cleanly");

    //     tracing::debug!(route_cache = ?mc.route_cache);

    //     // Three routes for each module.
    //     assert_eq!(6, mc.route_cache.as_ref().expect("routes are set").len());

    //     let modpath = module.module.clone();

    //     // Base route is from the config file
    //     let base = mc
    //         .handler_for_path("/base")
    //         .expect("Should get a /base route");
    //     assert_eq!("_start", base.entrypoint);
    //     assert_eq!(modpath, base.module.module);

    //     // Route one is from the module's _routes()
    //     let one = mc
    //         .handler_for_path("/base/one")
    //         .expect("Should get the /base/one route");

    //     assert_eq!("one", one.entrypoint);
    //     assert_eq!(modpath, one.module.module);

    //     // Route two is a wildcard.
    //     let two = mc
    //         .handler_for_path("/base/two/three")
    //         .expect("Should get the /base/two/... route");

    //     assert_eq!("two", two.entrypoint);
    //     assert_eq!(modpath, two.module.module);

    //     // This should fail
    //     assert!(mc.handler_for_path("/base/no/such/path").is_err());

    //     // This should pass
    //     mc.handler_for_path("/another/path")
    //         .expect("The generic handler should have been returned for this");
    // }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn should_parse_file_with_all_the_windows_slashes() {
        let tf = write_temp_wat(ROUTES_WAT).expect("wrote tempfile");
        let testcases = possible_slashes_for_paths(tf.path().to_string_lossy().to_string());
        for test in testcases {
            let module = Module::new("/base".to_string(), test);
            let ctx = WasiCtxBuilder::new().build();
            let engine = Engine::default();
            let store = Store::new(&engine, ctx);
            let tempdir = tempfile::tempdir().expect("create a temp dir");

            module
                .load_module(&store, tempdir.path())
                .await
                .expect("loaded module");
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

    // Why is there a test for upstream libraries? Well, because they each seem to have
    // quirks that cause them to differ from the spec. This is here because we plan on
    // changing to Hyper when it gets updated, but for now are using URL.
    //
    // Note that `url` follows the WhatWG convention of omitting `localhost` in `file:` urls.
    #[test]
    fn should_parse_file_scheme() {
        let uri = url::Url::parse("file:///foo/bar").expect("Should parse URI with no host");
        assert!(uri.host().is_none());

        let uri = url::Url::parse("file:/foo/bar").expect("Should parse URI with no host");
        assert!(uri.host().is_none());

        let uri =
            url::Url::parse("file://localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("/foo/bar", uri.path());
        // Here's why: https://github.com/whatwg/url/pull/544
        assert!(uri.host().is_none());

        let uri =
            url::Url::parse("foo://localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("/foo/bar", uri.path());
        assert_eq!(uri.host_str(), Some("localhost"));

        let uri =
            url::Url::parse("bindle:localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("localhost/foo/bar", uri.path());
        assert!(uri.host().is_none());

        // Two from the Bindle spec
        let uri = url::Url::parse("bindle:example.com/hello_world/1.2.3")
            .expect("Should parse URI with no host");
        assert_eq!("example.com/hello_world/1.2.3", uri.path());
        assert!(uri.host().is_none());

        let uri = url::Url::parse(
            "bindle:github.com/deislabs/example_bindle/123.234.34567-alpha.9999+hellothere",
        )
        .expect("Should parse URI with no host");
        assert_eq!(
            "github.com/deislabs/example_bindle/123.234.34567-alpha.9999+hellothere",
            uri.path()
        );
        assert!(uri.host().is_none());
    }

    #[test]
    fn test_url_to_oci() {
        let uri = url::Url::parse("oci:foo:bar").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("foo:bar", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci:example/foo:1.2.3").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example/foo:1.2.3", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com:9000/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com:9000/foo:dev", oci.whole().as_str());
    }
}
