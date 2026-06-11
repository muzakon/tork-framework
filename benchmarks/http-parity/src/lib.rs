use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Json as AxumJson, Path as AxumPath, Request as AxumRequest};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self as axum_middleware, Next as AxumNext};
use axum::response::{IntoResponse as AxumIntoResponse, Response as AxumResponse};
use axum::routing::{get as axum_get, post as axum_post};
use axum::{serve as axum_serve, Router as AxumRouter};
use bytes::Bytes;
use garde::Validate;
use http::header::{HeaderName, CONTENT_TYPE};
use http_body_util::{BodyExt, Full};
use hyper::Uri;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tork::{
    api_model, get, json_response, middleware, post, App, LoggerConfig, Next, Request, Response,
    Result as TorkResult, Valid,
};

pub type BenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const BENCH_REQUEST_ID: &str = "bench-request";
const BENCH_STACK_VALUE: &str = "enabled";
const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Tork,
    Axum,
}

impl Backend {
    pub const ALL: [Self; 2] = [Self::Tork, Self::Axum];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tork => "tork",
            Self::Axum => "axum",
        }
    }
}

impl Display for Backend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "tork" => Ok(Self::Tork),
            "axum" => Ok(Self::Axum),
            _ => Err(format!("unknown backend `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    JsonOk,
    PathParam,
    JsonValidate,
    MiddlewareStack,
    TypedError,
}

impl Scenario {
    pub const ALL: [Self; 5] = [
        Self::JsonOk,
        Self::PathParam,
        Self::JsonValidate,
        Self::MiddlewareStack,
        Self::TypedError,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonOk => "json_ok",
            Self::PathParam => "path_param",
            Self::JsonValidate => "json_validate",
            Self::MiddlewareStack => "middleware_stack",
            Self::TypedError => "typed_error",
        }
    }

    pub fn method(self) -> Method {
        match self {
            Self::JsonValidate => Method::POST,
            _ => Method::GET,
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::JsonOk => "/json",
            Self::PathParam => "/users/42",
            Self::JsonValidate => "/items",
            Self::MiddlewareStack => "/middleware",
            Self::TypedError => "/db",
        }
    }

    pub fn request_body(self) -> Option<Bytes> {
        match self {
            Self::JsonValidate => Some(Bytes::from(
                serde_json::to_vec(&CreateItem {
                    name: "widget".to_owned(),
                    count: 3,
                })
                .expect("serialize benchmark body"),
            )),
            _ => None,
        }
    }
}

impl Display for Scenario {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scenario {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "json_ok" => Ok(Self::JsonOk),
            "path_param" => Ok(Self::PathParam),
            "json_validate" => Ok(Self::JsonValidate),
            "middleware_stack" => Ok(Self::MiddlewareStack),
            "typed_error" => Ok(Self::TypedError),
            _ => Err(format!("unknown scenario `{value}`")),
        }
    }
}

#[api_model]
pub struct MessageBody {
    message: String,
}

#[api_model]
pub struct UserBody {
    id: i64,
    message: String,
}

#[api_model]
pub struct CreateItem {
    #[field(min_length = 1)]
    name: String,
    count: i64,
}

#[api_model]
pub struct ErrorBody {
    code: String,
    message: String,
}

#[derive(Debug, PartialEq, tork::AppError)]
#[status(503)]
enum DbError {
    Timeout,
}

impl Display for DbError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("database timed out")
    }
}

impl Error for DbError {}

#[derive(Debug)]
enum AxumBenchError {
    DbTimeout,
    Validation(garde::error::Report),
}

impl Display for AxumBenchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DbTimeout => f.write_str("database timed out"),
            Self::Validation(_) => f.write_str("validation failed"),
        }
    }
}

impl Error for AxumBenchError {}

