# The Environment Variables

Every time WAGI processes a request, it starts the module and injects environment variables.

These are the environment variables set on every WAGI requests:

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

In addition, if a `[[module]]` section that matches the route also declares `environment` variables, those will be added.