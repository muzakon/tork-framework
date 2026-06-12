# Current Risks & Untested Behaviors

This document lists the components, edge cases, and features of the Tork Framework that are currently untested or carry potential risks, ordered from **most critical (highest risk)** to **lowest risk**.

---

Status legend:
- `[x]` Resolved
- `[~]` Mitigated or partially constrained
- `[!]` Confirmed open risk

## 1. [x] Proxy Header Spoofing — Host Header Takeover (High Security Risk)
- **Risk:** The `ProxyHeaders` middleware now ignores forwarded host/scheme headers unless the request peer matches an explicit trusted IP/CIDR allowlist.
- **Status:** Resolved. Covered by regression tests for untrusted spoof rejection and trusted loopback proxy acceptance.

## 2. [x] HTTPS Redirect Spoofing via `X-Forwarded-Proto` (High Security Risk)
- **Risk:** `HttpsRedirect` now uses normalized scheme metadata from trusted proxy processing, not raw `X-Forwarded-Proto`.
- **Status:** Resolved. Covered by regression tests for untrusted header rejection and trusted proxy pass-through.

## 3. [x] WebSocket Origin Validation Missing (High Security Risk)
- **Risk:** The handshake now enforces same-origin by default when `Origin` is present, while still allowing non-browser clients with no `Origin`.
- **Status:** Resolved. Covered by regression tests for default cross-origin `403`, same-origin success, and explicit allowlist opt-in.

## 4. Missing Rate Limiting (High Risk / Feature Deficit)
- **Risk:** The framework provides no built-in rate limiting middleware.
- **Vulnerability:** All endpoints are exposed to brute-force attacks (login, password reset), API abuse, and denial-of-service. Without rate limiting at the framework level, every application must implement its own, and many will ship without it.
- **Status:** Unimplemented. No `429 Too Many Requests` middleware exists despite `ErrorKind::TooManyRequests` being defined in the error module.

## 5. [x] UploadFile::save_to Path Traversal (High Security Risk)
- **Risk:** `save_to` now rejects absolute and parent-traversal targets, and `save_to_dir(dir, file_name)` provides the safe persistence API for normal uploads.
- **Status:** Resolved. Covered by regression tests for safe directory writes and invalid path rejection.

## 6. [x] CORS Wildcard + Credentials Reflects Any Origin (Medium Security Risk)
- **Risk:** Wildcard-plus-credentials now fails closed: no `Access-Control-Allow-Origin` is granted for that invalid configuration.
- **Status:** Resolved. Covered by regression tests for both preflight and actual-request behavior.

## 7. [x] Panic Boundary Disabled by Default (Medium Risk)
- **Risk:** `App::new()` now catches handler panics by default, with explicit opt-out via `propagate_panics()`.
- **Status:** Resolved. Covered by unit and integration tests for default `500` conversion and explicit panic propagation.

## 8. No Response Body Size Limit (Medium Risk)
- **Risk:** The `RespBody` type ([`body.rs:L31-L97`](crates/tork-core/src/body.rs)) has no size limit enforcement. A handler returning `RespBody::stream(...)` can stream an unbounded amount of data to the client.
- **Vulnerability:** While the request body has `MAX_BODY_BYTES` (2 MiB), the response side has no equivalent protection. An endpoint streaming data (file download, SSE) could exhaust server memory and bandwidth, affecting all other connections.
- **Status:** Untested. No server-side response body cap exists.

## 9. [x] CORS Missing `Vary` Headers for Preflight Negotiation (Medium Security Risk)
- **Risk:** Preflight responses now include `Vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers`.
- **Status:** Resolved. Covered by regression tests for full `Vary` coverage.

## 10. [x] Log Injection via Unsanitized Request Path (Medium Security Risk)
- **Risk:** Request path and other request-derived log fields are sanitized before console emission.
- **Status:** Resolved. Covered by log-forgery regression tests.

