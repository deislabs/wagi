## Test data

### `standalone-bindles` directory

* 28e62...: ID `itowlson/toast-on-demand/0.1.0-ivan-20210924170616069`
  - WASM module that returns a HTML page with static text, two `img` tags and a list of EVs
  - WASM module for `wagi-fileserver`
  - Two image parcels for testing assets
  - Two exact routes and one wildcard route for the HTML page
  - One wildcard route for the fileserver
* 8b90d...: ID `print-env/0.1.0`
  - Single WASM module, no assets
  - Responds to `/` and `/test/...`
  - Responds with a sorted, plain text list of environment variables in format `k = v`
* 3291b...: ID `dynamic-routes/0.1.0`
  - Single WASM module, no assets
  - Responds to various routes under `/`, `/exactparent` and `/wildcardparent` per the `_routes` function
  - Each endpoint responds with plain text:
    - a line of descriptive text indicating which handler was called
    - a sorted list of environment variables in format `k = v`
