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

## 4. [x] Missing Rate Limiting (High Risk / Feature Deficit)
- **Risk:** The framework provides no built-in rate limiting middleware.
- **Vulnerability:** All endpoints are exposed to brute-force attacks (login, password reset), API abuse, and denial-of-service. Without rate limiting at the framework level, every application must implement its own, and many will ship without it.
- **Status:** RESOLVED. Added a NestJS-style rate limiter: `App::throttle(Throttle::new().policy(...).default(...))` defines named policies + a global default, and a `throttle` attribute on routers/routes (`throttle = "name"`, `throttle(limit = 3, ttl = 60)`, `throttle = "skip"`, `throttle = ["short", "long"]`) applies them with endpoint > router > global precedence, returning `429` + `Retry-After`. Custom keys via the `ThrottleKey` trait (default `ByIp`); pluggable in-memory or Redis store; fixed or sliding window; works on routes, SSE, and WebSockets. Covered by unit + integration tests; documented in docs/18-rate-limiting.md.

## 5. [x] UploadFile::save_to Path Traversal (High Security Risk)
- **Risk:** `save_to` now rejects absolute and parent-traversal targets, and `save_to_dir(dir, file_name)` provides the safe persistence API for normal uploads.
- **Status:** Resolved. Covered by regression tests for safe directory writes and invalid path rejection.

## 6. [x] CORS Wildcard + Credentials Reflects Any Origin (Medium Security Risk)
- **Risk:** Wildcard-plus-credentials now fails closed: no `Access-Control-Allow-Origin` is granted for that invalid configuration.
- **Status:** Resolved. Covered by regression tests for both preflight and actual-request behavior.

## 7. [x] Panic Boundary Disabled by Default (Medium Risk)
- **Risk:** `App::new()` now catches handler panics by default, with explicit opt-out via `propagate_panics()`.
- **Status:** Resolved. Covered by unit and integration tests for default `500` conversion and explicit panic propagation.

## 8. [x] No Response Body Size Limit (Medium Risk)
- **Risk:** The `RespBody` type ([`body.rs:L31-L97`](crates/tork-core/src/body.rs)) has no size limit enforcement. A handler returning `RespBody::stream(...)` can stream an unbounded amount of data to the client.
- **Vulnerability:** While the request body has `MAX_BODY_BYTES` (2 MiB), the response side has no equivalent protection. An endpoint streaming data (file download, SSE) could exhaust server memory and bandwidth, affecting all other connections.
- **Status:** RESOLVED (opt-in). `RespBody::stream_capped(body, max_bytes)` wraps a streaming body and errors the response once it emits more than the limit, so a runaway generated download cannot stream without end. SSE stays on the intentionally-unbounded `stream`. Covered by `capped_stream_errors_once_it_exceeds_the_limit`.

## 9. [x] CORS Missing `Vary` Headers for Preflight Negotiation (Medium Security Risk)
- **Risk:** Preflight responses now include `Vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers`.
- **Status:** Resolved. Covered by regression tests for full `Vary` coverage.

## 10. [x] Log Injection via Unsanitized Request Path (Medium Security Risk)
- **Risk:** Request path and other request-derived log fields are sanitized before console emission.
- **Status:** Resolved. Covered by log-forgery regression tests.

## 11. [x] OpenAPI Documentation UI Unprotected (Medium Security Risk)
- **Risk:** The OpenAPI spec (`/openapi.json`) and Scalar documentation UI (`/docs`) routes ([`docs.rs:L22-L31`](crates/tork-openapi/src/docs.rs), [`spec.rs:L86-L108`](crates/tork-openapi/src/spec.rs)) are registered as plain GET routes with no authentication or authorization checks.
- **Vulnerability:** An attacker accesses `/openapi.json` to discover the full API surface including parameter names, types, error codes, and internal route structures. This information disclosure aids in crafting targeted attacks. The routes are added after the developer's router configuration, making them difficult to gate behind auth middleware.
- **Status:** RESOLVED. `OpenApi::protect(predicate)` gates the spec and docs routes; a rejected request gets a `404` (hiding the routes exist). Gate on a token, internal network, or env flag, or omit `.openapi(...)` in production. Global middleware also covers these routes. Covered by `protect_gates_the_docs_and_spec_routes`.