## 11. OpenAPI Documentation UI Unprotected (Medium Security Risk)
- **Risk:** The OpenAPI spec (`/openapi.json`) and Scalar documentation UI (`/docs`) routes ([`docs.rs:L22-L31`](crates/tork-openapi/src/docs.rs), [`spec.rs:L86-L108`](crates/tork-openapi/src/spec.rs)) are registered as plain GET routes with no authentication or authorization checks.
- **Vulnerability:** An attacker accesses `/openapi.json` to discover the full API surface including parameter names, types, error codes, and internal route structures. This information disclosure aids in crafting targeted attacks. The routes are added after the developer's router configuration, making them difficult to gate behind auth middleware.
- **Status:** Untested. No built-in protection mechanism.

## 12. Scalar CDN Without Subresource Integrity (Medium Security Risk / Supply Chain)
- **Risk:** The documentation UI loads the Scalar API reference library from `https://cdn.jsdelivr.net/npm/@scalar/api-reference` ([`docs.rs:L17`](crates/tork-openapi/src/docs.rs)) without an SRI hash.
- **Vulnerability:** If the CDN is compromised or a supply-chain attack occurs on the `@scalar/api-reference` npm package, arbitrary JavaScript executes in every browser visiting the documentation page, potentially stealing API tokens or performing actions on behalf of the user.
- **Status:** Untested. No SRI hash or fallback mechanism.

## 13. Error Chain Leakage in Structured Logs (Medium Security Risk)
- **Risk:** The `LogEvent::error` method ([`event.rs:L31-L52`](crates/tork-core/src/logging/event.rs)) serializes the full error chain (type name, message, and all `source()` messages) into log output.
- **Vulnerability:** If an error's `Display` implementation includes sensitive data (database connection strings with passwords, API keys), this data appears in structured logs. Database drivers commonly include connection strings in error messages.
- **Status:** Untested. No sensitive data filtering in error logging.

## 14. [x] Request ID Logged Without Sanitization (Medium Security Risk)
- **Risk:** Client-supplied request IDs are sanitized before they reach console logs.
- **Status:** Resolved. Covered by log-forgery regression tests.

## 15. Multipart `temp_dir` Configuration Not Propagated (Medium Risk)
- **Risk:** `UploadConfig` exposes a `temp_dir` field ([`multipart.rs:L88-L91`](crates/tork-core/src/multipart.rs)), but the actual `SpooledTempFile` creation uses the system default temp directory, ignoring this setting.
- **Bug:** Declaring `temp_dir("/mnt/fast-ssd")` has no effect. In containerized environments where the default `/tmp` is a memory-backed `tmpfs`, large uploads spill to RAM instead of disk, causing OOM kills.
- **Status:** Untested. The `temp_dir` field is dead configuration.

## 16. [x] BodyLimit Middleware Bypass via Chunked Transfer Encoding (Medium Risk)
- **Risk:** `BodyLimit` now consumes and bounds request bodies regardless of transfer encoding before the handler runs.
- **Status:** Resolved. Covered by regression tests for chunked under-limit and over-limit requests.

## 17. No Per-Client WebSocket Connection Limit (Medium Risk)
- **Risk:** The WebSocket upgrade path has no limit on the number of concurrent connections from a single client IP.
- **Vulnerability:** A single client can open thousands of WebSocket connections, exhausting server memory, file descriptors, and task capacity. This is a denial-of-service vector, especially without rate limiting.
- **Status:** Untested and unimplemented.

## 18. Hub Room Memory Leak (Medium Risk / Resource Exhaustion)
- **Risk:** The `Hub` ([`realtime.rs:L16-L59`](crates/tork-core/src/realtime.rs)) stores rooms in a `HashMap` behind a `Mutex`. Once created, rooms are never evicted.
- **Bug:** In a long-running server where rooms are dynamically named (e.g., per-user or per-session rooms), the `HashMap` grows unboundedly. Each room holds a `broadcast::Sender` with a 256-message buffer. Over time this causes unbounded memory growth.
- **Status:** Untested. No eviction, TTL, or cleanup mechanism exists.

## 19. Missing Security Headers Middleware (Medium Risk / Feature Deficit)
- **Risk:** The framework provides no middleware for standard security headers (`Strict-Transport-Security`, `X-Content-Type-Options`, `X-Frame-Options`, `X-XSS-Protection`, `Content-Security-Policy`, `Referrer-Policy`).
- **Vulnerability:** Applications must implement these headers manually. Without them, applications are vulnerable to clickjacking, MIME-sniffing attacks, and will fail security audits.
- **Status:** Unimplemented.

