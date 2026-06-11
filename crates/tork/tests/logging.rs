//! Integration tests for the logging system.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use tork::{FromRequest, Inject, Logger, PathParams, RequestContext, StateMap, box_body};

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
