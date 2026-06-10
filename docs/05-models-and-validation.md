# 5. Models and validation

`#[api_model]` turns a struct into a request or response model. It derives
serialization, validation, and JSON Schema in one place, and translates concise
field constraints into the underlying rules.

## Declaring a model

```rust
use tork::api_model;

#[api_model(rename_all = "camelCase")]
pub struct CreateOrderInput {
    #[field(min_length = 1, max_length = 120)]
    pub name: String,

    #[field(max_length = 300, title = "The description of the item")]
    pub description: Option<String>,

    #[field(gt = 0, description = "The price must be greater than zero")]
    pub price: f64,

    #[field(ge = 0)]
    pub tax: Option<f64>,
}
```

The macro derives `Debug`, `Clone`, serialization, validation, and JSON Schema.
`rename_all = "camelCase"` applies to the serialized field names and the schema.

## Field constraints

`#[field(...)]` accepts:

- `min_length` / `max_length`: string length bounds.
- `ge` / `le`: inclusive numeric bounds.
- `gt` / `lt`: exclusive numeric bounds.
- `title` / `description`: documentation metadata.
- `custom = path`: a custom validator function (may be repeated).
- `nested`: validate a nested model or collection recursively.
- `default`: use the type's `Default` when the field is absent.

A field with no constraints is allowed; it simply has no rules. `Option<T>` fields
are optional during deserialization (an absent field becomes `None`).

## Validating a request body

Receive a model with the `Valid<T>` extractor. It parses the JSON body and runs
validation before the handler sees it:

```rust
use tork::{Valid, post};

use crate::models::order::{CreateOrderInput, OrderOut};

#[post("/", response_model = OrderOut, status_code = 201, summary = "Create an order")]
pub async fn create_order(
    user_id: i64,
    body: Valid<CreateOrderInput>,
    service: OrderService,
) -> tork::Result<OrderOut> {
    service.create_order(user_id, body.into_inner()).await
}
```

A body that fails validation produces `422 Unprocessable Entity` with one entry
per offending field. For example, posting `{"name":"","price":0}` returns:

```json
{
  "status": 422,
  "code": "VALIDATION_ERROR",
  "title": "Unprocessable Entity",
  "message": "The submitted data failed validation.",
  "details": [
    { "field": "name", "issue": "TOO_SHORT", "message": "length is lower than 1" },
    { "field": "price", "issue": "TOO_SMALL", "message": "must be greater than 0" }
  ],
  "traceId": "req-...",
  "timestamp": "2026-06-10T11:08:07Z"
}
```

## Custom validators

For a rule that the built-in constraints do not cover, write a function and point
`custom` at it. The function performs the check and supplies its own message:

```rust
use tork::api_model;

fn not_blank(value: &str, _ctx: &()) -> garde::Result {
    if value.trim().is_empty() {
        Err(garde::Error::new("must not be blank"))
    } else {
        Ok(())
    }
}

#[api_model]
pub struct CreateTag {
    #[field(min_length = 1, custom = not_blank)]
    pub label: String,
}
```

## Nested models

A field that is itself a model is serialized and documented automatically. To also
validate it recursively, mark it `nested`:

```rust
use tork::api_model;

#[api_model]
pub struct Image {
    #[field(min_length = 1)]
    pub url: String,
    pub name: String,
}

#[api_model(rename_all = "camelCase")]
pub struct Item {
    #[field(min_length = 1)]
    pub name: String,

    pub price: f64,

    #[field(default)]
    pub tags: std::collections::HashSet<String>,

    #[field(nested)]
    pub image: Option<Image>,
}
```

Validating an `Item` now also validates the nested `Image`. The `tags` field uses
`default`, so an absent `tags` becomes an empty set.