## 20. WebSocket Upgrade No Handshake Timeout (Medium Risk)
- **Risk:** The WebSocket upgrade process ([`ws.rs:L461-L488`](crates/tork-core/src/ws.rs)) awaits `on_upgrade.await` without a timeout.
- **Vulnerability:** A slow or malicious client can initiate a WebSocket upgrade and then stall the TCP connection indefinitely before completing the handshake. The server task waits forever, consuming a connection slot.
- **Status:** Untested. No timeout on the upgrade future.

## 21. Graceful Shutdown Drops In-Flight WebSocket Connections (Medium Risk)
- **Risk:** The server's graceful shutdown ([`server.rs:L76`](crates/tork-core/src/server.rs)) drains HTTP connections with a timeout but does not explicitly close WebSocket connections.
- **Bug:** WebSocket connections are long-lived and will not complete within the 15-second drain timeout. They are forcibly terminated, causing clients to receive unexpected disconnects without proper close frames.
- **Status:** Untested. No WebSocket-aware shutdown protocol.

## 22. [x] UploadFile::save_to Symlink Attack (Medium Security Risk)
- **Risk:** Existing symlink targets are rejected and destination writes use `create_new(true)` to avoid overwrite-through-existing-target behavior.
- **Status:** Resolved. Covered by the symlink attack regression test.

## 23. Compression Middleware Doesn't Set `Vary` on Non-Gzip Path (Low-Medium Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L60-L119`](crates/tork-core/src/middleware/compression.rs)) sets `Vary: Accept-Encoding` only when compression is actually applied.
- **Bug:** When gzip is disabled or the body is too small, the `Vary` header is not set. Intermediate caches may serve a compressed response to a client that doesn't support gzip, or an uncompressed response to a client that expects compression.
- **Status:** Untested for cache behavior.

## 24. TrustedHost Port Stripping Fails for IPv6 (Low-Medium Risk)
- **Risk:** The `TrustedHost` middleware ([`trusted_host.rs:L63-L64`](crates/tork-core/src/middleware/trusted_host.rs)) strips ports via `value.split(':').next()`.
- **Bug:** IPv6 addresses like `[::1]:8080` are not handled correctly. The split on `:` produces `[` as the first segment, which won't match any pattern. All IPv6-based `Host` headers are silently rejected.
- **Status:** Untested for IPv6 hosts.

## 25. WebSocket Disconnect Hook Fires Detached Task (Low-Medium Risk)
- **Risk:** `WebSocketConn::Drop` ([`ws.rs:L501-L519`](crates/tork-core/src/ws.rs)) spawns a detached tokio task to fire disconnect hooks because `Drop` cannot be async.
- **Vulnerability:** If the tokio runtime is shutting down (e.g., during graceful shutdown), `Handle::try_current()` may fail, silently skipping disconnect hooks. Metrics, cleanup logic, and audit trails in disconnect hooks will not execute during shutdown.
- **Status:** Untested for shutdown-time hook execution.

## 26. Settings Loader Environment Variable Race Condition (Low-Medium Risk)
- **Risk:** The `SettingsLoader` uses `dotenvy` to load `.env` files into the process environment ([`settings.rs:L187-L194`](crates/tork-core/src/settings.rs)) and reads environment variables with `std::env::var`.
- **Vulnerability:** In multi-threaded test runners or applications that reload config at runtime, `std::env::set_var` and `std::env::remove_var` are not thread-safe (documented in Rust stdlib). Concurrent config loading can read corrupted environment state.
- **Status:** Tests use `set_var`/`remove_var` without synchronization, which is undefined behavior in multi-threaded contexts.

## 27. SSE Stream Errors Logged to stderr (Low-Medium Risk)
- **Risk:** SSE stream errors ([`sse.rs:L393`](crates/tork-core/src/sse.rs)) are printed via `eprintln!` rather than the framework's structured logging system.
- **Vulnerability:** In production, these errors bypass log aggregation, alerting, and structured output. Operators cannot detect SSE failure rates or correlate them with trace IDs.
- **Status:** Untested. No logging hook integration for SSE errors.

