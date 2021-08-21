# The Environment Variables

Every time WAGI processes a request, it starts the module and injects environment variables.

These are the environment variables set on every WAGI requests:

```bash
# The name of the route that matched
X_MATCHED_ROUTE="/envwasm/..."  # for example.com/envwasm
# The value of the HTTP Accept header from the client. This could be empty.
HTTP_ACCEPT="*/*"
# The HTTP method (GET/POST/PUT/etc) sent by the client
REQUEST_METHOD="GET"
# The protocol that the server is using. Usually it is HTTP/1.1
SERVER_PROTOCOL="HTTP/1.1"
# The value of the HTTP User-Agent header. This could be empty.
HTTP_USER_AGENT="curl/7.64.1"
# If the client sent a body (in a POST/PUT), the value of the client's
# Content-Type header is here. This could be empty, even on a POST/PUT/PATCH.
CONTENT_TYPE=""             # Usually set on POST/PUT
# The URL path portion that goes to the top level of the script.
# Note that the /... is not present here, though it is on X_MATCHED_ROUTE
SCRIPT_NAME="/envwasm"
# The name of the server software and it's MAJOR version.
SERVER_SOFTWARE="WAGI/1"
# The port upon which the server received its request
SERVER_PORT="3000"
# The host and port that the server answered to. This does not contain the port.
SERVER_NAME="localhost"
# The auth type (e.g. basic/digest)
AUTH_TYPE=""
# The client's IP address
REMOTE_ADDR="127.0.0.1"
# The server's IP address
REMOTE_HOST="127.0.0.1"
# The path info after the SCRIPT_NAME. If the route is /envwasm/... and the 
# request is /envwasm/foo, the PathInfo is /foo
PATH_INFO="/foo"
# The client-supplied query string, E.g. http://example.com?foo=bar becomes foo=bar
QUERY_STRING=""
# This is PATH_INFO after it has been run through a url-decode
PATH_TRANSLATED="/foo"
# The length of the body sent by the client. This is >0 only if the client sends a
# non-empty body.
CONTENT_LENGTH="0"
# The value of the client-supplied HOST header.
HTTP_HOST="localhost:3000"
# The version of CGI that this gateway implements. Wagi always returns CGI/1.1
GATEWAY_INTERFACE="CGI/1.1"
# If authentication was performed by Wagi and a username is discernable from that
# authentication, then this value is set to the username.
REMOTE_USER=""
# The entire URL that the client sent. This is reconstructed, so parts of the URL
# may have been normalized out. For example, if the client sends
# http://localhost:3000/foo/../envwasm, it will be normalized to
# http://localhost:3000/envwasm.
X_FULL_URL="http://localhost:3000/envwasm"
```

In addition, any values set at the command line with `--env` or `--env-file` will be loaded into all modules as well.