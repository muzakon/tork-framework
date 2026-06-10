//! Integration tests for `#[derive(Resources)]` and `#[derive(Inject)]`.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tork::{App, Inject, Method, ReqBody, Resources, Router, StateMap, StatusCode, box_body, get};

#[derive(Clone)]
struct Db(i64);

#[derive(Clone, Resources)]
#[allow(dead_code)]
struct Env {
    #[resource]
    db: Db,
}

// A service whose field is a resource; resolved by injection.
#[derive(Inject)]
struct Counter {
    db: Db,
}

#[get("/resource")]
async fn read_resource(db: Db) -> tork::Result<i64> {
    Ok(db.0)
}

#[get("/service")]
async fn read_service(counter: Counter) -> tork::Result<i64> {
    Ok(counter.db.0)
}

fn request(uri: &str) -> http::Request<ReqBody> {
    http::Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(box_body(Full::new(Bytes::new())))
        .unwrap()
}

async fn body_i64(app: &std::sync::Arc<tork::AppInner>, uri: &str) -> i64 {
    let response = app.clone().handle(request(uri)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap().parse().unwrap()
}

#[test]
fn resources_register_into_the_registry() {
    let mut registry = StateMap::new();
    Env { db: Db(7) }.register(&mut registry);
    assert_eq!(registry.get::<Db>().map(|db| db.0), Some(7));
}

#[tokio::test]
async fn resource_and_service_inject_by_type() {
    let app = std::sync::Arc::new(
        App::new()
            .state(Db(9))
            .include_router(
                Router::new()
                    .route(__tork_route_read_resource())
                    .route(__tork_route_read_service()),
            )
            .build()
            .unwrap(),
    );

    assert_eq!(body_i64(&app, "/resource").await, 9);
    assert_eq!(body_i64(&app, "/service").await, 9);
}