## 28. Multipart Form Field Name Collision (Low-Medium Risk)
- **Risk:** The `MultipartForm` parser ([`multipart.rs:L526-L534`](crates/tork-core/src/multipart.rs)) uses `take_form_value` which takes the first field with a given name.
- **Bug:** If a multipart form contains multiple fields with the same name but different types (e.g., `count` as both text and file), the first `take_form_value` call consumes the text field, and subsequent calls silently return `None`. There is no type-safe enforcement that a field name maps to exactly one expected type.
- **Status:** Untested for name collision scenarios.

## 29. Error Response Does Not Set `Cache-Control` (Low-Medium Risk)
- **Risk:** The `Error` type's `IntoResponse` implementation ([`error.rs:L364-L401`](crates/tork-core/src/error.rs)) does not set `Cache-Control: no-store` on error responses.
- **Vulnerability:** Intermediate proxies or browsers may cache `4xx`/`5xx` error responses. A cached `401 Unauthorized` can prevent legitimate retry attempts, and a cached `500 Internal Server Error` can mask ongoing issues.
- **Status:** Untested for cache behavior on error responses.

## 30. HTML Escape Missing Single Quote (Low-Medium Risk)
- **Risk:** The `html_escape` function in the OpenAPI docs ([`docs.rs:L55-L61`](crates/tork-openapi/src/docs.rs)) escapes `&`, `<`, `>`, and `"`, but does not escape single quotes (`'`) or backticks.
- **Vulnerability:** If `spec_url` were ever placed inside a single-quoted attribute, an attacker could inject `onload=alert(1)`. Currently mitigated by double-quote template usage, but this is a defense-in-depth gap.
- **Status:** Latent risk. Currently mitigated by template structure.

## 31. Panic Message Leakage Through Hooks (Low-Medium Risk)
- **Risk:** When `catch_panics` is enabled, caught panics fire `on_panic` hooks with the panic message ([`service.rs:L74-L83`](crates/tork-core/src/service.rs)). The `panic_message` function extracts the `&str` or `String` from the panic payload.
- **Vulnerability:** While the panic message does not reach the client, it is available to all registered panic hooks. If any hook logs or transmits this message, internal details (e.g., `panic!("db_password is {secret}")`) are exposed in logs.
- **Status:** Hooks should sanitize panic messages, but this is not enforced.

## 32. Trace Middleware Logs Request Path Before Routing (Low-Medium Risk)
- **Risk:** The `Trace` middleware ([`trace.rs:L32-L52`](crates/tork-core/src/middleware/trace.rs)) logs the request path before routing occurs.
- **Vulnerability:** Even requests that result in a `404` or `405` generate log entries with their full path. An attacker can use this to enumerate paths by observing which paths generate log entries, even if those paths are protected by middleware.
- **Status:** Information disclosure through logs.

---

## 33. BearerToken Extractor Allocates Per Request (Low Risk)
- **Risk:** `BearerToken` ([`header.rs:L118`](crates/tork-core/src/extract/header.rs)) clones the token string into an owned `String` on every request.
- **Optimization:** For high-throughput APIs, this creates significant allocation pressure. The token could be a zero-copy `&str` borrow from the request headers, but the current design requires ownership because the headers outlive the extractor.
- **Status:** Untested for allocation overhead.

## 34. Compression Buffer Allocates Before Checking Size (Low Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L85-L88`](crates/tork-core/src/middleware/compression.rs)) calls `into_body_bytes` (which buffers the entire response body) before checking if the body exceeds `minimum_size`.
- **Optimization:** For small responses that don't meet the compression threshold, the body is fully buffered into memory unnecessarily. Streaming responses should avoid buffering entirely when compression won't be applied.
- **Status:** Untested. Small responses are still buffered.

## 35. SSE Heartbeat Allocates New `Bytes` Every Interval (Low Risk)
- **Risk:** The SSE heartbeat frame ([`sse.rs:L29`](crates/tork-core/src/sse.rs)) is a static `&[u8]`, but each heartbeat emission wraps it in `Bytes::from_static`. While this is cheap, under extremely high SSE connection counts (10k+), the per-heartbeat framing and poll wakeups add non-trivial overhead.
- **Optimization:** A pre-encoded shared `Bytes` value would avoid repeated static wrapping.
- **Status:** Not benchmarked at scale.

