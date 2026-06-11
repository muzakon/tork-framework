//! The in-process test client and its builder.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Full};

use super::TestOverrides;
use super::cookie::CookieJar;
use super::request::{PendingBody, TestRequestBuilder};
use super::response::TestResponse;
use crate::app::{App, AppInner, TestApp};
use crate::body::{ReqBody, box_body};
use crate::error::{Error, Result};
use crate::state::StateMap;

/// A closure that registers one override resource into the state map.
type ResourceRegister = Box<dyn FnOnce(&mut StateMap)>;

/// How a built request reaches the application.
///
/// In-process is the default (no network). A real-port variant is added later for
/// end-to-end tests.
pub(crate) enum Transport {
    /// Drive the application directly, in process.
    InProcess(Arc<AppInner>),
}

impl Transport {
    /// Executes a request and returns the status, headers, and full body.
    async fn execute(
        &self,
        request: http::Request<ReqBody>,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        match self {
            Transport::InProcess(app) => {
                let response = app.clone().handle(request).await;
                let (parts, body) = response.into_parts();
                let bytes = body
                    .collect()
                    .await
                    .map_err(|error| {
                        Error::internal(format!("failed to read response body: {error}"))
                    })?
                    .to_bytes();
                Ok((parts.status, parts.headers, bytes))
            }
        }
    }
}

/// State shared between the client and its request builders.
pub(crate) struct Shared {
    pub(crate) transport: Transport,
    default_headers: HeaderMap,
    cookies: Mutex<CookieJar>,
}

impl Shared {
    /// Builds, sends, and captures a request, updating the cookie jar.
    pub(crate) async fn send(
        &self,
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        headers: Vec<(HeaderName, HeaderValue)>,
        body: PendingBody,
    ) -> Result<TestResponse> {
        let request = self.build_request(method, &path, &query, headers, body)?;
        let (status, headers, bytes) = self.transport.execute(request).await?;
        self.cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .store(&headers);
        Ok(TestResponse {
            status,
            headers,
            body: bytes,
        })
    }

    /// Assembles an `http::Request`, merging default headers, the cookie jar, and
    /// the body's content type.
    fn build_request(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        headers: Vec<(HeaderName, HeaderValue)>,
        body: PendingBody,
    ) -> Result<http::Request<ReqBody>> {
        let uri = if query.is_empty() {
            path.to_owned()
        } else {
            let encoded = serde_urlencoded::to_string(query)
                .map_err(|_| Error::internal("failed to encode query parameters"))?;
            format!("{path}?{encoded}")
        };

        let mut request = http::Request::new(box_body(Full::new(body.bytes)));
        *request.method_mut() = method;
        *request.uri_mut() = uri
            .parse()
            .map_err(|_| Error::bad_request(format!("invalid request URI: {uri}")))?;

        let map = request.headers_mut();
        for (name, value) in self.default_headers.iter() {
            map.insert(name, value.clone());
        }
        for (name, value) in headers {
            map.insert(name, value);
        }
        self.cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .apply(map);
        if let Some(content_type) = body.content_type {
            map.insert(super::request::CONTENT_TYPE_HEADER, content_type);
        }

        Ok(request)
    }
}

/// An in-process client for exercising an application in tests.
///
/// Build it from a [`TestApp`] with [`TestClient::new`], or configure overrides
/// through [`TestClient::builder`]. Requests run straight through the request
/// pipeline with no network. Call [`shutdown`](TestClient::shutdown) when finished
/// to run the lifespan teardown.
pub struct TestClient {
    shared: Arc<Shared>,
    teardown: TestApp,
}

impl TestClient {
    /// Builds a client from an already-built [`TestApp`].
    pub async fn new(app: TestApp) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(Shared {
                transport: Transport::InProcess(app.inner.clone()),
                default_headers: HeaderMap::new(),
                cookies: Mutex::new(CookieJar::default()),
            }),
            teardown: app,
        })
    }

    /// Starts a builder for a client with resource and dependency overrides.
    pub fn builder(app: App) -> TestClientBuilder {
        TestClientBuilder::new(app)
    }

    // Used by the WebSocket/SSE builders, which land in a later commit.
    #[allow(dead_code)]
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Starts a `GET` request.
    pub fn get(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::GET, path)
    }

    /// Starts a `POST` request.
    pub fn post(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::POST, path)
    }

    /// Starts a `PUT` request.
    pub fn put(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::PUT, path)
    }

    /// Starts a `PATCH` request.
    pub fn patch(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::PATCH, path)
    }

    /// Starts a `DELETE` request.
    pub fn delete(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::DELETE, path)
    }

    /// Runs the lifespan shutdown.
    pub async fn shutdown(self) -> Result<()> {
        self.teardown.shutdown().await
    }
}

/// A builder for a [`TestClient`] with resource and dependency overrides, default
/// headers, and seeded cookies.
pub struct TestClientBuilder {
    app: App,
    resources: Vec<ResourceRegister>,
    overrides: TestOverrides,
    default_headers: HeaderMap,
    cookies: CookieJar,
}

impl TestClientBuilder {
    fn new(app: App) -> Self {
        Self {
            app,
            resources: Vec::new(),
            overrides: TestOverrides::default(),
            default_headers: HeaderMap::new(),
            cookies: CookieJar::default(),
        }
    }

    /// Registers (or overrides) a resource by type, applied after startup so it
    /// wins over a value the lifespan registered.
    pub fn resource<S: Send + Sync + 'static>(mut self, value: S) -> Self {
        self.resources
            .push(Box::new(move |state| state.insert(value)));
        self
    }

    /// Overrides an injected dependency with a pre-built value, cloned per request.
    pub fn override_dependency<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.overrides.insert::<T, _>(move || value.clone());
        self
    }

    /// Overrides an injected dependency with a factory invoked per request.
    pub fn override_dependency_with<T, F>(mut self, factory: F) -> Self
    where
        T: Send + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.overrides.insert::<T, F>(factory);
        self
    }

    /// Sets a default header sent with every request.
    pub fn default_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            self.default_headers.insert(name, value);
        }
        self
    }

    /// Seeds a cookie sent with every request.
    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.set(name, value);
        self
    }

    /// Builds the client, running startup and applying the overrides.
    pub async fn build(self) -> Result<TestClient> {
        let resources = self.resources;
        let overrides = self.overrides;
        let default_headers = self.default_headers;
        let cookies = self.cookies;

        let app = self
            .app
            .build_test_with(move |state| {
                for register in resources {
                    register(state);
                }
                if !overrides.is_empty() {
                    state.insert(overrides);
                }
            })
            .await?;

        Ok(TestClient {
            shared: Arc::new(Shared {
                transport: Transport::InProcess(app.inner.clone()),
                default_headers,
                cookies: Mutex::new(cookies),
            }),
            teardown: app,
        })
    }
}
