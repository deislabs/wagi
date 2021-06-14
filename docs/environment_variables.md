# The Environment Variables

Every time WAGI processes a request, it starts the module and injects environment variables.

These are the environment variables set on every WAGI requests:

```bash
# The name of the route that matched
X_MATCHED_ROUTE="/envwasm"  # for example.com/envwasm
# The value of the HTTP Accept header from the client. This could be empty.
HTTP_ACCEPT="*/*"
# The HTTP method (GET/POST/PUT/etc) sent by the client
REQUEST_METHOD="GET"
# The protocol that the server is using. Normally this is "http" or "https"
SERVER_PROTOCOL="http"
# The value of the HTTP User-Agent header. This could be empty.
HTTP_USER_AGENT="curl/7.64.1"
# If the client sent a body (in a POST/PUT), the value of the client's
# Content-Type header is here. This could be empty, even on a POST/PUT/PATCH.
CONTENT_TYPE=""             # Usually set on POST/PUT
# The name of the module requested. In a Bindle, this is the parcel name.
SCRIPT_NAME="/path/to/env_wagi.wasm"
# The name of the server software and it's MAJOR version.
SERVER_SOFTWARE="WAGI/1"
# The port upon which the server received its request
SERVER_PORT="3000"
# The host and port that the server answered to. This usually matches the HOST
# header.
SERVER_NAME="localhost:3000"
# The auth type (e.g. basic/digest)
AUTH_TYPE=""
# The client's IP address
REMOTE_ADDR="127.0.0.1"
# The server's IP address
REMOTE_HOST="127.0.0.1"
# The path portion of the URL. E.g. http://example.com/envwasm becomes /envwasm
PATH_INFO="/envwasm"
# The client-supplied query string, E.g. http://example.com?foo=bar becomes ?foo=bar
QUERY_STRING=""
# Currently, this is always the same as PATH_INFO, but is supplied for compatibility with
# the CGI specification. It is not recommended that you use this variable.
PATH_TRANSLATED="/envwasm"
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
# If a route containing /... matches, this is the part that matched "...".
# For example, if the route is "/static/..." and the request comes for "/static/icon.png",
# this will contain "icon.png"
X_RELATIVE_PATH=""    
```

In addition, if a `[[module]]` section that matches the route also declares `environment` variables, those will be added.