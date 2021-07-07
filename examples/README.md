# Wagi Examples

These examples provide an easy way to get started with Wagi.

- `cache.toml` illustrates using the caching features of Wagi
- `hello.wasm` and `hello.wat` provide simple WebAssembly modules for testing
- `http-example.wasm` provides a simple example of our HTTP client library support for Wagi modules
- `invoice.toml` and `mkbindle.rs` are for loading these testing modules into a Bindle server
- `modules.toml` is for loading the modules into Wagi straight off of the disk
- `error.wat` shows an example of sending an HTTP error.

## Getting Started with `modules.toml`

If you have Wagi compiled, you can use `wagi -c examples/modules.toml` to start up a Wagi
server using `modules.toml`.

Take a look at the `Makefile` and `make serve` for an example.

## Getting Started with `invoice.toml` and `mkbindle.rs`

If you already have a Bindle server, you can load the examples into a Bindle server:

```console
$ cargo run --example mkbindle
$ wagi -b example.com/hello/1.3.3
```

If your Bindle server isn't running at "http://localhost:8080/v1",
you can use the `BINDLE_URL` environment variable or the `--bindle-url` argument
to Wagi.

Take a look at the `Makefile` and `make run-bindle` for more examples.

## Caching

If you would like to turn on caching, take a look at `cache.toml`.
You will need to set some paths.