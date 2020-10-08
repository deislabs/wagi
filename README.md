# WAGI: Web Assembly Gateway Interface

Like fashion, all technologies eventually make a comeback.
WAGI (pronounced "waggy") is an implementation of CGI for WebAssembly and WASI.

In the current specification, WASI does not have a networking layer.
It has environment variables and file handles, but no way of creating a server.
Further, current WASM implementations are single-threaded and have no support for concurrency.
So it would be difficult to write a decent web server implementation anyway.

But these problems are nothing new.
In the 1990s, as the web was being created, a standard arose to make it easy to attach scripts to a web server, thus providing dynamic server-side logic.
This standard was the [Common Gateway Interface](https://tools.ietf.org/html/draft-robinson-www-interface-00), or CGI.
CGI made it dead simple to write stand-alone programs in any language, and have them answer Web requests.
WASM + WASI is suited to implementing the same features.

WAGI provides an HTTP server implementation that can dynamically load and execute WASM modules using the same techniques as CGI scripts.
Headers are placed in environment variables.
Query parameters, when present, are sent in as command line options.
Incoming HTTP payloads are sent in via STDIN.
And the response is simply written to STDOUT.

Because the system is so simple, writing and testing WAGI is simple, regardless of the language you choose to write in.
You can even execute WAGIs without a server runtime.

WAGI is designed to be higher in security than CGI.
Thus WAGIs have more security restrictions.
They cannot access many things (including the filesystem) without explicit access grants.
They cannot make outbound network connections.
They cannot execute other executables on the system.
They cannot access arbitrary environment variables--only ones that are explicitly passed in.

In the future, as WASI matures, we will relax the restrictions on outbound networking.

## Getting Started

To run the WAGI server, use `cargo run`.

To get an idea for writing your own WAGI, check out the [examples](examples/) folder.

And if you want to understand the details, read the [Common Gateway Interface](https://tools.ietf.org/html/draft-robinson-www-interface-00) specification.

## Configuring the WAGI Server

Each WAGI can be accompanied by a `wagi.toml`, which contains configuration for that WAGI.


## Differences from old CGI

- We use UTF-8 instead of ASCII.
- WAGIs are not handled as "processes", they are executed internally with multi-threading.
- WAGIs do _not_ have unrestricted access to the underlying OS or filesystem.
    * If you want to give a WAGI access to a portion of the filesystem, you must configure the WAGI's `wagi.toml` file
    * WAGIs cannot make outbound network connections