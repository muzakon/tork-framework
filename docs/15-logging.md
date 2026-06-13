# 15. Logging

Tork logs through an injectable, context-aware `Logger` built on `tracing`. In
development the output is a colored, NestJS-style console; in production it is one
flat JSON object per line. HTTP requests, startup, and the application lifecycle are
logged automatically, and a request id correlates every line of a request.

This chapter assumes you have read [Extractors and dependency injection](04-extractors-and-dependency-injection.md)
and [Settings](13-settings.md).

## Logging in a service

Add a `Logger` field to a `#[derive(Inject)]` service; the macro gives it the
struct's name as its context. Build a record with a level method, attach fields,
and `emit`.

```rust
use tork::{Inject, Logger};

#[derive(Clone, Inject)]
pub struct PaymentService {
    logger: Logger,
}

impl PaymentService {
    pub async fn charge(&self, user_id: i64) -> tork::Result<()> {
        self.logger
            .info("Charging user")
            .field("user_id", user_id)
            .emit();
        Ok(())
    }
}
```

Development output:

```
[Tork] 19096  - 06/11/2026, 14:32:11   INFO [PaymentService] Charging user user_id=42 +5ms
```

Production output (`format = json`):

```json
{"timestamp":"2026-06-11T14:32:11Z","level":"INFO","service":"tork-api","context":"PaymentService","message":"Charging user","user_id":42,"request_id":"req-..."}
```

Nothing is logged until `emit` is called. `error` always attaches the error's type
name; the top-level message and source chain are configurable through
`LoggerConfig::error_detail(...)`. The most conservative default is
`ErrorLogDetail::TypeOnly`.

```rust
self.logger.error("Payment failed").error(&err).field("order_id", id).emit();
```

To include more detail explicitly:

```rust
use tork::{ErrorLogDetail, LoggerConfig};

let logger = LoggerConfig::new().error_detail(ErrorLogDetail::MessageOnly);
```

`ErrorLogDetail::FullChain` also includes the bounded `source()` chain (capped at
16 entries so a deep error cannot produce an unbounded record). Logged errors are
not redacted the way `5xx` client responses are, and an error's
`Display`/`source` text can carry sensitive data (a database driver may include a
connection string in its message). Keep secrets out of error messages unless you
have deliberately enabled that detail in logs.

A `Logger` injected directly into a handler has the default context; call
`for_context("...")` to set one, or `with_field(...)` to add a field to every line.

### Context overrides

The context is the struct name by default. Override it per field or per struct:

```rust
#[derive(Clone, Inject)]
pub struct OrderService {
    #[logger(context = "Orders")]
    logger: Logger,
}

#[derive(Clone, Inject)]
#[inject(context = "Billing")]
pub struct Invoices {
    logger: Logger,
}
```

## Automatic logs

With no configuration, logging is on. The framework logs:

- startup: a line per mapped route (`[RouterExplorer] Mapped {GET /users/{id}}`) and
  the bound address (`[Tork] Application is running on ...`);
- each request, after it completes (`[HTTP] GET /info 200`), with the method, path,
  route, status, duration, and request id;
- lifecycle and error events.

Every log emitted while handling a request carries that request's id, so a service
log and the HTTP log share one `request_id`. Turn the request log off with
`request_logs(false)` on the configuration.

## Configuration

Logging is on by default: a developer console on a terminal, JSON otherwise; the
level comes from `RUST_LOG` or `info`. Configure it with `App::logger`, typically
from settings.

```rust
use tork::{settings, LogFormat, LoggerConfig};

#[settings]
pub struct LoggingConfig {
    #[setting(default = "info")]   pub level: String,
    #[setting(default)]            pub format: LogFormat, // auto | pretty | compact | json
    #[setting(default = true)]     pub request_logs: bool,
}

impl LoggingConfig {
    pub fn to_logger(&self) -> LoggerConfig {
        LoggerConfig::new()
            .level(self.level.clone())
            .format(self.format)
            .service_name("my-api")
            .request_logs(self.request_logs)
    }
}

// In main, before serving:
let config = Config::load()?;
App::new().logger(config.logging.to_logger()) /* ... */;
```

`LogFormat::Auto` (the default) renders the console when standard output is a
terminal and JSON otherwise, so the same binary is readable in development and
structured in production.

## Spans

Group an operation's logs and carry its fields with a span. Enter one for a
synchronous scope, or wrap a future so the span stays active across `await`:

```rust
// Synchronous scope.
let _scope = self.logger.span("import").field("batch", id).enter();

// Around a future.
let order = self
    .logger
    .instrument("create_order")
    .field("user_id", user_id)
    .run(self.orders.create(input))
    .await?;
```

## File output

Add a rolling file sink alongside the console. Files are always structured JSON.

```rust
use tork::{FileLogConfig, LoggerConfig, Rotation};

LoggerConfig::new().file(
    FileLogConfig::new("./logs")
        .prefix("api")
        .rotation(Rotation::Daily)
        .non_blocking(true),
);
```

## OpenTelemetry

With the `otel` feature enabled (`tork = { version = "0.1", features = ["otel"] }`),
the request spans and logs export over OTLP to a collector:

```rust
use tork::{LoggerConfig, TelemetryConfig};

LoggerConfig::new()
    .telemetry(TelemetryConfig::new("http://localhost:4317").service_name("my-api"));
```

## Testing

Capture logs in tests with a `LogRecorder` on the test client and assert on them.

```rust
use tork::testing::{LogRecorder, TestClient};
use tork::assert_logs;

let recorder = LogRecorder::new();
let client = TestClient::builder(app).logger(recorder.clone()).build().await?;

client.get("/orders").send().await?;

assert!(recorder.contains_context("OrderService"));
assert_logs!(recorder, context = "OrderService", message = "Listing orders");
```

See [Testing](14-testing.md) for the rest of the test client.
