# Getting Started with WAGI

This guide covers configuring and running the WAGI server, as well as loading a WebAssembly module.
It assumes you have already [installed](installation.md) WAGI.

This guide begins with starting the WAGI server, then covers the `modules.toml` and `cache.toml` configuration files.

## Running WAGI

The `wagi` server is run from the command line. It has a few flags:

- `-c`|`--config`: The path to a `modules.toml` configuration
- `-b`|`--bindle`: The name of a bindle to use for configuration, e.g. `-b example.com/hello/1.0.0`. 
  - You *must* specify _one of_ `--config` or `--bindle`.
  - If you specify both, it will use the `--bindle`
- `--bindle-server`: The full URL to a Bindle server. Default is `http://localhost:8080/v1`
- `--cache`: The path to an optional `cache.toml` configuration file (see the caching section below)
- `--default-host`: The hostname (with port) to use when no HOST header is provided. Default is `localhost:3000`
- `-l`|`--listen`: The IP address and port to listen on. Default is `127.0.0.1:3000`
- `--module-cache`: The location to write cached binary Wasm modules. Default is a tempdir.

At minimum, to start WAGI, run a command that looks like this:

```console
$ wagi -c examples/modules.toml
=> Starting server
(load_routes) instantiation time for module examples/hello.wat: 101.840297ms
(load_routes) instantiation time for module examples/hello.wasm: 680.518671ms
```

If you would prefer to load the application from a bindle, use the bindle name:
```console
$ export BINDLE_SERVER_URL=http://localhost:8080/v1
$ wagi -b example.com/hello/1.0.0
=> Starting server
(load_routes) instantiation time for module examples/hello.wat: 101.840297ms
(load_routes) instantiation time for module examples/hello.wasm: 680.518671ms
```

To start from source, use `cargo run -- -c examples/modules.toml` or `make run`.

Next we cover the `modules.toml` format, followed by the Bindle format.

## The `modules.toml` Configuration File

WAGI requires a TOML-formatted configuration file that details which modules should be loaded.
By convention, this file is called `modules.toml`.

In a nutshell, these are the fields that `modules.toml` supports.

