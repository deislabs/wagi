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

In previous releases, although the daemon (the WAGI server) runs constantly,
both the `modules.toml` and the `.wasm` file were loaded from disk each request, much as they were for CGI.
As of the time of writing, the WAGI server now reads the WASM modules at startup and keeps
them in memory.  This abstracts serving code away from filesystem interactions, and also
improves performance.

## Design notes

This implementation of WAGI falls into two parts:

* Initialisation (the bulk of `main.rs`)
* Request serving (`wagi_server` and the components it calls)

After initialisation we should know everything we need to know to handle requests,
and we should have failed if we can determine that anything is missing or
invalid. All configuration files have been parsed and validated, all modules have
been downloaded and read, all dependencies have been readied, etc.  If initialisation
fails, WAGI stops rapidly with an error message.

**Caveat:** We could probably perform even more validation during the initialisation
phase. For example, at the time of writing, we don't check if route entry points
exist.

Because any failure during initialisation should cause an immediate exit, we do
only minimal tracing during this phase; the exit and error message should provide
enough information to diagnose any problems.

### Principles of the initialisation phase

* Parse, don't validate.  That is, convert raw data such as config files into a
  form that minimises further checking or special case handling later on.
* Don't make downstream components care about how upstream components got their
  data.  This is not always practical, but the idea is to minimise how much, say,
  the route builder needs to care about whether it is dealing with an OCI reference
  in a `modules.toml` or a parcel in a local standalone bindle. Separate the stages;
  keep `main()` as simple and as linear as possible.
* Fail fast.  Related to the above, check that everything
  you need is present, in the right place, and usable.  Ideally parse it into
  a form such that the next stage doesn't need to repeat the checks.
* Fail informatively.  Be generous with error context and values.  Rust has
  an awful habit of reporting things like "key not in dictionary" and "file
  or directory does not exist."  Err on the side of saying _which_ thing
  went wrong.
* Provide entry points for automated testing.

### Key types and function groups

* Initialisation is geared to producing a `RoutingTable` which maps routes to handlers.
  A `RoutingTable` consists primarily of a vector of `RoutingTableEntry`. ('Map' is
  a slight misnomer here, because of ordering and wildcard routes.)
* `RoutingTableEntry` contains a route (represented by `RoutePattern`) and all the data
  required to handle that route (represented by the `RouteHandler` enum).
* The types with "handler" in the name can be a bit confusing.  We need them because
  we have different representations of handlers as we assemble the data we need to
  run them.
  - `RouteHandler` is the final, "runnable" form of handler.
  - `WasmRouteHandler` is the data for the interesting case of `RouteHandler`.
  - `WagiHandlerInfo` aggregates the information about a route and associated parcels
    specified in a bindle.
  - `HandlerConfigurationSource` represents the combination of flags passed on the
    command line to say where routing and handling is specified, e.g. a `modules.toml`
    file or a bindle.
  - `HandlerConfiguration` represents the parsed form of whatever the
    `HandlerConfigurationSource` points to. Note that `HandlerConfigurationSource` is the
    _reference_ to the source (e.g. file path or bindle ID); `HandlerConfiguration` is
    _the content of that the file or bindle_.
  - `LoadedHandlerConfiguration` is a `HandlerConfiguration` augmented with the binary
    content of the Wasm modules specified in that configuration.
  - Note that all those last three are different _again_ from `WagiConfiguration`
    which contains a whole bunch of other configuration like TLS and stuff.
  - I am very very sorry for everything.
* `WasmModuleSource` represents data that can be instantiated as a Wasm module. At the
  time of writing, the only case is `Blob`, which is the raw bytes of the Wasm binary.
  In future, this could have an additional case (or have a single different case!) of
  a pre-instantiated module - the point of the type is to insulate other code from making
  assumptions about the representation.
* The `wasm_runner` module provides services for executing Wasm modules that communicate
  via stdin/stdout.  This allows commonality between dynamic route discovery and handler
  execution.  There is scope for more encapsulation here though!

We welcome improvements to and tidying of the module structure and placement of
functions.