## 12. [x] Scalar CDN Without Subresource Integrity (Medium Security Risk / Supply Chain)
- **Risk:** The documentation UI loads the Scalar API reference library from `https://cdn.jsdelivr.net/npm/@scalar/api-reference` ([`docs.rs:L17`](crates/tork-openapi/src/docs.rs)) without an SRI hash.
- **Vulnerability:** If the CDN is compromised or a supply-chain attack occurs on the `@scalar/api-reference` npm package, arbitrary JavaScript executes in every browser visiting the documentation page, potentially stealing API tokens or performing actions on behalf of the user.
- **Status:** RESOLVED. The Scalar bundle is now pinned to an exact version and file (`@scalar/api-reference@1.59.3/dist/browser/standalone.min.js`) and loaded with a Subresource Integrity hash (`sha384-...`) plus `crossorigin="anonymous"`, so the browser refuses the script if its bytes differ from the pinned hash. Covered by `render_html_pins_the_cdn_and_adds_integrity`.

## 13. Error Chain Leakage in Structured Logs (Medium Security Risk)
- **Risk:** The `LogEvent::error` method ([`event.rs:L31-L52`](crates/tork-core/src/logging/event.rs)) serializes the full error chain (type name, message, and all `source()` messages) into log output.
- **Vulnerability:** If an error's `Display` implementation includes sensitive data (database connection strings with passwords, API keys), this data appears in structured logs. Database drivers commonly include connection strings in error messages.
- **Status:** PARTIALLY RESOLVED. The logged source chain is now capped at 16 entries (`MAX_ERROR_CHAIN`), bounding record size, and the API docs warn that error messages can carry sensitive data and should not contain secrets (logged errors are not redacted like `5xx` client responses). Generic automatic redaction of secrets in error text is not possible without knowing what is sensitive, so it stays the developer's responsibility. Covered by `error_chain_is_truncated_at_the_cap`.

## 14. [x] Request ID Logged Without Sanitization (Medium Security Risk)
- **Risk:** Client-supplied request IDs are sanitized before they reach console logs.
- **Status:** Resolved. Covered by log-forgery regression tests.

## 15. [x] Multipart `temp_dir` Configuration Not Propagated (Medium Risk)
- **Risk:** `UploadConfig` exposes a `temp_dir` field ([`multipart.rs:L88-L91`](crates/tork-core/src/multipart.rs)), but the actual `SpooledTempFile` creation uses the system default temp directory, ignoring this setting.
- **Bug:** Declaring `temp_dir("/mnt/fast-ssd")` has no effect. In containerized environments where the default `/tmp` is a memory-backed `tmpfs`, large uploads spill to RAM instead of disk, causing OOM kills.
- **Status:** RESOLVED. `temp_dir` now flows through `ResolvedConfig` into `tempfile::spooled_tempfile_in`, so spilled uploads land in the configured directory (avoiding `tmpfs` OOM in containers). Covered by the upload e2e tests over the spool path.

## 16. [x] BodyLimit Middleware Bypass via Chunked Transfer Encoding (Medium Risk)
- **Risk:** `BodyLimit` now consumes and bounds request bodies regardless of transfer encoding before the handler runs.
- **Status:** Resolved. Covered by regression tests for chunked under-limit and over-limit requests.

## 17. [x] No Per-Client WebSocket Connection Limit (Medium Risk)
- **Risk:** The WebSocket upgrade path has no limit on the number of concurrent connections from a single client IP.
- **Vulnerability:** A single client can open thousands of WebSocket connections, exhausting server memory, file descriptors, and task capacity. This is a denial-of-service vector, especially without rate limiting.
- **Status:** RESOLVED. `WebSocketConfig::max_connections_per_ip` caps concurrent connections per client IP via a shared app-wide counter; the cap is enforced before the upgrade and over-limit clients get `429`. The slot is held by a permit released when the connection ends. Covered by `ws_ip_limiter_caps_per_ip_and_releases_on_drop`.

## 18. [x] Hub Room Memory Leak (Medium Risk / Resource Exhaustion)
- **Risk:** The `Hub` ([`realtime.rs:L16-L59`](crates/tork-core/src/realtime.rs)) stores rooms in a `HashMap` behind a `Mutex`. Once created, rooms are never evicted.
- **Bug:** In a long-running server where rooms are dynamically named (e.g., per-user or per-session rooms), the `HashMap` grows unboundedly. Each room holds a `broadcast::Sender` with a 256-message buffer. Over time this causes unbounded memory growth.
- **Status:** RESOLVED. `Hub::room` evicts dead rooms (no subscribers and no outstanding `Room` handles, detected via `Arc::strong_count` + `receiver_count`) whenever a new room is created, bounding growth for dynamically named rooms. Covered by `dead_rooms_are_evicted_when_a_new_room_is_created` and `rooms_with_live_handles_or_subscribers_are_kept`.