## 36. Path Normalization Only Handles Trailing Slashes (Low Risk)
- **Risk:** The router matcher ([`matcher.rs:L124-L131`](crates/tork-core/src/router/matcher.rs)) normalizes only trailing slashes. Double slashes (`//`), encoded slashes (`%2F`), or case differences in path segments are not normalized.
- **Vulnerability:** Path traversal attempts using `//` or `..` may bypass route matching or reach unintended handlers depending on upstream proxy behavior.
- **Status:** Untested for path traversal vectors.

## 37. StateMap Silent Value Replacement (Low Risk)
- **Risk:** The `StateMap` ([`state.rs:L29-L38`](crates/tork-core/src/state.rs)) uses `TypeId` for keying and silently replaces any existing value of the same type on `insert`.
- **Vulnerability:** If a middleware, test override, or lifespan inserts a value of a type that is already registered, the previous value is silently dropped. This could lead to state pollution where a security-relevant value (like a database pool or auth configuration) is unintentionally replaced.
- **Status:** Primarily a testing/debugging concern, not a direct production attack vector.

## 38. Validation Body Buffering Depth (Low Risk)
- **Risk:** The `Valid<T>` extractor ([`valid.rs:L27-L43`](crates/tork-core/src/extract/valid.rs)) deserializes the body and then runs validation, meaning the full body is buffered in memory. There is no mechanism to short-circuit if deserialization consumes significant memory for deeply nested JSON.
- **Vulnerability:** An attacker sends a deeply nested JSON payload that passes `MAX_BODY_BYTES` but causes stack overflow or excessive allocation during deserialization. Mitigated by `MAX_BODY_BYTES` and `serde_json`'s own limits.
- **Status:** Low practical risk due to existing mitigations.

## 39. No Null Byte Injection Protection in Router (Low Risk)
- **Risk:** The `Matcher::find` method ([`matcher.rs:L73-L111`](crates/tork-core/src/router/matcher.rs)) receives the request path from `head.uri.path()` and passes it directly to `matchit::Router::at()` without checking for null bytes (`\0`).
- **Vulnerability:** If a reverse proxy forwards a request with a null byte in the path, the router might match it differently than expected. Mitigated by HTTP parsers typically rejecting null bytes in URIs.
- **Status:** Defense-in-depth gap. Mitigated at the HTTP layer.

## 40. Dependency Version Audit Required (Informational)
- **Risk:** Several dependencies should be checked against the RustSec advisory database:
  - `tokio-tungstenite = "0.24"` — older version; latest is 0.26+
  - `multer = "3.1"` — check for recent advisories
  - `matchit = "0.9"` — check for path traversal advisories
  - `hyper = "1.10"` — check for HTTP/2 and request smuggling advisories
- **Vulnerability:** A known vulnerability in any dependency could be exploited directly.
- **Status:** Requires running `cargo audit` to confirm specific CVEs.

## 41. Multipart Text Fields No Per-Field Size Limit (Low Risk)
- **Risk:** The multipart parser ([`multipart.rs:L511-L518`](crates/tork-core/src/multipart.rs)) reads entire text field values into `String` via `field.text()` without a per-field size limit.
- **Vulnerability:** A single text field could contain up to `max_body_size` (default 16 MiB) of data. While bounded by the total body limit, per-field limits would provide better defense-in-depth.
- **Status:** Mitigated by `max_body_size` but no per-field enforcement.

## 42. Route Metadata Could Inject Into OpenAPI JSON (Low Risk)
- **Risk:** Route summaries, descriptions, and tags ([`spec.rs:L117-L204`](crates/tork-openapi/src/spec.rs)) are serialized directly into the OpenAPI JSON document.
- **Vulnerability:** If a developer dynamically generates route descriptions from user input (unusual but possible), and the Scalar UI renders these as HTML, XSS could result. Currently mitigated because values come from developer-authored code and `serde_json` escapes strings.
- **Status:** Requires unusual dynamic route generation from user input.

