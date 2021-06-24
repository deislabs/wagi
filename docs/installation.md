# Installing the WAGI Server

Currently there are no prebuilt binaries of WAGI. You will need to build your own.

## Prerequisites

- Rust (a recent version. We suggest 1.52 or later)
- A Linux/macOS/UNIX/WSL2/Windows environment
- OpenSSL or existing SSL certificates if you want TLS support

We recently started testing WAGI on Windows, so please file an issue if you 
encounter any issues.

> On Windows, you may prefer to use `just` instead of `make` to run the `Makefile` commands.

## Building

To build a static binary, run the following command:

```console
$ make build
   Compiling wagi v0.1.0 (/Users/technosophos/Code/Rust/wagi)
    Finished release [optimized] target(s) in 18.47s
```

Once it has been built, the binary will be available as `target/release/wagi`.

You can move the binary to any location you choose. Just make sure it has execution permissions set.

## Running from Source

If you prefer to run from source without building, you can use `make serve` (which runs `cargo run` with all the settings).
You can test out SSL/TLS with `make serve-tls`, which will automatically generate a self-signed certificate for WAGI to use.

> Note that if you are using a self-signed certificate, you will need to supply the `-k` flag to curl.

When running using `cargo run` or `cargo build` manually, we recommend using the `--release` flag if WebAssembly performance is important to you.

## What's Next?

Continue on to [Configuring and Running WAGI](configuring_and_running.md) to learn about running WAGI.
