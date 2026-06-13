//! Integration tests for the in-process `TestClient` (HTTP).

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use http::header::SET_COOKIE;
use http::HeaderValue;
use serde_json::json;
use tork::testing::TestClient;
use tork::{
    api_model, get, middleware, post, App, FileBytes, Form, FromRequest, Inject, Next,
    RequestContext, Resources, Response, Router, Valid,
};

#[get("/hello")]
async fn hello() -> tork::Result<serde_json::Value> {
    Ok(json!({ "msg": "Hello World" }))
}

#[api_model]
struct Item {
    #[field(min_length = 1)]
    id: String,
    name: String,
}

#[post("/items")]
async fn create_item(item: Valid<Item>) -> tork::Result<Item> {
    Ok(item.into_inner())
}

#[api_model]
struct Counter {
    value: i32,
}

struct ItemId(String);

impl FromStr for ItemId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

#[tork::put("/items/{id}")]
async fn replace_item(id: ItemId, body: Valid<Counter>) -> tork::Result<serde_json::Value> {
    Ok(json!({ "id": id.0, "value": body.into_inner().value, "method": "put" }))
}

#[tork::patch("/items/{id}")]
async fn patch_item(id: ItemId, body: Valid<Counter>) -> tork::Result<serde_json::Value> {
    Ok(json!({ "id": id.0, "value": body.into_inner().value, "method": "patch" }))
}

#[tork::delete("/items/{id}")]
async fn delete_item(id: ItemId) -> tork::Result<serde_json::Value> {
    Ok(json!({ "id": id.0, "method": "delete" }))
}

#[api_model]
struct LoginForm {
    username: String,
    password: String,
}

#[post("/login")]
async fn login(form: Form<LoginForm>) -> tork::Result<serde_json::Value> {
    let form = form.into_inner();
    Ok(json!({ "user": form.username, "len": form.password.len() }))
}

#[post("/upload")]
async fn upload(#[file] file: FileBytes, #[form] token: String) -> tork::Result<serde_json::Value> {
    Ok(json!({ "size": file.len(), "token": token }))
}

/// A handler-side view of the request headers, for cookie and default-header tests.
struct Headers(http::HeaderMap);

impl FromRequest for Headers {
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = tork::Result<Self>> + Send {
        let headers = ctx.headers().clone();
        async move { Ok(Headers(headers)) }
    }
}

#[get("/headers")]
async fn read_headers(headers: Headers) -> tork::Result<serde_json::Value> {
    let token = headers.0.get("x-token").and_then(|v| v.to_str().ok());
    let cookie = headers.0.get("cookie").and_then(|v| v.to_str().ok());
    Ok(json!({ "token": token, "cookie": cookie }))
}

#[tokio::test]
async fn include_registers_a_handler_directly() {
    // The route factory is named after the handler, so it can be passed to include.
    let app = App::new()
        .include(hello)
        .include(create_item)
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let response = client.get("/hello").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "msg": "Hello World" })
    );
}