impl AxumIntoResponse for AxumBenchError {
    fn into_response(self) -> AxumResponse {
        match self {
            Self::DbTimeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(error_payload("DB_TIMEOUT", "database timed out")),
            )
                .into_response(),
            Self::Validation(report) => {
                let details: Vec<Value> = report
                    .iter()
                    .map(|(path, error)| {
                        json!({
                            "field": path.to_string(),
                            "issue": "INVALID",
                            "message": error.to_string(),
                        })
                    })
                    .collect();
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    axum::Json(json!({
                        "status": 422,
                        "code": "VALIDATION_ERROR",
                        "title": "Unprocessable Entity",
                        "message": "The submitted data failed validation.",
                        "details": details,
                        "traceId": "bench-validation",
                        "timestamp": "1970-01-01T00:00:00Z"
                    })),
                )
                    .into_response()
            }
        }
    }
}

fn error_payload(code: &str, message: &str) -> Value {
    json!({
        "code": code,
        "message": message,
    })
}

#[get("/json")]
async fn json_ok_tork() -> TorkResult<MessageBody> {
    Ok(MessageBody {
        message: "ok".to_owned(),
    })
}

#[get("/users/{user_id}")]
async fn path_param_tork(user_id: i64) -> TorkResult<UserBody> {
    Ok(UserBody {
        id: user_id,
        message: "user".to_owned(),
    })
}

#[post("/items", status_code = 201)]
async fn create_item_tork(payload: Valid<CreateItem>) -> TorkResult<CreateItem> {
    Ok(payload.into_inner())
}

#[get("/middleware")]
async fn middleware_tork() -> TorkResult<MessageBody> {
    Ok(MessageBody {
        message: "middleware_ok".to_owned(),
    })
}

#[get("/db")]
async fn typed_error_tork() -> TorkResult<MessageBody> {
    let result: Result<MessageBody, DbError> = Err(DbError::Timeout);
    Ok(result?)
}

#[middleware]
async fn seed_request_id(request: Request, next: Next) -> TorkResult<Response> {
    let mut request = request;
    request.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_static(BENCH_REQUEST_ID),
    );
    let mut response = next.run(request).await?;
    response.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_static(BENCH_REQUEST_ID),
    );
    Ok(response)
}

#[middleware]
async fn mark_stack(request: Request, next: Next) -> TorkResult<Response> {
    let mut response = next.run(request).await?;
    response.headers_mut().insert(
        HeaderName::from_static("x-bench-stack"),
        HeaderValue::from_static(BENCH_STACK_VALUE),
    );
    Ok(response)
}

async fn json_ok_axum() -> AxumJson<MessageBody> {
    AxumJson(MessageBody {
        message: "ok".to_owned(),
    })
}

async fn path_param_axum(AxumPath(user_id): AxumPath<i64>) -> AxumJson<UserBody> {
    AxumJson(UserBody {
        id: user_id,
        message: "user".to_owned(),
    })
}

async fn create_item_axum(
    AxumJson(payload): AxumJson<CreateItem>,
) -> Result<(StatusCode, AxumJson<CreateItem>), AxumBenchError> {
    payload.validate().map_err(AxumBenchError::Validation)?;
    Ok((StatusCode::CREATED, AxumJson(payload)))
}

async fn middleware_axum() -> AxumJson<MessageBody> {
    AxumJson(MessageBody {
        message: "middleware_ok".to_owned(),
    })
}

async fn typed_error_axum() -> Result<AxumJson<MessageBody>, AxumBenchError> {
    Err(AxumBenchError::DbTimeout)
}

async fn axum_seed_request_id(
    mut request: AxumRequest,
    next: AxumNext,
) -> Result<AxumResponse, StatusCode> {
    request.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_static(BENCH_REQUEST_ID),
    );
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_static(BENCH_REQUEST_ID),
    );
    Ok(response)
}

async fn axum_mark_stack(request: AxumRequest, next: AxumNext) -> Result<AxumResponse, StatusCode> {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        HeaderName::from_static("x-bench-stack"),
        HeaderValue::from_static(BENCH_STACK_VALUE),
    );
    Ok(response)
}

fn quiet_logger() -> LoggerConfig {
    LoggerConfig::new().level("error").request_logs(false)
}