## 43. Test Client Bypasses Security Validation (Low Risk)
- **Risk:** The test client ([`testing/client.rs:L56-L73`](crates/tork-core/src/testing/client.rs)) allows setting arbitrary security-sensitive headers (`Host`, `X-Forwarded-For`) without validation.
- **Vulnerability:** Integration tests using the test client may pass even when security middleware is misconfigured, giving a false sense of security. Developers may not realize CORS or TrustedHost middleware is missing.
- **Status:** Testing concern, not a direct production vulnerability.

---

## 44. Middleware Chain Arc Clones Per Request (Medium Concurrency Risk)
- **Risk:** Each request creates `Next::new` ([`mod.rs:L89-L95`](crates/tork-core/src/middleware/mod.rs)) which clones `Arc<AppInner>` and `Arc<[Arc<dyn Middleware>]>`. Then each middleware in the chain clones both again when calling `Next` ([`mod.rs:L104-L108`](crates/tork-core/src/middleware/mod.rs)).
- **Bug:** For a middleware stack of N layers, each request performs 2N+1 Arc atomic reference count increments. Under high concurrency (10k+ req/s), this creates significant atomic contention on the shared Arc counts. The `stack` is cloned per middleware invocation even though it never changes.
- **Status:** Untested. No benchmark measuring Arc contention under load.

## 45. Logger `with_field` Clones Entire Field Vec (Medium Concurrency Risk)
- **Risk:** `Logger::with_field` ([`logger.rs:L58-L67`](crates/tork-core/src/logging/logger.rs)) clones the entire `base` Vec (all previous fields) and wraps it in a new `Arc` on every call.
- **Bug:** When a handler calls `.with_field()` multiple times (e.g., `logger.with_field("a", 1).with_field("b", 2).with_field("c", 3)`), each call clones the accumulated fields. Under high throughput, this creates repeated Vec allocations and Arc churn.
- **Status:** Untested. No performance test measuring per-request logging allocation.

## 46. Per-Request String Allocations in Dispatch Hot Path (Low-Medium Performance Risk)
- **Risk:** The dispatch path ([`service.rs:L29-L58`](crates/tork-core/src/service.rs)) performs multiple `String::to_owned()` allocations per request: `head.uri.path().to_owned()`, `route.path().to_owned()`, `request_id.to_str().ok().map(str::to_owned)`, plus method logging strings.
- **Optimization:** These are necessary for the current logging design but create allocation pressure. Path and route strings could be borrowed or interned for zero-copy logging.
- **Status:** Not benchmarked. Under extreme load, this becomes a measurable overhead.

## 47. Tracing Span Created Per Request (Low-Medium Performance Risk)
- **Risk:** Every request creates a `tracing::info_span!` ([`service.rs:L62-L67`](crates/tork-core/src/service.rs)) with the method, route, and request ID as string fields. The span allocates and stores these values even when tracing is disabled or filtered out.
- **Optimization:** Span creation cost is non-trivial under high concurrency. The span should be conditionally created only when tracing is enabled for the request level.
- **Status:** Not benchmarked. The `tracing` framework may optimize this internally, but it's not verified.

## 48. SpooledTempFile Not Cleaned Up on Multipart Parse Error (Medium Resource Leak)
- **Risk:** The multipart parser ([`multipart.rs:L475-L519`](crates/tork-core/src/multipart.rs)) creates `SpooledTempFile` instances for each file field as data arrives. If a later field fails to parse (e.g., the body is truncated or a field exceeds limits), the already-created temp files are dropped.
- **Bug:** `SpooledTempFile::drop` closes the file handle, but if the file was spilled to disk, the OS may not immediately reclaim the disk space. Under sustained error conditions (e.g., an attacker sending malformed multipart bodies), temp files accumulate faster than the OS reclaims them.
- **Status:** Untested. No test verifying temp file cleanup on parse errors.

