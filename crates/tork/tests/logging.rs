//! Integration tests for the logging system.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use tork::testing::{LogRecorder, TestClient};
use tork::{
    App, FromRequest, Inject, Logger, PathParams, RequestContext, Router, StateMap, assert_logs,
    box_body, get,
};

#[derive(Clone, Inject)]
#[allow(dead_code)]
struct OrderService {
    logger: Logger,
}

#[derive(Clone, Inject)]
#[inject(context = "Payments")]
#[allow(dead_code)]
struct PaymentService {
    logger: Logger,
}

#[derive(Clone, Inject)]
#[allow(dead_code)]
struct CustomService {
    #[logger(context = "Custom")]
    logger: Logger,
}

fn context() -> RequestContext {
    let head = http::Request::builder()
        .header("x-request-id", "req-123")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    RequestContext::new(
        head,
        PathParams::new(),
        Arc::new(StateMap::new()),
        box_body(Full::new(Bytes::new())),
    )
}

#[tokio::test]
async fn inject_uses_struct_name_as_context() {
    let service = OrderService::from_request(&context()).await.unwrap();
    assert_eq!(service.logger.context(), "OrderService");
}

#[tokio::test]
async fn inject_container_attribute_overrides_context() {
    let service = PaymentService::from_request(&context()).await.unwrap();
    assert_eq!(service.logger.context(), "Payments");
}

#[tokio::test]
async fn inject_field_attribute_overrides_context() {
    let service = CustomService::from_request(&context()).await.unwrap();
    assert_eq!(service.logger.context(), "Custom");
}

#[derive(Clone, Inject)]
struct Greeter {
    logger: Logger,
}

impl Greeter {
    fn greet(&self) {
        self.logger.info("Greeting the world").field("who", "world").emit();
    }
}

#[get("/greet")]
async fn greet(service: Greeter) -> tork::Result<String> {
    service.greet();
    Ok("ok".to_owned())
}

#[tokio::test]
async fn recorder_captures_service_logs() {
    let recorder = LogRecorder::new();
    let client = TestClient::builder(App::new().include_router(Router::new().route(greet())))
        .logger(recorder.clone())
        .build()
        .await
        .unwrap();

    let response = client.get("/greet").send().await.unwrap();
    assert_eq!(response.status(), 200);

    // The service's log was captured with its struct-name context.
    assert!(recorder.contains_context("Greeter"));
    assert!(recorder.contains_message("Greeting the world"));
    assert_logs!(recorder, context = "Greeter", message = "Greeting");

    // The automatic HTTP request log is captured too.
    assert!(recorder.contains_context("HTTP"));
}
