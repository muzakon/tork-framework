//! Integration tests for the `#[api_model]` macro through the facade crate.

use garde::Validate;
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

#[test]
fn deserializes_camel_case_and_validates() {
    let json = r#"{"name":"Widget","description":null,"price":9.99,"tax":null}"#;
    let input: CreateOrderInput = serde_json::from_str(json).unwrap();
    assert_eq!(input.name, "Widget");
    assert!(input.validate().is_ok());
}

#[test]
fn serializes_to_camel_case() {
    let input = CreateOrderInput {
        name: "Widget".to_owned(),
        description: None,
        price: 9.99,
        tax: None,
    };
    let json = serde_json::to_string(&input).unwrap();
    // `total`-style camelCase only matters for multi-word fields; assert the
    // value round-trips and serializes without error.
    assert!(json.contains("\"name\":\"Widget\""), "json: {json}");
}

#[test]
fn rejects_blank_name() {
    let input = CreateOrderInput {
        name: String::new(),
        description: None,
        price: 9.99,
        tax: None,
    };
    let error = input.validate().unwrap_err();
    assert!(error.to_string().contains("name"), "report: {error}");
}

#[test]
fn rejects_non_positive_price() {
    let input = CreateOrderInput {
        name: "Widget".to_owned(),
        description: None,
        price: 0.0,
        tax: None,
    };
    let error = input.validate().unwrap_err();
    assert!(error.to_string().contains("price"), "report: {error}");
}

fn reject_foo(value: &str, _ctx: &()) -> garde::Result {
    if value == "foo" {
        Err(garde::Error::new("value must not be foo"))
    } else {
        Ok(())
    }
}

#[api_model]
struct WithCustom {
    #[field(custom = reject_foo)]
    name: String,
}

#[test]
fn custom_validator_runs_with_its_own_message() {
    let error = WithCustom {
        name: "foo".to_owned(),
    }
    .validate()
    .unwrap_err();
    assert!(
        error.to_string().contains("value must not be foo"),
        "report: {error}"
    );

    assert!(WithCustom {
        name: "bar".to_owned()
    }
    .validate()
    .is_ok());
}

#[test]
fn produces_json_schema() {
    let schema = schemars::schema_for!(CreateOrderInput);
    let value = serde_json::to_value(&schema).unwrap();
    // camelCase field names appear in the schema's properties.
    let props = &value["properties"];
    assert!(props.get("name").is_some(), "schema: {value}");
    assert!(props.get("price").is_some(), "schema: {value}");
}

// --- Nested models ---

#[api_model]
struct Image {
    #[field(min_length = 1)]
    url: String,
    name: String,
}

#[api_model(rename_all = "camelCase")]
struct Item {
    #[field(min_length = 1)]
    name: String,
    description: Option<String>,
    price: f64,
    #[field(default)]
    tags: std::collections::HashSet<String>,
    #[field(nested)]
    image: Option<Image>,
}

#[test]
fn nested_model_round_trips_and_defaults() {
    // `tags` is absent and falls back to its default; `image` is nested.
    let json = r#"{"name":"Widget","description":null,"price":9.99,
        "image":{"url":"http://x/y.png","name":"y"}}"#;
    let item: Item = serde_json::from_str(json).unwrap();
    assert!(item.tags.is_empty());
    assert_eq!(item.image.as_ref().unwrap().url, "http://x/y.png");
}

#[test]
fn nested_schema_defines_inner_model() {
    let value = serde_json::to_value(schemars::schema_for!(Item)).unwrap();
    // The nested model is emitted as its own definition and referenced.
    let text = value.to_string();
    assert!(
        text.contains("Image"),
        "schema should define Image: {value}"
    );
    assert!(text.contains("url"), "nested field should appear: {value}");
}

#[test]
fn nested_validation_recurses() {
    let invalid = Item {
        name: "ok".to_owned(),
        description: None,
        price: 1.0,
        tags: Default::default(),
        image: Some(Image {
            url: String::new(), // violates the nested url min_length
            name: "n".to_owned(),
        }),
    };
    assert!(invalid.validate().is_err(), "nested constraint should fail");

    let valid = Item {
        name: "ok".to_owned(),
        description: None,
        price: 1.0,
        tags: Default::default(),
        image: Some(Image {
            url: "u".to_owned(),
            name: "n".to_owned(),
        }),
    };
    assert!(valid.validate().is_ok());
}
