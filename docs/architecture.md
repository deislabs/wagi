# Architecture of WAGI

WAGI is essentially an implementation of the CGI protocol for WebAssembly.
This puts WAGI squarely in the domain of Functions as a Service (FaaS),
though it follows an existing specification instead of inventing a new one.

## What Is WebAssembly Gateway Interface (WAGI)

Like fashion, all technologies eventually make a comeback.
WAGI (pronounced "waggy") is an implementation of CGI for WebAssembly and
WebAssembly System Interface (WASI).

In the current specification, WASI does not have a networking layer.
It has environment variables and file handles, but no way of creating a server.
Further, current WASM implementations are single-threaded and have no support for concurrency.
So it would be difficult to write a decent web server implementation anyway.

But these problems are nothing new.
In the 1990s, as the web was being created, a standard arose to make it easy to attach scripts to a web server, thus providing dynamic server-side logic.
This standard was the [Common Gateway Interface](https://tools.ietf.org/html/rfc3875), or CGI.
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

- They cannot access many things (including the filesystem) without explicit access grants.
- They cannot make outbound network connections.
- They cannot execute other executables on the system.
- They cannot access arbitrary environment variables--only ones that are explicitly passed in.

In the future, as WASI matures, we will relax the restrictions on outbound networking.

## Differences from CGI 1.1

- We use UTF-8 instead of ASCII.
- WAGIs are not handled as "processes", they are executed internally with multi-threading.
- WAGIs do _not_ have unrestricted access to the underlying OS or filesystem.
  * If you want to give a WAGI access to a portion of the filesystem, you must configure the WAGI's `wagi.toml` file
  * WAGIs cannot make outbound network connections
  * Some CGI env vars are rewritten to remove local FS information
- WAGIs have a few extra CGI environment variables, prefixed with `X_`.
- A `location` header from a WAGI must return a full URL, not a path. (CGI supports both)
  * This will set the status code to `302 Found` (per 6.2.4 of the CGI specification)
  * If `status` is returned AFTER `location`, it will override the status code
- WAGI does NOT support NPH (Non-Parsed Header) mode
- The value of `args` is NOT escaped for borne-style shells (See section 7.2 of CGI spec)

It should be noted that while the daemon (the WAGI server) runs constantly, both the `modules.toml` and the `.wasm` file are loaded for each request, much as they were for CGI.
In the future, the WAGI server may cache the WASM modules to speed loading.
But in the near term, we are less concerned with performance and more concerned with debugging.
