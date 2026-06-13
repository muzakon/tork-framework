//! Integration tests for the `#[settings]` macro: loading, defaults, nesting,
//! secrets, validation, and injection as a resource.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tork::{
    api_model, box_body, get, settings, App, Method, ReqBody, Resources, Router, SecretString,
    StatusCode,
};

#[settings(prefix = "TORKAPP")]
struct DatabaseConfig {
    #[setting(default = 5)]
    max_connections: u32,
    url: String,
}

#[settings(prefix = "TORKAPP")]
struct AppConfig {
    #[setting(default = "Awesome API")]
    app_name: String,
    #[setting(default = 50, ge = 1, le = 500)]
    items_per_user: u32,
    #[setting(nested)]
    database: DatabaseConfig,
    #[setting(secret)]
    api_key: SecretString,
}

// A container so the Resources derive generates `FromRequest for Arc<AppConfig>`.
#[derive(Clone, Resources)]
#[allow(dead_code)]
struct State {
    #[resource]
    config: Arc<AppConfig>,
}

#[api_model(rename_all = "camelCase")]
struct InfoOut {
    app_name: String,
    items_per_user: u32,
    max_connections: u32,
    api_key_masked: String,
    api_key_exposed: String,
}

#[get("/info")]
async fn info(config: Arc<AppConfig>) -> tork::Result<InfoOut> {
    Ok(InfoOut {
        app_name: config.app_name.clone(),
        items_per_user: config.items_per_user,
        max_connections: config.database.max_connections,
        api_key_masked: format!("{}", config.api_key),
        api_key_exposed: config.api_key.expose().to_owned(),
    })
}

#[tokio::test]
async fn settings_load_nest_secret_and_inject() {
    std::env::set_var("TORKAPP_ITEMS_PER_USER", "120");
    std::env::set_var("TORKAPP_DATABASE__URL", "postgres://localhost/app");
    std::env::set_var("TORKAPP_DATABASE__MAX_CONNECTIONS", "20");
    std::env::set_var("TORKAPP_API_KEY", "top-secret");

    let config = AppConfig::load().expect("config should load");

    // Defaults, env values, and nested env values all resolve.
    assert_eq!(config.app_name, "Awesome API"); // default
    assert_eq!(config.items_per_user, 120); // env
    assert_eq!(config.database.url, "postgres://localhost/app"); // nested env
    assert_eq!(config.database.max_connections, 20); // nested env over default

    // The secret is masked but readable through expose().
    assert_eq!(format!("{}", config.api_key), "********");
    assert_eq!(config.api_key.expose(), "top-secret");

    let app = App::new()
        .state(Arc::new(config))
        .include_router(Router::new().route(__tork_route_info()))
        .build()
        .unwrap();

    let request: http::Request<ReqBody> = http::Request::builder()
        .method(Method::GET)
        .uri("/info")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app.dispatch(request).await;
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["appName"], "Awesome API");
    assert_eq!(json["itemsPerUser"], 120);
    assert_eq!(json["maxConnections"], 20);
    assert_eq!(json["apiKeyMasked"], "********");
    assert_eq!(json["apiKeyExposed"], "top-secret");

    for key in [
        "ITEMS_PER_USER",
        "DATABASE__URL",
        "DATABASE__MAX_CONNECTIONS",
        "API_KEY",
    ] {
        std::env::remove_var(format!("TORKAPP_{key}"));
    }
}

#[settings(prefix = "TORKSTRICT")]
#[allow(dead_code)]
struct StrictConfig {
    #[setting(default = 50, ge = 1, le = 500)]
    items_per_user: u32,
    url: String,
}

#[tokio::test]
async fn settings_validation_fails_at_load() {
    std::env::set_var("TORKSTRICT_ITEMS_PER_USER", "9999"); // exceeds le = 500
    std::env::set_var("TORKSTRICT_URL", "http://localhost");

    let error = StrictConfig::load().unwrap_err();
    assert_eq!(error.code(), "CONFIG_VALIDATION_ERROR");

    std::env::remove_var("TORKSTRICT_ITEMS_PER_USER");
    std::env::remove_var("TORKSTRICT_URL");
}