pub fn build_tork_app(scenario: Scenario) -> App {
    let app = App::new().logger(quiet_logger());
    match scenario {
        Scenario::JsonOk => app.include(json_ok_tork),
        Scenario::PathParam => app.include(path_param_tork),
        Scenario::JsonValidate => app.include(create_item_tork),
        Scenario::MiddlewareStack => app
            .middleware(seed_request_id)
            .middleware(mark_stack)
            .include(middleware_tork),
        Scenario::TypedError => app
            .exception_handler::<DbError, _, _>(|_error, _ctx| async move {
                json_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &error_payload("DB_TIMEOUT", "database timed out"),
                )
            })
            .include(typed_error_tork),
    }
}

pub fn build_axum_app(scenario: Scenario) -> AxumRouter {
    match scenario {
        Scenario::JsonOk => AxumRouter::new().route("/json", axum_get(json_ok_axum)),
        Scenario::PathParam => {
            AxumRouter::new().route("/users/{user_id}", axum_get(path_param_axum))
        }
        Scenario::JsonValidate => AxumRouter::new().route("/items", axum_post(create_item_axum)),
        Scenario::MiddlewareStack => AxumRouter::new()
            .route("/middleware", axum_get(middleware_axum))
            .layer(axum_middleware::from_fn(axum_mark_stack))
            .layer(axum_middleware::from_fn(axum_seed_request_id)),
        Scenario::TypedError => AxumRouter::new().route("/db", axum_get(typed_error_axum)),
    }
}

pub enum RunningServer {
    Tork {
        addr: SocketAddr,
        shutdown: Option<oneshot::Sender<()>>,
        handle: JoinHandle<()>,
    },
    Axum {
        addr: SocketAddr,
        shutdown: Option<oneshot::Sender<()>>,
        handle: JoinHandle<()>,
    },
}

impl RunningServer {
    pub fn local_addr(&self) -> SocketAddr {
        match self {
            Self::Tork { addr, .. } => *addr,
            Self::Axum { addr, .. } => *addr,
        }
    }

    pub fn base_uri(&self) -> BenchResult<Uri> {
        Ok(format!("http://{}", self.local_addr()).parse()?)
    }

    pub async fn shutdown(self) -> BenchResult<()> {
        match self {
            Self::Tork {
                shutdown, handle, ..
            } => {
                if let Some(tx) = shutdown {
                    let _ = tx.send(());
                }
                let _ = handle.await;
            }
            Self::Axum {
                shutdown, handle, ..
            } => {
                if let Some(tx) = shutdown {
                    let _ = tx.send(());
                }
                let _ = handle.await;
            }
        }
        Ok(())
    }
}