## 19. [x] Missing Security Headers Middleware (Medium Risk / Feature Deficit)
- **Risk:** The framework provides no middleware for standard security headers (`Strict-Transport-Security`, `X-Content-Type-Options`, `X-Frame-Options`, `X-XSS-Protection`, `Content-Security-Policy`, `Referrer-Policy`).
- **Vulnerability:** Applications must implement these headers manually. Without them, applications are vulnerable to clickjacking, MIME-sniffing attacks, and will fail security audits.
- **Status:** RESOLVED. Added the `SecurityHeaders` middleware (`tork::middleware::SecurityHeaders`) with secure defaults — `Strict-Transport-Security`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` — plus a builder for HSTS/frame-options/referrer-policy and an opt-in `Content-Security-Policy`. Each header is set only when absent so handlers can override. Single-registration (`Reject`). Covered by unit tests and `security_headers_*` integration tests; documented in docs/08-middleware.md.

## 20. [x] WebSocket Upgrade No Handshake Timeout (Medium Risk)
- **Risk:** The WebSocket upgrade process ([`ws.rs:L461-L488`](crates/tork-core/src/ws.rs)) awaits `on_upgrade.await` without a timeout.
- **Vulnerability:** A slow or malicious client can initiate a WebSocket upgrade and then stall the TCP connection indefinitely before completing the handshake. The server task waits forever, consuming a connection slot.
- **Status:** RESOLVED. `WebSocket::accept` bounds `on_upgrade` with a handshake timeout (default 10s, configurable via `WebSocketConfig::handshake_timeout`); a client that stalls the upgrade is dropped instead of holding the slot. Covered by `route_config_overrides_app_defaults_for_new_limits` (config) and the timeout path in `accept`.

## 21. [x] Graceful Shutdown Drops In-Flight WebSocket Connections (Medium Risk)
- **Risk:** The server's graceful shutdown ([`server.rs:L76`](crates/tork-core/src/server.rs)) drains HTTP connections with a timeout but does not explicitly close WebSocket connections.
- **Bug:** WebSocket connections are long-lived and will not complete within the 15-second drain timeout. They are forcibly terminated, causing clients to receive unexpected disconnects without proper close frames.
- **Status:** RESOLVED. The app holds a `watch::Sender<bool>` shutdown channel; the receiver is injected into WebSocket connections through app state (`WsShutdown`). When the server begins draining it calls `AppInner::begin_ws_shutdown`, and `WebSocketConn::recv` races every read against that signal: on shutdown it sends a `1001 Going Away` close frame and returns `None`, so the handler completes and the client disconnects cleanly instead of being abruptly dropped. Covered by `graceful_shutdown_closes_live_websockets`.

## 22. [x] UploadFile::save_to Symlink Attack (Medium Security Risk)
- **Risk:** Existing symlink targets are rejected and destination writes use `create_new(true)` to avoid overwrite-through-existing-target behavior.
- **Status:** Resolved. Covered by the symlink attack regression test.

## 23. [x] Compression Middleware Doesn't Set `Vary` on Non-Gzip Path (Low-Medium Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L60-L119`](crates/tork-core/src/middleware/compression.rs)) sets `Vary: Accept-Encoding` only when compression is actually applied.
- **Bug:** When gzip is disabled or the body is too small, the `Vary` header is not set. Intermediate caches may serve a compressed response to a client that doesn't support gzip, or an uncompressed response to a client that expects compression.
- **Status:** RESOLVED. `Compression` now appends `Vary: Accept-Encoding` to every eligible response when gzip is enabled (not only the compressed ones), so a cache cannot serve a compressed body to a client that did not ask for it. Covered by `compression_sets_vary_even_when_not_compressing`.

