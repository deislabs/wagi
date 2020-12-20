# WAGI: WebAssembly Gateway Interface

_WAGI is the easiest way to get started doing cloud-side WASM web apps._

**WARNING:** This is experimental code put together on a whim.
It is not considered production-grade by its developers, neither is it "supported" software.
This is a project we wrote to demonstrate another way to use WASI.

> DeisLabs is experimenting with many WASM technologies right now.
> This is one of a multitude of projects (including [Krustlet](https://github.com/deislabs/krustlet))
> designed to test the limits of WebAssembly as a cloud-based runtime.

## tl;dr

WAGI allows you to run WebAssembly WASI binaries as HTTP handlers.
Write a "command line" application that prints a few headers, and compile it to WASM32-WASI.
Add an entry to the `modules.toml` matching URL to WASM module.
That's it.

You can use any programming language that can compile to WASM32-WASI.

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

## Getting Started

To run the WAGI server, use `cargo run -- --config path/to/modules.toml`.
You can also `cargo build` WAGI and run it as a static binary.

Once you are running WAGI, you can test it out with your browser or `curl`. By default,
WAGI runs on `localhost:3000`: You can override this with `--listen`/`-l`.

```console
$ curl http://localhost:3000/hello/world
hello world
```

To get a look at the HTTP request and response, use the `-v` flag on `curl`:

```
$ curl -v http://localhost:3000/hello/world
*   Trying 127.0.0.1...
* TCP_NODELAY set
* Connected to localhost (127.0.0.1) port 3000 (#0)
> GET /hello/world HTTP/1.1
> Host: localhost:3000
> User-Agent: curl/7.64.1
> Accept: */*
>
< HTTP/1.1 200 OK
< content-type: text/plain
< content-length: 12
< date: Wed, 14 Oct 2020 22:00:59 GMT
<
hello world
* Connection #0 to host localhost left intact
* Closing connection 0
```

### Examples and Demos

- [env_wagi](https://github.com/deislabs/env_wagi): Dump the environment that WAGI sets up, including env vars and args.
- [hello-wagi-as](https://github.com/deislabs/hello-wagi-as): AssemblyScript example using environment variables and query params.

If you want to understand the details, read the [Common Gateway Interface 1.1](https://tools.ietf.org/html/rfc3875) specification.
While this is not an exact implementation, it is very close.
See the "Differences" section below for the differences.

## Configuring the WAGI Server

The WAGI server uses a `modules.toml` file to point to the WAGI modules that can be executed.
(A WAGI module is just a WASM+WASI module that prints its output correctly.)

Here is an example `modules.toml`:

```toml
[[modules]]
route = "/"
module = "/absolute/path/to/root.wasm"

[[modules]]
route = "/foo"
module = "/path/to/foo.wasm"

[[modules]]
# The "/..." suffix means this will match /bar and its subpaths, like /bar/a/b/c
route = "/bar/..."
module = "/path/to/bar.wasm"
# You can give WAGI access to particular directories on the filesystem.
volumes = {"/path/inside/wasm": "/path/on/host"}
# You can also put static environment variables in the TOML file
environment.TEST_NAME = "test value" 

[[modules]]
# You can also execute a WAT file directly
route = "/hello"
module = "/path/to/hello.wat"
```
### TOML fields

- Top-level fields
  - Currently none
- The `[[modules]]` list: Each module starts with a `[[modules]]` header. Inside of a module, the following fields are available:
  - `route`: The path that is appended to the server URL to create a full URL (e.g. `/foo` becomes `https://example.com/foo`)
  - `module`: The absolute path to the module on the file system
  - `environment`: A list of string/string environment variable pairs.
  - `repository`: RESERVED

## Writing WAGI Modules

A WAGI module is a WASM binary compiled with WASI support.

A module must have a `_start` method. Most of the time, that is generated by the compiler.
More often, an implementation of a WAGI module looks like this piece of pseudo-code:

```javascript
function main() {
    println("Content-Type: text/plain") // Required header
    println("")                         // Empty line is also require
    println("hello world")              // The body
}
```

Here is the above written in Rust:

```rust
fn main() {
    println!("Content-Type: text/plain\n");
    println!("hello world");
}
```

In Rust, you can compile the above with `cargo build --target wasm32-wasi --release` and have a WAGI module ready to use!

And here is an [AssemblyScript](https://www.assemblyscript.org) version:

```typescript
import "wasi";
import { Console } from "as-wasi";

Console.log("content-type: text-plain");
Console.log(""); // blank line separates headers from body.
Console.log("hello world");
```

Note that the AssemblyScript compiler generates the function body wrapper for you.
For more, check out the AssemblyScript WASI [docs](https://wasmbyexample.dev/examples/wasi-hello-world/wasi-hello-world.assemblyscript.en-us.html).

## The Enviornment Variables

These are the environment variables set on WAGI requests:

```bash
X_MATCHED_ROUTE="/envwasm"  # for example.com/envwasm
HTTP_ACCEPT="*/*"
REQUEST_METHOD="GET"
SERVER_PROTOCOL="http"
HTTP_USER_AGENT="curl/7.64.1"
CONTENT_TYPE=""             # Usually set on POST/PUT
SCRIPT_NAME="/path/to/env_wagi.wasm"
SERVER_SOFTWARE="WAGI/1"
SERVER_PORT="80"
SERVER_NAME="localhost:3000"
AUTH_TYPE=""
REMOTE_ADDR="127.0.0.1"
REMOTE_HOST="127.0.0.1"
PATH_INFO="/envwasm"
QUERY_STRING=""
PATH_TRANSLATED="/envwasm"
CONTENT_LENGTH="0"
HTTP_HOST="localhost:3000"
GATEWAY_INTERFACE="CGI/1.1"
REMOTE_USER=""
X_FULL_URL="http://localhost:3000/envwasm"
```

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

## Contributing

We hang out in the [#krustlet](https://kubernetes.slack.com/messages/krustlet) channel of the [Kubernetes Slack](https://kubernetes.slack.com).
If WAGI gains popularity, we'll create a dedicated channel (probably on a more fitting Slack server).

WAGI is experimental, and we welcome contributions to improve the project.
In fact, we're delighted that you're even reading this section of the docs!

For bug fixes:

- Please, pretty please, file issues for anything you find. This is a new project, and we are SURE there are some bugs to work out.
- If you want to write some code, feel free to open PRs to fix issues. You may want to drop a comment on the issue to let us know you're working on it (so we don't duplicate effort).

For refactors and tests:

- We welcome any changes to improve performance, clean up code, add tests, etc.
- For style/idiom guidelines, we follow the same conventions as [Krustlet](https://github.com/deislabs/krustlet)

For features:

- If there is already an issue for that feature, please let us know in the comments if you plan on working on it.
- If you have a new idea, file an issue describing it, and we will happily discuss it.

Since this is an experimental repository, we might be a little slow to answer.

## Code of Conduct

This project has adopted the [Microsoft Open Source Code of
Conduct](https://opensource.microsoft.com/codeofconduct/).

For more information see the [Code of Conduct
FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or contact
[opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.
