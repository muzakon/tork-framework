# 7. OpenAPI and docs

Tork generates an OpenAPI document from your routes and models, and serves an
interactive documentation page.

## Enabling it

Configure OpenAPI on the `App` with the `OpenApi` builder:

```rust
use tork::{App, OpenApi};

App::new()
    .state(state)
    .include_router(users::router())
    .openapi(
        OpenApi::new()
            .title("My API")
            .version("1.0.0")
            .json("/openapi.json")
            .docs("/docs"),
    )
    .serve("0.0.0.0:8000")
    .await
```

- `title` and `version` set the document metadata; `description` is also available.
- `json("/openapi.json")` serves the specification document at that path.
- `docs("/docs")` serves a documentation page at that path.

## What the document contains

For each route, the document records the method, path, summary, description, tags,
and path parameters. When a handler declares a request body (a `Valid<T>` or
`Json<T>` parameter) or a response model, the corresponding schema is included.

Schemas come from `#[api_model]` types. Each model is registered once under
`components.schemas`, and request and response bodies reference it. Nested models
are registered too, and referenced by `$ref`. For this to work, request and
response types must implement JSON Schema, which `#[api_model]` provides.

A request to `/openapi.json` returns a document like:

```json
{
  "openapi": "3.1.0",
  "info": { "title": "My API", "version": "1.0.0" },
  "paths": {
    "/users/{user_id}/orders": {
      "post": {
        "summary": "Create an order for a user",
        "tags": ["orders", "users"],
        "parameters": [
          { "name": "user_id", "in": "path", "required": true, "schema": { "type": "string" } }
        ],
        "requestBody": {
          "required": true,
          "content": { "application/json": { "schema": { "$ref": "#/components/schemas/CreateOrderInput" } } }
        },
        "responses": {
          "201": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/OrderOut" } } } }
        }
      }
    }
  },
  "components": { "schemas": { "CreateOrderInput": { }, "OrderOut": { } } }
}
```

## The documentation page

`docs("/docs")` serves an HTML page that renders the document with the Scalar API
reference. Open `http://127.0.0.1:8000/docs` in a browser to read and try the API.

OpenAPI support is behind a default-on `openapi` feature. If you do not need it,
disable default features to drop the dependency.