## 24. [x] TrustedHost Port Stripping Fails for IPv6 (Low-Medium Risk)
- **Risk:** The `TrustedHost` middleware ([`trusted_host.rs:L63-L64`](crates/tork-core/src/middleware/trusted_host.rs)) strips ports via `value.split(':').next()`.
- **Bug:** IPv6 addresses like `[::1]:8080` are not handled correctly. The split on `:` produces `[` as the first segment, which won't match any pattern. All IPv6-based `Host` headers are silently rejected.
- **Status:** RESOLVED. `TrustedHost` parses the host with `strip_port`, which keeps a bracketed IPv6 literal (`[::1]:8080` -> `[::1]`) intact instead of mangling it to `[`. Covered by `strip_port_handles_names_ipv4_and_bracketed_ipv6` and `trusted_host_accepts_bracketed_ipv6_with_a_port`.

## 25. [x] WebSocket Disconnect Hook Fires Detached Task (Low-Medium Risk)
- **Risk:** `WebSocketConn::Drop` ([`ws.rs:L501-L519`](crates/tork-core/src/ws.rs)) spawns a detached tokio task to fire disconnect hooks because `Drop` cannot be async.
- **Vulnerability:** If the tokio runtime is shutting down (e.g., during graceful shutdown), `Handle::try_current()` may fail, silently skipping disconnect hooks. Metrics, cleanup logic, and audit trails in disconnect hooks will not execute during shutdown.
- **Status:** RESOLVED. The `on_ws_disconnect` hooks now fire inline (awaited in the connection's own task) on every `recv`-driven close path — peer close, idle timeout, stream end, and the new graceful-shutdown close — guarded by a `hooks_fired` flag so they run exactly once. `Drop` keeps the detached-task path only as a fallback for an abrupt handler exit (early return or panic mid-stream) and skips when the hooks already ran. Because the hooks are awaited while the runtime is still alive, graceful shutdown no longer loses them. Covered by `chat_broadcasts_to_room_and_fires_lifecycle_hooks` (disconnect hooks on close) and `graceful_shutdown_closes_live_websockets`.

## 26. [x] Settings Loader Environment Variable Race Condition (Low-Medium Risk)
- **Risk:** The `SettingsLoader` uses `dotenvy` to load `.env` files into the process environment ([`settings.rs:L187-L194`](crates/tork-core/src/settings.rs)) and reads environment variables with `std::env::var`.
- **Vulnerability:** In multi-threaded test runners or applications that reload config at runtime, `std::env::set_var` and `std::env::remove_var` are not thread-safe (documented in Rust stdlib). Concurrent config loading can read corrupted environment state.
- **Status:** Resolved. Tests that manipulate environment variables now acquire a global `Mutex` guard (`ENV_LOCK`) before calling `set_var`/`remove_var`, preventing concurrent env var access in multi-threaded test runners.

## 27. [x] SSE Stream Errors Logged to stderr (Low-Medium Risk)
- **Risk:** SSE stream errors ([`sse.rs:L393`](crates/tork-core/src/sse.rs)) are printed via `eprintln!` rather than the framework's structured logging system.
- **Vulnerability:** In production, these errors bypass log aggregation, alerting, and structured output. Operators cannot detect SSE failure rates or correlate them with trace IDs.
- **Status:** RESOLVED. SSE stream errors and oversized-event skips now go through `tracing` (`tracing::error!`/`tracing::warn!` on the `tork` target) instead of `eprintln!`, so they reach the structured subscriber, log aggregation, and trace correlation.

## 28. [x] Multipart Form Field Name Collision (Low-Medium Risk)
- **Risk:** The `MultipartForm` parser ([`multipart.rs:L526-L534`](crates/tork-core/src/multipart.rs)) uses `take_form_value` which takes the first field with a given name.
- **Bug:** If a multipart form contains multiple fields with the same name but different types (e.g., `count` as both text and file), the first `take_form_value` call consumes the text field, and subsequent calls silently return `None`. There is no type-safe enforcement that a field name maps to exactly one expected type.
- **Status:** Resolved. `take_form_value` and `take_file_bytes` now detect type conflicts: accessing a text field as a file (or vice versa) returns an `422 Unprocessable Entity` error instead of silently returning `None`.

## 29. [x] Error Response Does Not Set `Cache-Control` (Low-Medium Risk)
- **Risk:** The `Error` type's `IntoResponse` implementation ([`error.rs:L364-L401`](crates/tork-core/src/error.rs)) does not set `Cache-Control: no-store` on error responses.
- **Vulnerability:** Intermediate proxies or browsers may cache `4xx`/`5xx` error responses. A cached `401 Unauthorized` can prevent legitimate retry attempts, and a cached `500 Internal Server Error` can mask ongoing issues.
- **Status:** RESOLVED. `Error::into_response` sets `Cache-Control: no-store`, so proxies and browsers do not cache `4xx`/`5xx` responses (a cached `401` would block retries; a cached `500` would mask recovery). Covered by `error_responses_are_not_cacheable`.

## 30. [x] HTML Escape Missing Single Quote (Low-Medium Risk)
- **Risk:** The `html_escape` function in the OpenAPI docs ([`docs.rs:L55-L61`](crates/tork-openapi/src/docs.rs)) escapes `&`, `<`, `>`, and `"`, but does not escape single quotes (`'`) or backticks.
- **Vulnerability:** If `spec_url` were ever placed inside a single-quoted attribute, an attacker could inject `onload=alert(1)`. Currently mitigated by double-quote template usage, but this is a defense-in-depth gap.
- **Status:** RESOLVED. `html_escape` now also escapes single quotes (`&#x27;`) and backticks (`&#x60;`), so an interpolated value cannot break out of a single-quoted attribute even if the template changes. Covered by `html_escape_replaces_reserved_characters`.

## 31. [x] Panic Message Leakage Through Hooks (Low-Medium Risk)
- **Risk:** When `catch_panics` is enabled, caught panics fire `on_panic` hooks with the panic message ([`service.rs:L74-L83`](crates/tork-core/src/service.rs)). The `panic_message` function extracts the `&str` or `String` from the panic payload.
- **Vulnerability:** While the panic message does not reach the client, it is available to all registered panic hooks. If any hook logs or transmits this message, internal details (e.g., `panic!("db_password is {secret}")`) are exposed in logs.
- **Status:** Resolved. `panic_message` now truncates panic payloads to 1024 characters before passing them to hooks, limiting the data exposure if a hook logs or transmits the message.

## 32. [x] Trace Middleware Logs Request Path Before Routing (Low-Medium Risk)
- **Risk:** The `Trace` middleware ([`trace.rs:L32-L52`](crates/tork-core/src/middleware/trace.rs)) logs the request path before routing occurs.
- **Vulnerability:** Even requests that result in a `404` or `405` generate log entries with their full path. An attacker can use this to enumerate paths by observing which paths generate log entries, even if those paths are protected by middleware.
- **Status:** Resolved. Pre-routing logs now use `debug` level instead of `info`, so they are hidden from production log aggregation at the default `info` threshold. The framework's built-in HTTP log still covers matched routes at `info`.

---

## 33. BearerToken Extractor Allocates Per Request (Low Risk)
- **Risk:** `BearerToken` ([`header.rs:L118`](crates/tork-core/src/extract/header.rs)) clones the token string into an owned `String` on every request.
- **Optimization:** For high-throughput APIs, this creates significant allocation pressure. The token could be a zero-copy `&str` borrow from the request headers, but the current design requires ownership because the headers outlive the extractor.
- **Status:** Untested for allocation overhead.

## 34. [x] Compression Buffer Allocates Before Checking Size (Low Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L85-L88`](crates/tork-core/src/middleware/compression.rs)) calls `into_body_bytes` (which buffers the entire response body) before checking if the body exceeds `minimum_size`.
- **Optimization:** For small responses that don't meet the compression threshold, the body is fully buffered into memory unnecessarily. Streaming responses should avoid buffering entirely when compression won't be applied.
- **Status:** Resolved. When the response advertises a `Content-Length` below `minimum_size`, the body is now passed through without buffering at all, avoiding unnecessary allocation for small responses.

## 35. [x] SSE Heartbeat Allocates New `Bytes` Every Interval (Low Risk)
- **Risk:** The SSE heartbeat frame ([`sse.rs:L29`](crates/tork-core/src/sse.rs)) is a static `&[u8]`, but each heartbeat emission wraps it in `Bytes::from_static`. While this is cheap, under extremely high SSE connection counts (10k+), the per-heartbeat framing and poll wakeups add non-trivial overhead.
- **Optimization:** A pre-encoded shared `Bytes` value would avoid repeated static wrapping.
- **Status:** Resolved. `HEARTBEAT_BYTES` is now a `LazyLock<Bytes>` initialized once and cloned on each heartbeat tick, eliminating the per-tick wrapper allocation.

## 36. [x] Path Normalization Only Handles Trailing Slashes (Low Risk)
- **Risk:** The router matcher ([`matcher.rs:L124-L131`](crates/tork-core/src/router/matcher.rs)) normalizes only trailing slashes. Double slashes (`//`), encoded slashes (`%2F`), or case differences in path segments are not normalized.
- **Vulnerability:** Path traversal attempts using `//` or `..` may bypass route matching or reach unintended handlers depending on upstream proxy behavior.
- **Status:** Resolved. `Matcher::find` now collapses double slashes (`//api//users` → `/api/users`) as an additional fallback before returning `NotFound`.

## 37. [x] StateMap Silent Value Replacement (Low Risk)
- **Risk:** The `StateMap` ([`state.rs:L29-L38`](crates/tork-core/src/state.rs)) uses `TypeId` for keying and silently replaces any existing value of the same type on `insert`.
- **Vulnerability:** If a middleware, test override, or lifespan inserts a value of a type that is already registered, the previous value is silently dropped. This could lead to state pollution where a security-relevant value (like a database pool or auth configuration) is unintentionally replaced.
- **Status:** Resolved. `StateMap::insert` now emits a `tracing::warn!` when overwriting an existing entry, so accidental state pollution is visible in logs.

## 38. [~] Validation Body Buffering Depth (Low Risk)
- **Risk:** The `Valid<T>` extractor ([`valid.rs:L27-L43`](crates/tork-core/src/extract/valid.rs)) deserializes the body and then runs validation, meaning the full body is buffered in memory. There is no mechanism to short-circuit if deserialization consumes significant memory for deeply nested JSON.
- **Vulnerability:** An attacker sends a deeply nested JSON payload that passes `MAX_BODY_BYTES` but causes stack overflow or excessive allocation during deserialization. Mitigated by `MAX_BODY_BYTES` and `serde_json`'s own limits.
- **Status:** Mitigated by `MAX_BODY_BYTES` (2 MiB) and `serde_json`'s default recursion limit (128 levels).

## 39. [x] No Null Byte Injection Protection in Router (Low Risk)
- **Risk:** The `Matcher::find` method ([`matcher.rs:L73-L111`](crates/tork-core/src/router/matcher.rs)) receives the request path from `head.uri.path()` and passes it directly to `matchit::Router::at()` without checking for null bytes (`\0`).
- **Vulnerability:** If a reverse proxy forwards a request with a null byte in the path, the router might match it differently than expected. Mitigated by HTTP parsers typically rejecting null bytes in URIs.
- **Status:** Resolved. `Matcher::find` rejects paths containing `\0` with `Match::NotFound`, closing the defense-in-depth gap.

## 40. [x] Dependency Version Audit Required (Informational)
- **Risk:** Several dependencies should be checked against the RustSec advisory database:
  - `tokio-tungstenite = "0.24"` — older version; latest is 0.26+
  - `multer = "3.1"` — check for recent advisories
  - `matchit = "0.9"` — check for path traversal advisories
  - `hyper = "1.10"` — check for HTTP/2 and request smuggling advisories
- **Vulnerability:** A known vulnerability in any dependency could be exploited directly.
- **Status:** Resolved. `cargo audit` reports 0 known vulnerabilities across all 312 crate dependencies.

## 41. [x] Multipart Text Fields No Per-Field Size Limit (Low Risk)
- **Risk:** The multipart parser ([`multipart.rs:L511-L518`](crates/tork-core/src/multipart.rs)) reads entire text field values into `String` via `field.text()` without a per-field size limit.
- **Vulnerability:** A single text field could contain up to `max_body_size` (default 16 MiB) of data. While bounded by the total body limit, per-field limits would provide better defense-in-depth.
- **Status:** Resolved. Text fields now have a configurable per-field limit (`UploadConfig::max_text_field_size`, default 1 MiB), enforced before the field value is returned.

## 42. [~] Route Metadata Could Inject Into OpenAPI JSON (Low Risk)
- **Risk:** Route summaries, descriptions, and tags ([`spec.rs:L117-L204`](crates/tork-openapi/src/spec.rs)) are serialized directly into the OpenAPI JSON document.
- **Vulnerability:** If a developer dynamically generates route descriptions from user input (unusual but possible), and the Scalar UI renders these as HTML, XSS could result. Currently mitigated because values come from developer-authored code and `serde_json` escapes strings.
- **Status:** Requires unusual dynamic route generation from user input.

## 43. [~] Test Client Bypasses Security Validation (Low Risk)
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

## 48. [x] SpooledTempFile Not Cleaned Up on Multipart Parse Error (Medium Resource Leak)
- **Risk:** The multipart parser ([`multipart.rs:L475-L519`](crates/tork-core/src/multipart.rs)) creates `SpooledTempFile` instances for each file field as data arrives. If a later field fails to parse (e.g., the body is truncated or a field exceeds limits), the already-created temp files are dropped.
- **Bug:** `SpooledTempFile::drop` closes the file handle, but if the file was spilled to disk, the OS may not immediately reclaim the disk space. Under sustained error conditions (e.g., an attacker sending malformed multipart bodies), temp files accumulate faster than the OS reclaims them.
- **Status:** RESOLVED (by design + test). The spill path uses `tempfile`'s anonymous temp file (`spooled_tempfile`/`spooled_tempfile_in`), which is unlinked from the filesystem the moment it is created. There is no named path to leak: once the `SpooledTempFile` is dropped, its last file descriptor closes and the OS reclaims the inode and disk space immediately, even on an early-return error path. Cleanup on a truncated/failed parse is exercised end-to-end by the upload tests (see requirement I).

## 49. [x] LogRecorder Accumulates Records Indefinitely (Low Resource Leak)
- **Risk:** `LogRecorder` ([`recorder.rs:L55-L68`](crates/tork-core/src/testing/recorder.rs)) pushes every `LogRecord` into a `Vec` behind a `Mutex`. There is no eviction, no max capacity, and no `clear()` method.
- **Bug:** In long-running tests or integration tests that generate many log lines, the `Vec` grows unboundedly. This is a test-only concern but can cause test OOM failures.
- **Status:** Resolved. `LogRecorder::clear()` allows tests to discard accumulated records between test phases.

## 50. [x] Compression Buffers Entire Response Before Compressing (Medium Memory Risk)
- **Risk:** The `Compression` middleware ([`compression.rs:L85-L88`](crates/tork-core/src/middleware/compression.rs)) calls `into_body_bytes(response)` which collects the entire response body into a single `Bytes` buffer before checking size and compressing.
- **Bug:** A 10 MiB JSON response is first buffered entirely into memory, then gzip-compressed into another ~1-3 MiB buffer. Peak memory usage is ~13 MiB per concurrent request with compression enabled. Under high concurrency, this can exhaust memory.
- **Status:** RESOLVED. `Compression` now has a `maximum_size` cap (default 8 MiB, configurable, `usize::MAX` to lift). When a response advertises a `Content-Length` over the cap, the body is passed through without being buffered at all; otherwise a body that turns out larger than the cap is returned uncompressed (skipping the second gzip buffer). This bounds per-request peak memory and stops a large streaming download from being pulled fully into memory just to decline compression. Covered by `compression_skips_bodies_over_the_maximum_size` and `content_length_parses_only_valid_values`.

## 51. [x] SSE Stream Holds Pinned BoxStream + Interval Indefinitely (Medium Memory Risk)
- **Risk:** Each SSE response ([`sse.rs:L337-L345`](crates/tork-core/src/sse.rs)) creates an `SseBody` that holds a pinned `BoxStream`, an optional `Interval`, an optional `Sleep` timer, and an optional `Bytes` done event.
- **Bug:** These allocations persist for the entire lifetime of the SSE connection (which can be hours). With 10,000 concurrent SSE connections, this is 10,000 pinned streams + 10,000 interval timers consuming memory and waking up periodically.
- **Status:** RESOLVED. `App::max_sse_connections(n)` installs an `SseLimiter` (a semaphore) into app state. The `#[sse]`/`#[post_sse]` generated handlers acquire an owned permit through `__sse_into_response`; the permit is held by the `SseBody` for the stream's lifetime and released on drop, so a freed slot is reusable. When the cap is reached, further SSE requests are rejected with `503 Service Unavailable` instead of opening another unbounded stream. With no cap configured, streams remain unbounded (unchanged default). Covered by `sse_limiter_caps_concurrent_permits_and_frees_them_on_drop` and `sse_connection_limit_rejects_over_the_cap`.

## 52. [~] Hook Event Cloning Per Request (Low Concurrency Overhead)
- **Risk:** Every request that triggers hooks ([`app.rs:L682-L704`](crates/tork-core/src/app.rs)) clones `RequestInfo`, `ResponseEvent`, and `ErrorEvent` structs for each hook invocation. The `RequestInfo` clone is O(1) (Arc clones), but the `ResponseEvent` includes a `StatusCode` and `Duration`.
- **Optimization:** Events could be shared via `Arc` instead of cloned per hook invocation, especially when multiple hooks observe the same request.
- **Status:** Low practical impact; hooks are typically 1-3 per app.

## 53. [x] StateMap Entries Never Evicted (Low Memory Risk)
- **Risk:** `StateMap` ([`state.rs:L29-L31`](crates/tork-core/src/state.rs)) holds `Arc<dyn Any + Send + Sync>` values keyed by `TypeId`. Once inserted, values are never removed or replaced unless the same type is re-inserted.
- **Bug:** In applications that dynamically register state (e.g., per-tenant resources), the map grows monotonically. There is no TTL, no eviction, and no capacity limit.
- **Status:** Resolved. `StateMap::remove::<S>()` allows explicit eviction of state entries when they are no longer needed.

## 54. [~] WebSocket Connection Arc Clone Overhead (Low Concurrency Overhead)
- **Risk:** Each WebSocket connection ([`ws.rs:L476-L478`](crates/tork-core/src/ws.rs)) clones `Arc<WsHooks>` and captures it in the connection struct. The `WsHooks` contains `Vec<WsConnectHook>` and `Vec<WsDisconnectHook>`.
- **Optimization:** With many concurrent WebSocket connections and multiple hooks, each connection holds a strong reference to the same hooks vec. This is correct but creates Arc reference count contention.
- **Status:** Low practical impact; hooks are typically 1-2 per app.

## 55. [x] Blocking IO in Multipart Spool Write (Medium Concurrency Risk)
- **Risk:** The multipart parser ([`multipart.rs:L497-L499`](crates/tork-core/src/multipart.rs)) writes file chunks to `SpooledTempFile` synchronously inside an async context (within the `while let Some(chunk)` loop). The `write_all` call is blocking IO.
- **Bug:** When the temp file spills to disk, `write_all` performs synchronous disk IO on the tokio runtime thread. Under high concurrency, this blocks the runtime thread and degrades throughput for all concurrent requests.
- **Status:** RESOLVED. `MultipartForm::parse` now buffers chunks and flushes them to the spool via `spawn_blocking` (`spool_flush`), batching at a 256 KB threshold, so disk writes no longer block the async runtime. The final flush also rewinds off-runtime.

## 56. [~] Settings Loader Allocates Multiple Figment Instances (Low Memory Risk)
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

### C. [x] Graceful Shutdown for Long-Lived Connections
- **Requirement:** WebSocket and SSE connections must be notified of shutdown (via close frames or stream termination) before the drain timeout expires.
- **Status:** Resolved for WebSocket (see #21): active connections receive a `1001 Going Away` close frame at the start of drain, verified by `graceful_shutdown_closes_live_websockets`. SSE streams end when the client disconnects or via `client_timeout`; the new `max_sse_connections` cap (see #51) bounds their count.

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

### J. [x] Compression Memory Under Concurrency
- **Requirement:** The compression middleware must not buffer uncompressed + compressed copies simultaneously when possible, or must document memory limits.
- **Status:** Resolved (see #50). `Compression` has a documented `maximum_size` cap (default 8 MiB): bodies over the cap are passed through uncompressed, and a `Content-Length` over the cap skips buffering entirely, bounding per-request peak memory. Covered by `compression_skips_bodies_over_the_maximum_size`.

### K. [x] SSE Connection Resource Limits
- **Requirement:** SSE streams must have configurable connection limits to prevent unbounded resource consumption.
- **Status:** Resolved. `App::max_sse_connections(n)` caps concurrent SSE streams and rejects requests over the cap with `503` (see #51), verified by `sse_connection_limit_rejects_over_the_cap`.

### L. Multipart Blocking IO
- **Requirement:** File chunk writes during multipart parsing must use `spawn_blocking` to avoid blocking the async runtime.
- **Missing Test:** A test verifying that multipart parsing does not block the tokio runtime thread under high concurrency.

### M. Middleware Chain Performance
- **Requirement:** The middleware chain must minimize per-request allocation (Arc clones, boxed futures).
- **Missing Test:** A benchmark measuring request throughput with 5+ middleware layers vs. zero middleware to quantify overhead.
