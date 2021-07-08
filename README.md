# WAGI: WebAssembly Gateway Interface

_WAGI is the easiest way to get started writing WebAssembly microservices and web apps._

**WARNING:** This is experimental code.
It is not considered production-grade by its developers, neither is it "supported" software.

> DeisLabs is experimenting with many WASM technologies right now.
> This is one of a multitude of projects (including [Krustlet](https://github.com/deislabs/krustlet))
> designed to test the limits of WebAssembly as a cloud-based runtime.

## tl;dr

WAGI allows you to run WebAssembly WASI binaries as HTTP handlers.
Write a "command line" application that prints a few headers, and compile it to `WASM32-WASI`.
Add an entry to the `modules.toml` matching URL to WASM module.
That's it.

You can use any programming language that can compile to `WASM32-WASI`.

## Quickstart

Here's the fastest way to try out WAGI.
For details, checkout out the [documentation](docs/README.md).

1. Get the [latest binary release](https://github.com/deislabs/wagi/releases)
2. Unpack it `tar -zxf wagi-VERSION-OS.tar.gz`
3. Run the `wagi --help` command

If you would like to try out a few simple configurations, we recommend cloning this repository
and then using the `examples` directory:

```console
$ wagi -c examples/modules.toml
No log_dir specified, using temporary directory /var/folders/hk/l1mlxz1x01x9yl33ll9vh9980000gp/T/.tmpx55XkJ for logs
```

This will start WAGI on `http://localhost:3000`. Use a browser or a tool like `curl` to test:

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
< content-type: text/html; charset=UTF-8
< content-length: 12
< date: Wed, 14 Oct 2020 22:00:59 GMT
<
hello world
* Connection #0 to host localhost left intact
* Closing connection 0
```

To add your own modules, compile your code to `wasm32-wasi` format and add them to the `modules.toml` file.
Check out our [Yo-Wasm](https://github.com/deislabs/yo-wasm/) project for a quick way to build Wasm modules in a variety of languages.

### Examples and Demos

Wagi is an implementation of CGI for WebAssembly.
That means that writing a Wagi module is as easy as sending properly formatted content to standard output.
If you want to understand the details, read the [Common Gateway Interface 1.1](https://tools.ietf.org/html/rfc3875) specification.

Take a look at the [Wagi Examples Repository](https://github.com/deislabs/wagi-examples) for examples in various languages.

For a "production grade" (whatever that means for a pre-release project) module, checkout out the [Wagi Fileserver](https://github.com/deislabs/wagi-fileserver): A fileserver written in Grain, compiled to Wasm, and ready to run in Wagi.

## Contributing

Want to chat?
We hang out in the [#krustlet](https://kubernetes.slack.com/messages/krustlet) channel of the [Kubernetes Slack](https://kubernetes.slack.com).

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
