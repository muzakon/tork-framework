# Tork Pre-Production Red-Team Security Audit

A pre-deployment adversarial pass over the framework: real attacks were fired at a
realistic Tork app and each candidate confirmed or refuted by a reproducing
exploit. Confirmed framework-level issues were fixed and locked in with regression
tests. The regression suite lives in
[`crates/tork/tests/security_redteam.rs`](crates/tork/tests/security_redteam.rs);
precise unit-level checks live next to the code they cover.

Legend: **Fixed** (was exploitable, now closed) · **Safe** (attacked, held) ·
**By design** (bounded, app responsibility documented).

---

## Fixed

### 1. SSE field injection via `event` name / `id` — High
- **Where:** `crates/tork-core/src/sse.rs` `encode_event`.
- **Bug:** the `event` and `id` fields were written to the wire raw, with no newline
  handling (only `data`/`comment` were line-split). A user-controlled event name or
  id containing `\n`/`\r` could inject arbitrary SSE fields or whole events — e.g.
  an event name `ping\nevent: admin\ndata: spoofed` forges a second event on the
  client's stream. The natural vector is any app that streams a user/DB value as an
  event name or id (a display name, a record id, an echoed `Last-Event-ID`).
- **Fix:** `push_single_line` strips `\r`/`\n` (and NUL from `id`, per the SSE spec)
  from both single-line fields.
- **Tests:** `sse::tests::event_name_and_id_cannot_inject_extra_fields`.

### 2. No slowloris / header-read timeout — Medium
- **Where:** `crates/tork-core/src/server.rs` (the `auto::Builder`).
- **Bug:** the server set no header-read or request-read timeout, so a client that
  opened a connection and dribbled header bytes tied up a spawned task/FD
  indefinitely. The `Timeout` middleware is opt-in and only wraps the handler, not
  the connection read.
- **Fix:** a default HTTP/1 `header_read_timeout` (30s,
  `DEFAULT_HEADER_READ_TIMEOUT`) is wired onto the connection builder with a
  `TokioTimer`. Configurable via `App::header_read_timeout(Duration)` /
  `App::without_header_read_timeout()`. Secure by default.
- **Tests:** `security_redteam::slowloris_slow_header_client_is_dropped` (real
  socket; asserts the connection is held for ~the timeout, then dropped).

### 3. Multipart field-count amplification — Low-Medium
- **Where:** `crates/tork-core/src/multipart.rs` parse loop.
- **Bug:** the parser was bounded only by the 16 MiB total body, so thousands of
  tiny fields amplified `Vec`/`String` allocations and parser work per request.
- **Fix:** `UploadConfig::max_fields` (default 1000) caps the total number of parts;
  exceeding it returns `422 TOO_MANY_FIELDS`.
- **Tests:** `security_redteam::multipart_field_flood_is_rejected`.

### 4. Unbounded per-message memory on WS/SSE — Medium (secure-by-default)
- **Where:** `crates/tork-core/src/ws.rs` (`WebSocketConfig::to_tungstenite`),
  `crates/tork-core/src/sse.rs` (`SseConfig`).
- **Bug:** WS `max_message_size`/`max_frame_size` were unset by default, falling
  back to tungstenite's 64 MiB; SSE `max_event_size` was unset (unbounded per
  event). A single peer could make the server buffer a very large message/event.
- **Fix:** secure defaults — WS message/frame default to 1 MiB
  (`DEFAULT_WS_MAX_MESSAGE_SIZE`/`DEFAULT_WS_MAX_FRAME_SIZE`); SSE event defaults to
  256 KiB (`DEFAULT_MAX_EVENT_SIZE`). All overridable. Idle timeouts stay opt-in: a
  legitimately idle long-lived stream is normal, so a default idle-kill would break
  valid apps — the knob is documented instead.
- **Tests:** `ws::tests::websocket_config_builders_and_connect_info_accessors_work`
  (defaults applied), `security_redteam::websocket_oversized_message_is_rejected_by_default`.

### 5. Timing-unsafe credential comparison in the docs `protect` example — Low
- **Where:** `crates/tork-openapi/src/spec.rs` (`OpenApi::protect` doc example).
- **Bug:** the example compared a bearer token with `==`, which short-circuits on
  the first mismatching byte and leaks, over many requests, how many leading bytes
  matched.
- **Fix:** added `tork::security::constant_time_eq` and updated the example to use
  it; documented that token verification is app responsibility.
- **Tests:** `security::tests::equal_values_match_and_others_do_not`; the doc
  example runs as a doctest.

---

## Safe (attacked, held)

- **5xx information disclosure:** `Error::into_response` (`error.rs`) replaces any
  server-error message with the generic `Internal server error`, drops field
  details, logs the real cause server-side under a trace id, and sets
  `Cache-Control: no-store`. Confirmed by
  `security_redteam::server_error_does_not_leak_internal_detail` (a secret-bearing
  500 leaks neither the secret, the DSN, nor the file path). Note: the response
  `code` is developer-set, not user data — keep secrets out of custom error codes.
- **Credential echo:** a rejected `Authorization` value is never reflected in the
  error body. Confirmed by `security_redteam::rejected_authorization_is_not_echoed`.
- **JSON depth bomb:** `ensure_json_depth_within_limit` pre-scans the body and
  rejects nesting beyond 128 with `400` before the recursive parser runs. Confirmed
  by `security_redteam::deeply_nested_json_is_rejected_before_parsing` (5000 levels →
  400, server still alive) and `extract::body::tests::json_depth_guard_*`.
- **Response header injection (CRLF):** every header value Tork sets flows through
  `HeaderValue::from_str`, which rejects control characters and CRLF (CORS reflected
  origin, redirect `Location`, `Retry-After`, echoed request id). HTTP also forbids
  newlines in inbound header values, so a header-sourced value cannot carry one.
- **CORS:** exact-origin match, wildcard-with-credentials rejected, reflected origin
  validated (`cors.rs`).
- **Router:** null-byte rejection, `//` collapse, `%2F`/traversal handled
  (`router/matcher.rs`).
- **Upload path traversal / symlink:** `save_to_dir` confines writes and `save_to`
  rejects absolute/parent-traversal/symlink targets (covered by `uploads_e2e.rs`).
- **Body size:** `MAX_BODY_BYTES` (2 MiB) enforced incrementally before buffering
  (`extract/body.rs`).
- **Secrets:** `SecretString` masks `Debug`/`Display` and is not `Serialize`
  (`settings.rs`).
- **Panic = DoS:** `catch_panics` is on by default; a handler panic becomes a 500,
  and the panic message is truncated before reaching hooks.

---

## By design (bounded; documented)

- **Hub room growth (`realtime.rs`):** `Hub::room` evicts dead rooms (no live
  handle, no subscribers) on every new-room creation, so dynamically named rooms do
  not grow without bound. A room only persists while it has a live subscriber or
  handle — i.e. growth is bounded by concurrent live connections, which are
  separately capped (WS per-IP limit, SSE `max_sse_connections`). The "millions of
  unique rooms" attack reduces to the connection-count DoS, not a distinct leak.
- **Rate limiting / `ByIp` key:** opt-in; the default key uses the socket peer IP,
  not `X-Forwarded-For`. Behind a proxy, configure `ProxyHeaders` with a trusted
  allowlist or a custom `ThrottleKey`. Documented.
- **Path params used for filesystem/SQL:** the framework hands handlers a decoded
  string; using it to build a filesystem path or SQL is the app's responsibility
  (the upload `save_to*` APIs already guard the upload case).
