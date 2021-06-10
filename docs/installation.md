# Installing the WAGI Server

Currently there are no prebuilt binaries of WAGI. You will need to build your own.

## Prerequisites

- Rust (a recent version. We suggest 1.47 or later)
- A Linux/macOS/UNIX/WSL2/Windows environment

We recently started testing WAGI on Windows, so please file an issue if you 
encounter any issues.

## Building

To build a static binary, run the following command:

```console
$ cargo build --release 
   Compiling wagi v0.1.0 (/Users/technosophos/Code/Rust/wagi)
    Finished release [optimized] target(s) in 18.47s
```

We recommend using `--release` to speed up the WebAssembly runtime execution.

Once it has been built, the binary will be available as `target/release/wagi`.

You can move the binary to any location you choose. Just make sure it has execution permissions set.

## Running from Source

If you prefer to run from source without building, you can use `cargo run`.
Again, we recommend using the `--release` flag if WebAssembly performance is important to you.

## What's Next?

Continue on to [Configuring and Running WAGI](configuring_and_running.md) to learn about running WAGI.