## 49. LogRecorder Accumulates Records Indefinitely (Low Resource Leak)
- **Risk:** `LogRecorder` ([`recorder.rs:L55-L68`](crates/tork-core/src/testing/recorder.rs)) pushes every `LogRecord` into a `Vec` behind a `Mutex`. There is no eviction, no max capacity, and no `clear()` method.
- **Bug:** In long-running tests or integration tests that generate many log lines, the `Vec` grows unboundedly. This is a test-only concern but can cause test OOM failures.
- **Status:** Untested. No max capacity or auto-cleanup.

## 50. Compression Buffers Entire Response Before Compressing (Medium Memory Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L85-L88`](crates/tork-core/src/middleware/compression.rs)) calls `into_body_bytes(response)` which collects the entire response body into a single `Bytes` buffer before checking size and compressing.
- **Bug:** A 10 MiB JSON response is first buffered entirely into memory, then gzip-compressed into another ~1-3 MiB buffer. Peak memory usage is ~13 MiB per concurrent request with compression enabled. Under high concurrency, this can exhaust memory.
- **Status:** Untested. No benchmark measuring peak memory under concurrent compressed responses.

## 51. SSE Stream Holds Pinned BoxStream + Interval Indefinitely (Medium Memory Risk)
- **Risk:** Each SSE response ([`sse.rs:L337-L345`](crates/tork-core/src/sse.rs)) creates an `SseBody` that holds a pinned `BoxStream`, an optional `Interval`, an optional `Sleep` timer, and an optional `Bytes` done event.
- **Bug:** These allocations persist for the entire lifetime of the SSE connection (which can be hours). With 10,000 concurrent SSE connections, this is 10,000 pinned streams + 10,000 interval timers consuming memory and waking up periodically.
- **Status:** Not benchmarked at scale. No connection limit or resource cap on SSE streams.

## 52. Hook Event Cloning Per Request (Low Concurrency Overhead)
- **Risk:** Every request that triggers hooks ([`app.rs:L682-L704`](crates/tork-core/src/app.rs)) clones `RequestInfo`, `ResponseEvent`, and `ErrorEvent` structs for each hook invocation. The `RequestInfo` clone is O(1) (Arc clones), but the `ResponseEvent` includes a `StatusCode` and `Duration`.
- **Optimization:** Events could be shared via `Arc` instead of cloned per hook invocation, especially when multiple hooks observe the same request.
- **Status:** Low practical impact; hooks are typically 1-3 per app.

## 53. StateMap Entries Never Evicted (Low Memory Risk)
- **Risk:** `StateMap` ([`state.rs:L29-L31`](crates/tork-core/src/state.rs)) holds `Arc<dyn Any + Send + Sync>` values keyed by `TypeId`. Once inserted, values are never removed or replaced unless the same type is re-inserted.
- **Bug:** In applications that dynamically register state (e.g., per-tenant resources), the map grows monotonically. There is no TTL, no eviction, and no capacity limit.
- **Status:** Typically state is registered once at startup, so practical impact is low unless dynamic registration is used.

## 54. WebSocket Connection Arc Clone Overhead (Low Concurrency Overhead)
- **Risk:** Each WebSocket connection ([`ws.rs:L476-L478`](crates/tork-core/src/ws.rs)) clones `Arc<WsHooks>` and captures it in the connection struct. The `WsHooks` contains `Vec<WsConnectHook>` and `Vec<WsDisconnectHook>`.
- **Optimization:** With many concurrent WebSocket connections and multiple hooks, each connection holds a strong reference to the same hooks vec. This is correct but creates Arc reference count contention.
- **Status:** Low practical impact; hooks are typically 1-2 per app.

## 55. Blocking IO in Multipart Spool Write (Medium Concurrency Risk)
- **Risk:** The multipart parser ([`multipart.rs:L497-L499`](crates/tork-core/src/multipart.rs)) writes file chunks to `SpooledTempFile` synchronously inside an async context (within the `while let Some(chunk)` loop). The `write_all` call is blocking IO.
- **Bug:** When the temp file spills to disk, `write_all` performs synchronous disk IO on the tokio runtime thread. Under high concurrency, this blocks the runtime thread and degrades throughput for all concurrent requests.
- **Status:** Untested. The `UploadFile::with_storage` method correctly uses `spawn_blocking` for reads, but the initial spool writes in `MultipartForm::parse` do not.

