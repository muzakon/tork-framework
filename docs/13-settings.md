# 13. Settings

Most applications need configuration: a database URL, a port, feature flags,
secrets. Tork loads configuration once at startup into a typed, validated struct
and injects it like any other resource. There is no global singleton to import and
no re-parsing per request. If a value is missing or invalid, the application
refuses to start with a clear message.

This chapter assumes you have read [Extractors and dependency injection](04-extractors-and-dependency-injection.md)
and [Models and validation](05-models-and-validation.md).

## A first configuration

Annotate a struct with `#[settings]`. Each field becomes a setting; `#[setting(...)]`
adds defaults and validation.

```rust
use tork::{settings, SecretString};

#[settings(prefix = "APP", env_file = ".env")]
pub struct Config {
    #[setting(default = "Awesome API")]
    pub app_name: String,

    #[setting(email)]
    pub admin_email: String,

    #[setting(default = 50, ge = 1, le = 500)]
    pub items_per_user: u32,

    #[setting(secret)]
    pub api_key: SecretString,
}
```

The macro generates a `load()` method that reads the configured sources, fills in
defaults, validates the result, and returns `tork::Result<Config>`:

```rust
let config = Config::load()?;
```

## Loading at startup and injecting

Load the configuration once in your lifespan and store it as `Arc<Config>` so each
injection clones only a pointer. This is the Tork equivalent of a cached settings
singleton, without the hidden global.

```rust
use std::sync::Arc;
use tork::{LifespanContext, Resources, Result};

#[derive(Clone, Resources)]
pub struct AppState {
    #[resource]
    pub config: Arc<Config>,
    // other resources...
}

#[tork::lifespan]
impl AppState {
    async fn startup(_ctx: LifespanContext) -> Result<Self> {
        let config = Arc::new(Config::load()?);
        Ok(AppState { config })
    }
}
```

A handler, service, or repository then takes the configuration by type:

```rust
#[get("/info")]
pub async fn info(config: Arc<Config>) -> tork::Result<InfoOut> {
    Ok(InfoOut {
        app_name: config.app_name.clone(),
        items_per_user: config.items_per_user,
    })
}
```

`Arc<Config>` injects through a built-in extractor, so you never reach through a
state object to read settings.

## Sources and precedence

`#[settings]` merges several sources. From lowest to highest precedence:

1. struct defaults (`#[setting(default = ...)]`)
2. the base config file (`config_file = "..."`)
3. environment-specific files (`files = ["config/{env}.toml"]`)
4. the `.env` file (`env_file = "..."`)
5. environment variables (with the configured `prefix`)
6. a secrets directory (`secrets_dir = "..."`)
7. explicit overrides (set on the loader, useful in tests)

A higher source overrides a lower one for any key it sets. Real environment
variables win over `.env`, because loading `.env` never overwrites a variable that
is already set.

```rust
#[settings(
    prefix = "APP",
    env_file = ".env",
    config_file = "config/default.toml",
    files = ["config/{env}.toml"]
)]
pub struct Config { /* ... */ }
```

File paths are relative to the working directory the process runs in. Under
`cargo run -p <crate>` that directory is the workspace root, not the crate, so run
the binary from the crate's own directory (or pass absolute paths) when you rely on
a relative config file or `.env`. A missing file is skipped, so a config file is
optional; if no source supplies a value and the field has a default, the default is
used.

### Environment variables and nesting

With `prefix = "APP"`, the loader reads variables that start with `APP_`. Levels
of a nested setting are separated by a double underscore, while a single
underscore stays inside a field name. This avoids any ambiguity for field names
that themselves contain underscores.

```
APP_APP_NAME=Awesome API              # app_name
APP_ITEMS_PER_USER=100                # items_per_user
APP_SERVER__PORT=8080                 # server.port
APP_DATABASE__MAX_CONNECTIONS=20      # database.max_connections
```