- The `[[module]]` list: Each module starts with a `[[module]]` header. Inside of a module, the following fields are available:
  - `route` (REQUIRED): The path that is appended to the server URL to create a full URL (e.g. `/foo` becomes `https://example.com/foo`)
  - `module` (REQUIRED): A module reference. See Module References below.
  - `environment`: A list of string/string environment variable pairs.
  - `repository`: RESERVED for future use
  - `entrypoint` (default: `_start`): The name of the function within the module. This will directly execute that function. Most WASM/WASI implementations create a `_start` function by default. An example of a module that declares 3 entrypoints can be found [here](https://github.com/technosophos/hello-wagi).
  
Here is a brief example of a `modules.toml` file that declares two routes:

```toml
[[module]]
# Example executing a Web Assembly Text file
route = "/"
module = "examples/hello.wat"

[[module]]
# Example running a WASM file.
route = "/hello/..."
module = "examples/hello.wasm"
```

Each `[[module]]` section in the `modules.toml` file is responsible for mapping a route (the path part of a URL) to an executable piece of code.

The two required directives for a module section are:

- `route`: The path-portion of a URL
- `module`: A reference to the WebAssembly module to execute

Routes are paths relative to the WAGI HTTP root. Assuming the routes above are running on a server whose domain is `example.com`:

- The `/` route handles traffic to `http://example.com/` (or `https://example.com/`)
- A route like `/hello` would handle traffic to `http://example.com/hello`
- The route `/hello/...` is a special wildcard route that handles any traffic to `http://example.com/hello` or a subpath (like `http://example.com/hello/today/is/a/good/day`)

### Module References

A module reference is a URL. There are three supported module reference schemes:

- `file://`: A path to a `.wasm` or `.wat` file on the filesystem. We recommend using absolute paths beginning with `file://`. Right now, there is legacy support for absolute and relative paths without the `file://` prefix (note that this is not working on Windows with absolute paths), but we discourage using that. Relative paths will be resolved from the current working directory in which `wagi` was started.
- `bindle:`: DEPRECATED: A reference to a Bindle. This will be looked up in the configured Bindle server. Example: `bindle:example.com/foo/bar/1.2.3`. Bindle URLs do not ever have a `//` after `bindle:`.
- `oci`: A reference to an OCI image in an OCI registry. Example: `oci:foo/bar:1.2.3` (equivalent to the Docker image `foo/bar:1.2.3`). OCI URLs should not need `//` after `oci://`.

#### Volume Mounting

In addition to the required directives, the `[[module]]` sections support several other directives.
One of these is the `volume` directive, which mounts a local directory into the module.

By default, Wasm modules in WAGI have no ability to access the host filesystem.
That is, a Wasm module cannot open `/etc/` and read the files there, even if the `wagi` server has access to `/etc/`.
In WAGI, modules are considered untrusted when it comes to accessing resources on the host.
But it is definitely the case that code sometimes needs access to files.
For that reason, WAGI provides the `volumes` directive.

Here is an example of providing a volume:

```toml
[[module]]
route = "/bar"
module = "/path/to/bar.wasm"
# You can give WAGI access to particular directories on the filesystem.
volumes = {"/path/inside/wasm" = "/path/on/host"}
```

In this case, the `volumes` directive tells WAGI to expose the contents of `/path/on/host` to the `bar.wasm` module.
But `bar.wasm` will see that directory as `/path/inside/wasm`. Importantly, it will not be able to access any other parts of the filesystem. Fo example, it will not see anything on the path `/path/inside`. It _only_ has access to the paths specified
in the `volumes` directive.

#### Environment Variables

Similarly to volumes, by default a WebAssembly module cannot access the host's environment variables.
However, WAGI provides a way for you to pass in environment variables:

```toml
[[module]]
route = "/hello"
module = "/path/to/hello.wasm"
# You can put static environment variables in the TOML file
environment.TEST_NAME = "test value"
```

In this case, the environment variable `TEST_NAME` will be set to `test value` for the `hello.wasm` module.
When the module starts up, it will be able to access the `TEST_NAME` variable.

Note that while the module will not be able to access the host environment variables, WAGI does provide a wealth of other environment variables. See [Environment Variables](environment_variables.toml) for details.

#### Entrypoint

By default, a WASM WASI module has a function called `_start()`.
Usually, this function is created at compilation time, and typically it just calls the `main()`
function (this is a detail specific to the language in which the code was written).

Sometimes, though, you may want to have WAGI invoke another function.
This is what the `entrypoint` directive is for.

The following example shows loading the same module at three different paths, each time
invoking a different function:

```toml
# With no `entrypoint`, this will invoke `_start()`
[[module]]
route = "/hello"
module = "/path/to/bar.wasm"

[[module]]
route = "/entrypoint/hello"
module = "/path/to/bar.wasm"
entrypoint = "hello"  # Executes the `hello()` function in the module (instead of `_start`)

[[module]]
route = "/entrypoint/goodbye"
module = "/path/to/bar.wasm"
entrypoint = "goodbye  # Executes the `goodbye()` function in the module (instead of `_start`)
```

### A Large Example

Here is an example `modules.toml` that exercises the features discussed above:

```toml
[[module]]
route = "/"
module = "/absolute/path/to/root.wasm"

[[module]]
route = "/foo"
module = "/path/to/foo.wasm"

[[module]]
# The "/..." suffix means this will match /bar and its subpaths, like /bar/a/b/c
route = "/bar/..."
module = "/path/to/bar.wasm"
# You can give WAGI access to particular directories on the filesystem.
volumes = {"/path/inside/wasm" = "/path/on/host"}
# You can also put static environment variables in the TOML file
environment.TEST_NAME = "test value" 

[[module]]
# You can also execute a WAT file directly
route = "/hello"
module = "/path/to/hello.wat"


# You can declare custom handler methods as 'entrypoints' to the module.
# Here we have two module entries that use the same module, but call into
# different entrypoints.
[[module]]
route = "/entrypoint/hello"
module = "/path/to/bar.wasm"
entrypoint = "hello"  # Executes the `hello()` function in the module (instead of `_start`)

[[module]]
route = "/entrypoint/goodbye"
module = "/path/to/bar.wasm"
entrypoint = "goodbye  # Executes the `goodbye()` function in the module (instead of `_start`)
```

## Using a Bindle Instead of a `modules.toml`

Instead of using a `modules.toml`, it is possible to directly use a bindle.
To do this, you will need to configure the following:

- You will need access to a Bindle server. See the [Bindle project](https://github.com/deislabs/bindle) for instructions.
- You will need to set the environment variable `BINDLE_SERVER_URL`
  - The default value is `http://localhost:8080/v1`
  - The version identifier is required. You cannot omit `/v1`
- You will need a bindle that has your app. We cover this below.
- When starting up `wagi`, use the `--bindle` argument to specify the bindle that holds your app

```console
$ export BINDLE_SERVER_URL="http://localhost:8080/v1"
$ wagi -b example.com/hello/1.3.3
```

### Building a Bindle for Wagi

In the event that a Bindle is used, the Bindle will construct a module configuration according
to the following rules:

- Every parcel in the global group (aka the default group) that has the media type `application/wasm` will be mounted to a path.
- The parcel should be annotated with the `feature.wagi.route = "SOME PATH"` to declare the path.

A parcel may require a group of supporting parcels. Supporting parcels are evaluated as follows:
- Any supporting parcel that is marked `feature.wagi.file = "true"` will be mounted as a file, using the `lable.name` as the relative path.

A supporting file MUST be be a member of a group, and that group MUST be required by a module before that module will be given access to the file.

### Wagi Features in a Parcel

The following features are available for Wagi under `feature.wagi.FEATURE`:

| Feature | Description |
| --- | --- |
| entrypoint | The name of the entrypoint function |
| bindle_server | RESERVED (to prevent using a deprecated feature) |
| route | The relative path from the server route. e.g. "/foo" is mapped to http://example.com/foo |
| allowed_hosts | A comma-separated list of hosts that the HTTP client is allowed to access |
| file | If this is "true", this parcel will be treated as a file for consumption by a Wagi module |

### Simple Bindle Example

This example can be found in `examples/invoice.toml` in the Wagi source code.

```toml
bindleVersion = "1.0.0"

[bindle]
name = "example.com/hello"
version = "1.0.0"
description = "Autogenerated example bindle for Wagi"

[[parcel]]
[parcel.label]
name = "examples/hello.wasm"
mediaType = "application/wasm"
size = 165
sha256 = "1f2bc60e4e39297d9a3fd06b789f6f00fac4272d72da6bf5dae20fb5f32d45a4"
[parcel.label.feature.wagi]
route = "/"
```

The above declares a bindle named `example.com/hello/1.0.0`.
It references one module: `examples/hello.wasm`, which you can find in the `examples/hello.wasm` file in the source code.
This module is mounted to the route `/`, which means it will be executable at `http://localhost:3000/`.

The `examples/mkbindle.rs` program can load the invoice into a bindle server for you.
To run it, use `cargo run --example mkbindle`.

```console
$ cargo run --example mkbindle
    Finished dev [unoptimized + debuginfo] target(s) in 0.70s
     Running `target/debug/examples/mkbindle`
You can now use example.com/hello/1.0.0
```

Feel free to edit the code in that example and see what it does.
Remember that you must change a bindle's version each time you send it to the Bindle server.

### Advanced Bindle Example

This invoice defines a bindle with two Wasm modules and two other parcels.
The `static.wasm` file will have one file mounted to it.
A number of Bindle fields have been omitted for readability.

```toml
bindle_version = "1.0.0"

[[group]]
name = "files"

[[parcel]]
[parcel.label]
name = "examples/hello.wasm"
mediaType = "application/wasm" # Wagi will only run application/wasm modules
size = 165
sha256 = "1f2bc60e4e392..."
[parcel.label.feature.wagi]
route = "/"    # This will be http://example.com/

[[parcel]]
[parcel.label]
name = "static.wasm"
mediaType = "application/wasm"
size = 155
sha256  = "aaaaa..."
[parcel.label.feature.wagi]
route = "/static/..." # This will be http://example.com/static/*
[parcel.conditions]
requires = ["files"]  # This will cause Wagi to load the files group for this module

[[parcel]]
[parcel.label]
name = "image.jpeg"        # The name of the file
mediaType = "image/jpeg"
size = 12345
sha256  = "aaaaa..."
[parcel.label.feature.wagi]
file = "true"              # Mark this as a file
[parcel.conditions]
member_of = ["files"]      # Add it to the "files" group

# Nothing is done with this, since no features tell Wagi what to do
[[parcel]]
[parcel.label]
name = "another.jpeg"        # The name of the file
mediaType = "image/jpeg"
size = 12345
sha256  = "aaaaa..."
[parcel.conditions]
member_of = ["files"]       # Add it to the "files" group
```

When Wagi loads the invoice above, it will create two routes: `/` and `/static/...`.
The main module (`hello.wasm`) will listen on `/`.
The static module (`static.wasm`) will listen on any subpath of `/static`.
It will also have access to the file `/image.jpeg` on its virtual file system.
Note that because `another.jpeg` was not marked as a `feature.wagi.file`, it is not mounted as a file.

## Enabling Caching

To enable the [Wasmtime cache](https://docs.wasmtime.dev/cli-cache.html), which caches the result of the compilation
of a WebAssembly module, resulting in improved instantiation times for modules, you must create a `cache.toml` file
with the following structure, and point the WAGI binary to it:

```toml
[cache]
enabled = true
directory = "<absolute-path-to-a-cache-directory>"
# optional
# see more details at https://docs.wasmtime.dev/cli-cache.html
cleanup-interval = "1d"
files-total-size-soft-limit = "10Gi"
```

To start WAGI with caching enabled, use the `--cache` flag.
For example, `cargo run -- --config path/to/modules.toml --cache path/to/cache.toml`.

The WAGI server now prints the module instantiation time, so you can choose whether caching helps for your modules.

## What's Next?

Next, read about [Writing Modules](writing_modules.md) for WAGI.