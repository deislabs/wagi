// Why is there a test for upstream libraries? Well, because they each seem to have
// quirks that cause them to differ from the spec. This is here because we plan on
// changing to Hyper when it gets updated, but for now are using URL.
//
// Note that `url` follows the WhatWG convention of omitting `localhost` in `file:` urls.

#[cfg(test)]
mod test {
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[allow(dead_code)]  // Used on Windows
    const ROUTES_WAT: &str = r#"
    (module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1)
        (export "memory" (memory 0))

        (data (i32.const 8) "/one one\n/two/... two\n")

        (func $main (export "_routes")
            (i32.store (i32.const 0) (i32.const 8))
            (i32.store (i32.const 4) (i32.const 22))

            (call $fd_write
                (i32.const 1)
                (i32.const 0)
                (i32.const 1)
                (i32.const 20)
            )
            drop
        )
    )
    "#;

    #[allow(dead_code)]  // Used on Windows
    fn write_temp_wat(data: &str) -> anyhow::Result<NamedTempFile> {
        let mut tf = tempfile::NamedTempFile::new()?;
        write!(tf, "{}", data)?;
        Ok(tf)
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn should_parse_file_with_all_the_windows_slashes() {
        use wasi_cap_std_sync::WasiCtxBuilder;
        use wasmtime::*;

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
}