pub async fn spawn_server(backend: Backend, scenario: Scenario) -> BenchResult<RunningServer> {
    match backend {
        Backend::Tork => {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            drop(listener);

            let app = build_tork_app(scenario);
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let handle = tokio::spawn(async move {
                let _ = app
                    .serve_with_shutdown(addr.to_string(), async move {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });

            wait_until_ready(addr).await?;
            Ok(RunningServer::Tork {
                addr,
                shutdown: Some(shutdown_tx),
                handle,
            })
        }
        Backend::Axum => {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let app = build_axum_app(scenario);
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let handle = tokio::spawn(async move {
                let _ = axum_serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });
            Ok(RunningServer::Axum {
                addr,
                shutdown: Some(shutdown_tx),
                handle,
            })
        }
    }
}

pub fn http_client() -> Client<HttpConnector, Full<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

#[derive(Debug)]
pub struct BufferedResponse {
    pub status: StatusCode,
    pub headers: http::HeaderMap,
    pub body: Bytes,
}

pub async fn send_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    base_uri: &Uri,
    scenario: Scenario,
) -> BenchResult<BufferedResponse> {
    let uri = endpoint_uri(base_uri, scenario.path())?;
    let mut builder = http::Request::builder().method(scenario.method()).uri(uri);
    let body = scenario.request_body().unwrap_or_default();
    if scenario.method() == Method::POST {
        builder = builder.header(CONTENT_TYPE, JSON_CONTENT_TYPE);
    }
    let response = client.request(builder.body(Full::new(body))?).await?;
    let (parts, body) = response.into_parts();
    let buffered = body.collect().await?.to_bytes();
    Ok(BufferedResponse {
        status: parts.status,
        headers: parts.headers,
        body: buffered,
    })
}

pub async fn send_invalid_validation_request(
    client: &Client<HttpConnector, Full<Bytes>>,
    base_uri: &Uri,
) -> BenchResult<BufferedResponse> {
    let uri = endpoint_uri(base_uri, Scenario::JsonValidate.path())?;
    let body = Bytes::from_static(br#"{"name":"","count":3}"#);
    let response = client
        .request(
            http::Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(CONTENT_TYPE, JSON_CONTENT_TYPE)
                .body(Full::new(body))?,
        )
        .await?;
    let (parts, body) = response.into_parts();
    Ok(BufferedResponse {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await?.to_bytes(),
    })
}

fn endpoint_uri(base_uri: &Uri, path: &str) -> BenchResult<Uri> {
    Ok(format!("{}{}", base_uri.to_string().trim_end_matches('/'), path).parse()?)
}

async fn wait_until_ready(addr: SocketAddr) -> BenchResult<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(_stream) => {
                return Ok(());
            }
            Err(error) if Instant::now() < deadline => {
                if error.kind() != std::io::ErrorKind::ConnectionRefused {
                    return Err(Box::new(error));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
}

pub fn assert_expected_response(
    scenario: Scenario,
    response: &BufferedResponse,
) -> BenchResult<()> {
    match scenario {
        Scenario::JsonOk => {
            assert_eq!(response.status, StatusCode::OK);
            assert_json_eq(&response.body, &json!({ "message": "ok" }))?;
        }
        Scenario::PathParam => {
            assert_eq!(response.status, StatusCode::OK);
            assert_json_eq(&response.body, &json!({ "id": 42, "message": "user" }))?;
        }
        Scenario::JsonValidate => {
            assert_eq!(response.status, StatusCode::CREATED);
            assert_json_eq(&response.body, &json!({ "name": "widget", "count": 3 }))?;
        }
        Scenario::MiddlewareStack => {
            assert_eq!(response.status, StatusCode::OK);
            assert_eq!(
                response
                    .headers
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some(BENCH_REQUEST_ID)
            );
            assert_eq!(
                response
                    .headers
                    .get("x-bench-stack")
                    .and_then(|value| value.to_str().ok()),
                Some(BENCH_STACK_VALUE)
            );
            assert_json_eq(&response.body, &json!({ "message": "middleware_ok" }))?;
        }
        Scenario::TypedError => {
            assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
            assert_json_eq(
                &response.body,
                &json!({ "code": "DB_TIMEOUT", "message": "database timed out" }),
            )?;
        }
    }
    Ok(())
}

pub fn assert_invalid_validation_response(response: &BufferedResponse) -> BenchResult<()> {
    let body: Value = serde_json::from_slice(&response.body)?;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["status"], 422);
    assert_eq!(body["code"], "VALIDATION_ERROR");
    assert_eq!(body["message"], "The submitted data failed validation.");
    let has_name_detail = body["details"]
        .as_array()
        .map(|details| details.iter().any(|detail| detail["field"] == "name"))
        .unwrap_or(false);
    assert!(
        has_name_detail,
        "expected a validation detail for `name`: {body}"
    );
    Ok(())
}

fn assert_json_eq(actual: &[u8], expected: &Value) -> BenchResult<()> {
    let actual: Value = serde_json::from_slice(actual)?;
    assert_eq!(actual, *expected);
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LoadConfig {
    pub warmup: Duration,
    pub measure: Duration,
    pub concurrency: usize,
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            warmup: Duration::from_secs(5),
            measure: Duration::from_secs(20),
            concurrency: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadReport {
    pub backend: Backend,
    pub scenario: Scenario,
    pub concurrency: usize,
    pub elapsed: Duration,
    pub success_count: u64,
    pub error_count: u64,
    pub latencies_us: Vec<u64>,
}

impl LoadReport {
    pub fn requests_per_sec(&self) -> f64 {
        if self.elapsed.is_zero() {
            0.0
        } else {
            self.success_count as f64 / self.elapsed.as_secs_f64()
        }
    }

    pub fn average_latency_ms(&self) -> f64 {
        average_us(&self.latencies_us) / 1000.0
    }

    pub fn p50_ms(&self) -> f64 {
        percentile_us(&self.latencies_us, 0.50) / 1000.0
    }

    pub fn p95_ms(&self) -> f64 {
        percentile_us(&self.latencies_us, 0.95) / 1000.0
    }

    pub fn p99_ms(&self) -> f64 {
        percentile_us(&self.latencies_us, 0.99) / 1000.0
    }
}

fn average_us(latencies: &[u64]) -> f64 {
    if latencies.is_empty() {
        0.0
    } else {
        latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
    }
}

fn percentile_us(latencies: &[u64], percentile: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }
    let mut values = latencies.to_vec();
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index] as f64
}

pub async fn run_load(
    backend: Backend,
    scenario: Scenario,
    config: LoadConfig,
) -> BenchResult<LoadReport> {
    let server = spawn_server(backend, scenario).await?;
    let base_uri = server.base_uri()?;
    let client = Arc::new(http_client());

    if !config.warmup.is_zero() {
        run_load_phase(
            client.clone(),
            base_uri.clone(),
            scenario,
            config.concurrency,
            config.warmup,
            false,
        )
        .await?;
    }

    let started = Instant::now();
    let (success_count, error_count, latencies_us) = run_load_phase(
        client,
        base_uri,
        scenario,
        config.concurrency,
        config.measure,
        true,
    )
    .await?;
    let elapsed = started.elapsed();
    server.shutdown().await?;

    Ok(LoadReport {
        backend,
        scenario,
        concurrency: config.concurrency,
        elapsed,
        success_count,
        error_count,
        latencies_us,
    })
}

async fn run_load_phase(
    client: Arc<Client<HttpConnector, Full<Bytes>>>,
    base_uri: Uri,
    scenario: Scenario,
    concurrency: usize,
    duration: Duration,
    record_latencies: bool,
) -> BenchResult<(u64, u64, Vec<u64>)> {
    let deadline = Instant::now() + duration;
    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let client = client.clone();
        let base_uri = base_uri.clone();
        handles.push(tokio::spawn(async move {
            let mut success_count = 0_u64;
            let mut error_count = 0_u64;
            let mut latencies = Vec::new();
            loop {
                if Instant::now() >= deadline {
                    break;
                }
                let started = Instant::now();
                match send_request(&client, &base_uri, scenario).await {
                    Ok(response) => {
                        if assert_expected_response(scenario, &response).is_ok() {
                            success_count += 1;
                            if record_latencies {
                                latencies.push(started.elapsed().as_micros() as u64);
                            }
                        } else {
                            error_count += 1;
                        }
                    }
                    Err(_) => error_count += 1,
                }
            }
            (success_count, error_count, latencies)
        }));
    }

    let mut success_count = 0_u64;
    let mut error_count = 0_u64;
    let mut latencies = Vec::new();
    for handle in handles {
        let (worker_success, worker_error, worker_latencies) = handle.await?;
        success_count += worker_success;
        error_count += worker_error;
        latencies.extend(worker_latencies);
    }
    Ok((success_count, error_count, latencies))
}

pub fn markdown_report(report: &LoadReport) -> String {
    format!(
        "| backend | scenario | concurrency | ok | errors | rps | avg_ms | p50_ms | p95_ms | p99_ms |\n\
         |---|---|---:|---:|---:|---:|---:|---:|---:|---:|\n\
         | {} | {} | {} | {} | {} | {:.2} | {:.3} | {:.3} | {:.3} | {:.3} |",
        report.backend,
        report.scenario,
        report.concurrency,
        report.success_count,
        report.error_count,
        report.requests_per_sec(),
        report.average_latency_ms(),
        report.p50_ms(),
        report.p95_ms(),
        report.p99_ms()
    )
}