Config files express nesting as tables, so the same `server.port` reads as:

```toml
[server]
port = 8080
```

### Environment-specific files

The `{env}` token in a `files` path is replaced with the current environment name,
read from `{PREFIX}_ENV` (here `APP_ENV`) and defaulting to `development`. So
`APP_ENV=production` loads `config/production.toml` on top of `config/default.toml`.

## Defaults

`#[setting(default = ...)]` supplies a value when no source provides one. A string
literal works for any field whose type can be built from a string (such as
`String` or `SecretString`); numbers and other expressions are used as written.

```rust
#[setting(default = "0.0.0.0")] pub host: String,
#[setting(default = 8000)]      pub port: u16,
```

Defaults are applied per field, so they compose through nested configurations.

## Validation

`#[setting(...)]` accepts the same constraints as `#[api_model]`: `ge`, `le`, `gt`,
`lt`, `min_length`, `max_length`, `email`, and `custom = path`. Validation runs at
the end of `load()`. Because configuration loads at startup, a violation aborts the
boot rather than failing a request later.

```rust
#[setting(default = 50, ge = 1, le = 500)]
pub items_per_user: u32,
```

A failed load returns an error with code `CONFIG_VALIDATION_ERROR` (or
`CONFIG_LOAD_FAILED` when a source cannot be parsed at all), listing the offending
fields. Returning it from `startup` stops the application before it serves traffic.

## Secrets

A field marked `#[setting(secret)]` uses the `SecretString` type. Its `Debug` and
`Display` render a fixed mask, so a secret never lands in a log line by accident.
The value is readable only through `expose()`, which makes every read site easy to
find.

```rust
#[settings(prefix = "APP")]
pub struct Config {
    #[setting(secret)]
    pub database_url: SecretString,
}

// let pool = connect(config.database_url.expose()).await?;
// println!("{:?}", config.database_url) prints SecretString("********")
```

Secrets are not validated and are never serialized, so a configuration carrying
them cannot be written back out.

## Nested configuration

Group related settings into their own `#[settings]` structs and mark the parent
field `#[setting(nested)]`. The whole tree still injects as one `Arc<Config>`.

```rust
#[settings]
pub struct ServerConfig {
    #[setting(default = "0.0.0.0")] pub host: String,
    #[setting(default = 8000)]      pub port: u16,
}

#[settings(prefix = "APP")]
pub struct Config {
    #[setting(default = "Awesome API")] pub app_name: String,
    #[setting(nested, default)]         pub server: ServerConfig,
}

// config.server.port
```

A bare `#[setting(nested)]` group is required: if no source supplies it, loading
fails. Add the `default` flag (`#[setting(nested, default)]`) to fill an absent
group from its own field defaults, so the whole config loads even when nothing
provides the group. This works when every field of the nested struct has a default
(so the group is itself default-constructible); keep a group required when it holds
a value with no sensible default, such as a database URL. The bare `default` flag
works on any field whose type has a `Default` (for example to make an `Option`
field optional or a counter start at zero).

## Overriding in tests

Tests do not need a config file or environment. Construct the value (its fields are
plain) and register it directly, or use the loader's `override_value` for the
highest-priority source:

```rust
let config = SettingsLoader::<Config>::new()
    .prefix("APP")
    .override_value("items_per_user", 5u32)
    .override_value("server.port", 0u16)
    .load()?;

let app = App::new()
    .state(Arc::new(config))
    .include_router(info_router::router())
    .build()?;
```

This is one reason the configuration is a resource rather than a global: a test
supplies its own without touching process state.

## Why not a global

A `LazyLock` static would also load configuration once, but it ties the value to
process state. That makes tests harder to isolate, complicates multi-tenant or
multi-application setups, and turns configuration into a hidden dependency. Loading
at startup and injecting `Arc<Config>` keeps the dependency explicit and the value
swappable, which is why it is the recommended path.
