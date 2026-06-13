use tork::testing::TestClient;
use tork::{api_model, get, App, AsyncApi, OpenApi};

#[api_model]
struct Health {
    ok: bool,
}

#[get("/health")]
async fn health() -> tork::Result<Health> {
    Ok(Health { ok: true })
}

#[get("/events")]
async fn events() -> tork::Result<Health> {
    Ok(Health { ok: true })
}

#[tokio::test]
async fn docs_and_spec_routes_are_served_through_the_framework_pipeline() {
    let client = TestClient::new(
        App::new()
            .include(health)
            .include(events)
            .openapi(
                OpenApi::new()
                    .title("Tork API")
                    .version("1.0.0")
                    .json("/schema.json")
                    .docs("/docs"),
            )
            .asyncapi(AsyncApi::new().json("/events.json"))
            .build_test()
            .await
            .unwrap(),
    )
    .await
    .unwrap();

    let schema = client.get("/schema.json").send().await.unwrap();
    assert_eq!(schema.status(), 200);
    assert!(schema.text().unwrap().contains("\"openapi\":\"3.1.0\""));

    let docs = client.get("/docs").send().await.unwrap();
    assert_eq!(docs.status(), 200);
    let docs_html = docs.text().unwrap();
    assert!(docs_html.contains("api-reference"));
    assert!(docs_html.contains("/schema.json"));

    let asyncapi = client.get("/events.json").send().await.unwrap();
    assert_eq!(asyncapi.status(), 200);
    assert!(asyncapi.text().unwrap().contains("\"asyncapi\":\"3.0.0\""));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn protect_gates_the_docs_and_spec_routes() {
    let client = TestClient::new(
        App::new()
            .include(health)
            .openapi(
                OpenApi::new()
                    .json("/openapi.json")
                    .docs("/docs")
                    .protect(|ctx| {
                        ctx.headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            == Some("Bearer docs-token")
                    }),
            )
            .build_test()
            .await
            .unwrap(),
    )
    .await
    .unwrap();

    // Without the token both routes are hidden behind a 404.
    assert_eq!(client.get("/openapi.json").send().await.unwrap().status(), 404);
    assert_eq!(client.get("/docs").send().await.unwrap().status(), 404);

    // With the token they are served normally.
    let spec = client
        .get("/openapi.json")
        .header("authorization", "Bearer docs-token")
        .send()
        .await
        .unwrap();
    assert_eq!(spec.status(), 200);

    let docs = client
        .get("/docs")
        .header("authorization", "Bearer docs-token")
        .send()
        .await
        .unwrap();
    assert_eq!(docs.status(), 200);

    client.shutdown().await.unwrap();
}