#[tokio::test]
async fn get_json_and_post_json() {
    let app = App::new()
        .include_router(
            Router::new()
                .route(__tork_route_hello())
                .route(__tork_route_create_item()),
        )
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let response = client.get("/hello").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "msg": "Hello World" })
    );

    let response = client
        .post("/items")
        .json(&json!({ "id": "foo", "name": "Foo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let item = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(item["id"], "foo");

    // An invalid body is rejected by validation.
    let response = client
        .post("/items")
        .json(&json!({ "id": "", "name": "Foo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn post_form_and_multipart() {
    let app = App::new()
        .include_router(
            Router::new()
                .route(__tork_route_login())
                .route(__tork_route_upload()),
        )
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let response = client
        .post("/login")
        .form(&json!({ "username": "ada", "password": "secret" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["user"], "ada");
    assert_eq!(body["len"], 6);

    let response = client
        .post("/upload")
        .multipart()
        .text("token", "secret-token")
        .file_bytes("file", "a.txt", "text/plain", b"hello world".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["size"], 11);
    assert_eq!(body["token"], "secret-token");
}

#[middleware]
async fn add_cookie(req: tork::Request, next: Next) -> tork::Result<Response> {
    let mut response = next.run(req).await?;
    response
        .headers_mut()
        .insert(SET_COOKIE, HeaderValue::from_static("sid=abc123; Path=/"));
    Ok(response)
}

#[tokio::test]
async fn default_headers_and_cookie_jar() {
    let app = App::new()
        .middleware(add_cookie)
        .include_router(Router::new().route(__tork_route_read_headers()));
    let client = TestClient::builder(app)
        .default_header("X-Token", "cone-of-silence")
        .build()
        .await
        .unwrap();

    // The default header is sent with every request.
    let response = client.get("/headers").send().await.unwrap();
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["token"], "cone-of-silence");

    // The first response set a cookie; the jar replays it on the next request.
    let response = client.get("/headers").send().await.unwrap();
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["cookie"], "sid=abc123");
}

#[tokio::test]
async fn seeded_cookie_and_invalid_default_header_behavior() {
    let app = App::new().include_router(Router::new().route(__tork_route_read_headers()));
    let client = TestClient::builder(app)
        .default_header("\n", "ignored")
        .cookie("session", "seeded")
        .build()
        .await
        .unwrap();

    assert!(client.local_addr().is_none());

    let response = client.get("/headers").send().await.unwrap();
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert!(body["token"].is_null());
    assert_eq!(body["cookie"], "session=seeded");
}

#[tokio::test]
async fn sensitive_default_headers_require_explicit_opt_in() {
    let app = App::new().include_router(Router::new().route(__tork_route_read_headers()));
    let error = match TestClient::builder(app)
        .default_header("Host", "example.com")
        .build()
        .await
    {
        Ok(_) => panic!("sensitive default header must fail without opt-in"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "TEST_UNSAFE_HEADER_REQUIRES_OPT_IN");
    assert!(error.message().contains("host"));
}

#[tokio::test]
async fn unsafe_default_header_is_applied_in_process() {
    let app = App::new().include_router(Router::new().route(__tork_route_read_headers()));
    let client = TestClient::builder(app)
        .unsafe_default_header("Host", "example.com")
        .default_header("X-Token", "cone-of-silence")
        .build()
        .await
        .unwrap();

    let response = client.get("/headers").send().await.unwrap();
    assert_eq!(response.status(), 200);
}

#[derive(Clone)]
struct Greeting(String);

#[get("/greeting")]
async fn greeting(value: Arc<Greeting>) -> tork::Result<serde_json::Value> {
    Ok(json!({ "greeting": value.0 }))
}

#[tokio::test]
async fn resource_override_wins() {
    let app = App::new()
        .state(Arc::new(Greeting("base".to_owned())))
        .include_router(Router::new().route(__tork_route_greeting()));
    let client = TestClient::builder(app)
        .resource(Arc::new(Greeting("override".to_owned())))
        .build()
        .await
        .unwrap();

    let response = client.get("/greeting").send().await.unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["greeting"],
        "override"
    );
}

#[derive(Clone)]
struct SequenceState(u32);

#[derive(Clone, Inject)]
struct SequenceDependency {
    state: Arc<SequenceState>,
}

#[get("/dependency")]
async fn dependency_value(value: SequenceDependency) -> tork::Result<serde_json::Value> {
    Ok(json!({ "value": value.state.0 }))
}

#[tokio::test]
async fn builder_override_dependency_with_and_extra_http_verbs_work() {
    let next = Arc::new(std::sync::Mutex::new(0u32));
    let app = App::new().include_router(
        Router::new()
            .route(__tork_route_replace_item())
            .route(__tork_route_patch_item())
            .route(__tork_route_delete_item())
            .route(__tork_route_dependency_value()),
    );
    let client = TestClient::builder(app)
        .override_dependency_with({
            let next = next.clone();
            move || {
                let mut guard = next.lock().unwrap();
                *guard += 1;
                SequenceDependency {
                    state: Arc::new(SequenceState(*guard)),
                }
            }
        })
        .build()
        .await
        .unwrap();

    let response = client
        .put("/items/abc")
        .json(&json!({ "value": 7 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "id": "abc", "value": 7, "method": "put" })
    );

    let response = client
        .patch("/items/abc")
        .json(&json!({ "value": 8 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "id": "abc", "value": 8, "method": "patch" })
    );

    let response = client.delete("/items/abc").send().await.unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "id": "abc", "method": "delete" })
    );

    let first = client.get("/dependency").send().await.unwrap();
    let second = client.get("/dependency").send().await.unwrap();
    assert_eq!(first.json::<serde_json::Value>().await.unwrap()["value"], 1);
    assert_eq!(
        second.json::<serde_json::Value>().await.unwrap()["value"],
        2
    );
}

#[derive(Clone)]
struct Backend(String);

#[derive(Clone, Inject)]
struct Notifier {
    backend: Arc<Backend>,
}

#[get("/notify")]
async fn notify(notifier: Notifier) -> tork::Result<serde_json::Value> {
    Ok(json!({ "from": notifier.backend.0 }))
}

#[tokio::test]
async fn dependency_override_bypasses_field_resolution() {
    // No Arc<Backend> is registered, so building Notifier from its fields would
    // fail; the override supplies a pre-built instance instead.
    let app = App::new().include_router(Router::new().route(__tork_route_notify()));
    let client = TestClient::builder(app)
        .override_dependency::<Notifier>(Notifier {
            backend: Arc::new(Backend("mock".to_owned())),
        })
        .build()
        .await
        .unwrap();

    let response = client.get("/notify").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["from"],
        "mock"
    );
}

static SHUTDOWN_RAN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Resources)]
struct BootState {
    #[resource]
    greeting: Arc<Greeting>,
}

#[tork::lifespan]
impl BootState {
    async fn startup(_ctx: tork::LifespanContext) -> tork::Result<Self> {
        Ok(BootState {
            greeting: Arc::new(Greeting("from-startup".to_owned())),
        })
    }

    async fn shutdown(self) -> tork::Result<()> {
        SHUTDOWN_RAN.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn lifespan_startup_and_shutdown_run() {
    let app = App::new()
        .lifespan::<BootState>()
        .include_router(Router::new().route(__tork_route_greeting()))
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    // Startup registered the resource.
    let response = client.get("/greeting").send().await.unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["greeting"],
        "from-startup"
    );

    // Shutdown runs the lifespan teardown.
    assert!(!SHUTDOWN_RAN.load(Ordering::SeqCst));
    client.shutdown().await.unwrap();
    assert!(SHUTDOWN_RAN.load(Ordering::SeqCst));
}
