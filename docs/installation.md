# Installing the WAGI Server

## Prebuilt Binaries

To get started with a binary release, head to the [releases page](https://github.com/deislabs/wagi/releases)
and download the desired release. Usually, the most recent release is the one you want.

You can generate and compare the SHA with `shasum`:

```console
$ shasum wagi-v0.8.1-linux-amd64.tar.gz
ad4114b2ed9e510a8c24348d5ea544da55c685f5  wagi-v0.8.1-linux-amd64.tar.gz
```

You can then compare that SHA with the one present in the release notes.

Unpack the `.tar.gz`. The `wagi` file is the binary server.
You may wish to put it on your `PATH` at a location such as `/usr/local/bin`.
But that is up to you.
On some systems, you may need to set the execute bit (`chmod 755 wagi`).

From there, you can run `wagi` directly. We recommend starting with `wagi --help`.

The rest of this document deals with building and running Wagi from source.

## Prerequisites for Working With Source

- Rust (a recent version. We suggest 1.52 or later)
- A Linux/macOS/UNIX/WSL2/Windows environment
- OpenSSL or existing SSL certificates if you want TLS support

We recently started testing WAGI on Windows, so please file an issue if you 
encounter any issues.

> On Windows, you may prefer to use `just` instead of `make` to run the `Makefile` commands.

## Building from Source

To build a static binary, run the following command:

```console
$ make build
   Compiling wagi v0.8.1 (/Users/technosophos/Code/Rust/wagi)
    Finished release [optimized] target(s) in 18.47s
```

Once it has been built, the binary will be available as `target/release/wagi`.

You can move the binary to any location you choose. Just make sure it has execution permissions set.

## Running from Source

If you prefer to run from source without building, you can use `make serve` (which runs `cargo run` with all the settings).
You can test out SSL/TLS with `make gen-cert` to generate a testing certificate, and `make serve-tls` to serve with that certificate.

> Note that if you are using a self-signed certificate, you will need to supply the `-k` flag to curl.

When running using `cargo run` or `cargo build` manually, we recommend using the `--release` flag if WebAssembly performance is important to you.

## What's Next?

Continue on to [Configuring and Running WAGI](configuring_and_running.md) to learn about running WAGI.
