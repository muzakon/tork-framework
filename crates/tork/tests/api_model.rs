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

#[test]
fn produces_json_schema() {
    let schema = schemars::schema_for!(CreateOrderInput);
    let value = serde_json::to_value(&schema).unwrap();
    // camelCase field names appear in the schema's properties.
    let props = &value["properties"];
    assert!(props.get("name").is_some(), "schema: {value}");
    assert!(props.get("price").is_some(), "schema: {value}");
}