## 56. Settings Loader Allocates Multiple Figment Instances (Low Memory Risk)
- **Risk:** The `SettingsLoader::load` method ([`settings.rs:L184-L229`](crates/tork-core/src/settings.rs)) creates multiple `Figment` instances, merges TOML files, env providers, and secrets, then deserializes into a `serde_json::Value` intermediary before extracting the final typed value.
- **Optimization:** The layered merge allocates intermediate `Value` objects that are discarded after extraction. This is a one-time startup cost, not a runtime concern.
- **Status:** Acceptable for startup. Not a runtime memory issue.

---

## 57. Production Readiness Requirements & Missing Tests

The following verification steps and test suites are **mandatory** to complete before deploying this framework into production:

### A. [x] Security Middleware Integration Tests
- **Requirement:** End-to-end tests verifying that `ProxyHeaders`, `TrustedHost`, `HttpsRedirect`, and CORS work correctly when chained together, including spoofing resistance.
- **Status:** Resolved. Real-port and in-process regression tests now cover spoof rejection and trusted proxy normalization.

### B. [x] WebSocket Origin Validation
- **Requirement:** WebSocket upgrades must validate the `Origin` header against a configurable allow-list, rejecting cross-origin upgrades by default.
- **Status:** Resolved. Integration tests now cover default cross-origin rejection, same-origin success, and allowlist opt-in.

### C. Graceful Shutdown for Long-Lived Connections
- **Requirement:** WebSocket and SSE connections must be notified of shutdown (via close frames or stream termination) before the drain timeout expires.
- **Missing Test:** A test verifying that active WebSocket connections receive a close frame during graceful shutdown.

### D. [x] Panic Recovery Integration
- **Requirement:** When `catch_panics` is enabled, a handler panic must result in a `500 Internal Server Error` response, not a connection reset.
- **Status:** Resolved. Integration tests now verify panic-to-`500` behavior over the facade.

### E. [x] Chunked Request Body Enforcement
- **Requirement:** The `BodyLimit` middleware must enforce limits regardless of transfer encoding (chunked or content-length).
- **Status:** Resolved. Integration tests now verify both chunked under-limit acceptance and over-limit rejection.

### F. [x] File Upload Path Confinement
- **Requirement:** `UploadFile::save_to` must validate that the target path is within a configured upload directory.
- **Status:** Resolved. Upload tests now cover invalid path rejection and the safe `save_to_dir` path.

### G. [x] Log Injection Prevention
- **Requirement:** Request paths, headers, and user-controlled values logged by the framework must be sanitized to prevent log forging.
- **Status:** Resolved. Logging regression tests now cover forged request IDs and other request-derived values.

### H. [x] CORS Cache Correctness
- **Requirement:** CORS responses must set `Vary` headers correctly to prevent CDN/proxy caching of origin-specific responses.
- **Status:** Resolved. Middleware tests now verify the full preflight `Vary` set and `Origin` on actual responses.

### I. [x] Multipart Temp File Cleanup on Error
- **Requirement:** When multipart parsing fails mid-stream, all already-created `SpooledTempFile` instances must be cleaned up.
- **Status:** Resolved. End-to-end upload tests now verify truncated multipart cleanup.

### J. Compression Memory Under Concurrency
- **Requirement:** The compression middleware must not buffer uncompressed + compressed copies simultaneously when possible, or must document memory limits.
- **Missing Test:** A stress test measuring peak memory usage with 100+ concurrent requests returning compressed responses.

### K. SSE Connection Resource Limits
- **Requirement:** SSE streams must have configurable connection limits to prevent unbounded resource consumption.
- **Missing Test:** A test verifying that the server rejects SSE connections beyond a configured limit.

### L. Multipart Blocking IO
- **Requirement:** File chunk writes during multipart parsing must use `spawn_blocking` to avoid blocking the async runtime.
- **Missing Test:** A test verifying that multipart parsing does not block the tokio runtime thread under high concurrency.

### M. Middleware Chain Performance
- **Requirement:** The middleware chain must minimize per-request allocation (Arc clones, boxed futures).
- **Missing Test:** A benchmark measuring request throughput with 5+ middleware layers vs. zero middleware to quantify overhead.
