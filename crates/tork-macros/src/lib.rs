//! Procedural macros for the Tork web framework.
//!
//! Every macro here emits code that refers to the public API through the `tork`
//! facade crate (for example `::tork::Router`), never through `tork-core`
//! directly. This lets generated code compile inside user crates that depend
//! only on `tork`.

use proc_macro::TokenStream;

mod api_model;
mod api_router;
mod app_error;
mod common;
mod dependency;
mod form_model;
mod inject;
mod lifespan;
mod main_macro;
mod middleware;
mod resources;
mod route;
mod settings;
mod sse;
mod websocket;

/// Marks the asynchronous entrypoint of a Tork application.
///
/// Rewrites an `async fn main` into a synchronous `main` that builds a
/// multi-threaded Tokio runtime and blocks on the original body. The function's
/// return type is preserved, so returning `tork::Result<()>` works as written.
///
/// # Example
///
/// ```ignore
/// #[tork::main]
/// async fn main() -> tork::Result<()> {
///     App::new().serve("0.0.0.0:8000").await
/// }
/// ```
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    main_macro::expand(attr, item)
}

/// Turns an inline module into a router.
///
/// Discovers the functions annotated with a route macro and generates a
/// `router()` function that mounts them under the given `prefix` and `tags`.
///
/// # Example
///
/// ```ignore
/// #[api_router(prefix = "/users", tags = ["users"])]
/// pub mod users_router {
///     use super::*;
///
///     #[get("/{user_id}", response_model = UserOut)]
///     pub async fn get_user(user_id: i64, service: UserService) -> tork::Result<UserOut> {
///         service.get_user(user_id).await
///     }
/// }
/// // `users_router::router()` is now available.
/// ```
#[proc_macro_attribute]
pub fn api_router(attr: TokenStream, item: TokenStream) -> TokenStream {
    api_router::expand(attr, item)
}

/// Turns a type into a request dependency.
///
/// Applied to an `impl` block containing an async `resolve` function, it
/// generates a `FromRequest` implementation. Each parameter of `resolve` is
/// itself resolved through `FromRequest` (recursively), then `resolve` is called.
///
/// # Example
///
/// ```ignore
/// #[tork::dependency]
/// impl CurrentUser {
///     pub async fn resolve(token: BearerToken, users: UserRepository) -> tork::Result<Self> {
///         // ...resolve the current user from the token...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn dependency(attr: TokenStream, item: TokenStream) -> TokenStream {
    dependency::expand(attr, item)
}

/// Turns a struct into a validated, documented API model.
///
/// Derives serde (de)serialization, `garde` validation, and `schemars` JSON
/// Schema, and translates `#[field(...)]` constraints into the matching `garde`
/// and `schemars` attributes.
///
/// # Supported `#[field(...)]` constraints
///
/// - `min_length` / `max_length` — string length bounds
/// - `ge` / `le` — inclusive numeric bounds
/// - `gt` / `lt` — exclusive numeric bounds
/// - `title` / `description` — documentation metadata
/// - `custom` — a custom validator function (may be repeated); the function
///   performs the check and returns its own message, e.g.
///   `#[field(custom = validate_password)]`
///
/// # Example
///
/// ```ignore
/// #[api_model(rename_all = "camelCase")]
/// pub struct CreateOrderInput {
///     #[field(min_length = 1, max_length = 120)] pub name: String,
///     #[field(gt = 0, description = "The price must be greater than zero")] pub price: f64,
/// }
/// ```
#[proc_macro_attribute]
pub fn api_model(attr: TokenStream, item: TokenStream) -> TokenStream {
    api_model::expand(attr, item)
}

/// Declares a typed application configuration.
///
/// Derives `serde` deserialization and `garde` validation, maps `#[setting(...)]`
/// constraints to validation rules, turns `#[setting(default = ...)]` into a serde
/// default, and generates a `load()` method that merges the configured sources
/// (`.env`, environment variables, config files, a secrets directory, overrides)
/// into a validated value. Loading is meant to run once at startup.
///
/// # Example
///
/// ```ignore
/// #[tork::settings(prefix = "APP", env_file = ".env")]
/// pub struct Config {
///     #[setting(default = "Awesome API")] pub app_name: String,
///     #[setting(email)] pub admin_email: String,
///     #[setting(default = 50, ge = 1, le = 500)] pub items_per_user: u32,
///     #[setting(secret)] pub jwt_secret: tork::SecretString,
/// }
/// // let config = Config::load()?;
/// ```
#[proc_macro_attribute]
pub fn settings(attr: TokenStream, item: TokenStream) -> TokenStream {
    settings::expand(attr, item)
}

/// Declares an application lifespan on a resource container.
///
/// Applied to `impl T { async fn startup(ctx) -> tork::Result<Self>; async fn
/// shutdown(self) -> tork::Result<()> }`, it makes `T` a `Lifespan`. `shutdown`
/// is optional. `T` must also derive `Resources`.
///
/// # Example
///
/// ```ignore
/// #[tork::lifespan]
/// impl AppState {
///     async fn startup(ctx: tork::LifespanContext) -> tork::Result<Self> { /* ... */ }
///     async fn shutdown(self) -> tork::Result<()> { /* ... */ }
/// }
/// // App::new().lifespan::<AppState>()
/// ```
#[proc_macro_attribute]
pub fn lifespan(attr: TokenStream, item: TokenStream) -> TokenStream {
    lifespan::expand(attr, item)
}

/// Declares a resource container.
///
/// Generates a `Resources` implementation that registers each `#[resource]`
/// field by type, and a `FromRequest` implementation for each resource type so
/// it can be injected directly.
#[proc_macro_derive(Resources, attributes(resource))]
pub fn derive_resources(item: TokenStream) -> TokenStream {
    resources::expand(item)
}

/// Derives request-time construction by injecting each field.
///
/// Generates a `FromRequest` implementation that resolves every field through
/// `FromRequest` (a resource, another `Inject` service, or a built-in extractor).
#[proc_macro_derive(Inject)]
pub fn derive_inject(item: TokenStream) -> TokenStream {
    inject::expand(item)
}

/// Derives `FromMultipart` for a `multipart/form-data` model.
///
/// Each field binds from the parsed form: file fields (`FileBytes` / `UploadFile`,
/// optionally `Option` or `Vec`) are recognized by a `#[file]` attribute or their
/// type; every other field is a text field parsed from its string value.
/// `#[field(...)]` constraints validate text fields.
///
/// # Example
///
/// ```ignore
/// #[derive(FormModel)]
/// pub struct CreateFileForm {
///     #[file] pub file: FileBytes,
///     #[field(min_length = 16)] pub token: String,
/// }
/// ```
#[proc_macro_derive(FormModel, attributes(file, form, field))]
pub fn derive_form_model(item: TokenStream) -> TokenStream {
    form_model::expand(item)
}

/// Derives `From<Self> for tork::Error`, storing the value as a typed source.
///
/// This lets a user error convert into a framework error through `?` while
/// preserving the original value, so a registered
/// `exception_handler::<Self>()` can recover and map it. A type-level
/// `#[status(...)]` attribute sets the default HTTP status (a status code such as
/// `503`, or an `ErrorKind` variant name); it defaults to `500`. The type must
/// implement `Display` and `std::error::Error`.
///
/// # Example
///
/// ```ignore
/// #[derive(Debug, tork::AppError)]
/// #[status(503)]
/// pub enum DbError { Timeout }
/// // impl Display + Error for DbError ...
/// // `repo.query()?` now converts DbError into tork::Error.
/// ```
#[proc_macro_derive(AppError, attributes(status))]
pub fn derive_app_error(item: TokenStream) -> TokenStream {
    app_error::expand(item)
}

/// Turns an async function into a middleware layer.
///
/// The function must be `async fn(request, next) -> tork::Result<tork::Response>`.
/// It is rewritten into a unit struct of the same name implementing the
/// `Middleware` trait, so it can be passed to `App::middleware`.
///
/// # Example
///
/// ```ignore
/// #[middleware]
/// pub async fn add_process_time_header(req: Request, next: Next) -> Result<Response> {
///     let mut res = next.run(req).await?;
///     // ...set a header on res...
///     Ok(res)
/// }
/// ```
#[proc_macro_attribute]
pub fn middleware(attr: TokenStream, item: TokenStream) -> TokenStream {
    middleware::expand(attr, item)
}

/// Declares a `GET` route. See [`macro@get`] and the other method macros for the
/// supported attributes (`response_model`, `summary`, `description`,
/// `status_code`, `tags`).
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("GET", attr, item)
}

/// Declares a `POST` route.
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("POST", attr, item)
}

/// Declares a `PUT` route.
#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("PUT", attr, item)
}

/// Declares a `PATCH` route.
#[proc_macro_attribute]
pub fn patch(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("PATCH", attr, item)
}

/// Declares a `DELETE` route.
#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("DELETE", attr, item)
}

/// Declares a Server-Sent Events endpoint (default `GET`).
///
/// The handler returns `tork::Result<Sse<T>>`. Attributes: path (first), `method`,
/// `event` (default event name), `summary`, `description`, `tags`.
///
/// # Example
///
/// ```ignore
/// #[sse("/items/stream", event = "item_update")]
/// pub async fn stream_items(service: ItemService) -> tork::Result<Sse<ItemOut>> {
///     Ok(Sse::new(service.item_stream()))
/// }
/// ```
#[proc_macro_attribute]
pub fn sse(attr: TokenStream, item: TokenStream) -> TokenStream {
    sse::expand("GET", attr, item)
}

/// Declares a Server-Sent Events endpoint served over `POST`.
///
/// Shorthand for [`macro@sse`] with `method = POST`; useful for streaming a
/// response to a request that carries a body.
#[proc_macro_attribute]
pub fn post_sse(attr: TokenStream, item: TokenStream) -> TokenStream {
    sse::expand("POST", attr, item)
}

/// Declares a WebSocket endpoint.
///
/// The handler takes a `WebSocket` parameter (the upgrade handle) and returns
/// `tork::Result<()>`. Other parameters are path parameters or dependencies,
/// resolved before the upgrade. Attributes: path (first), `summary`,
/// `description`, `tags`.
///
/// # Example
///
/// ```ignore
/// #[websocket("/ws")]
/// pub async fn echo(socket: WebSocket) -> tork::Result<()> {
///     let mut socket = socket.accept().await?;
///     while let Some(message) = socket.recv().await? { /* ... */ }
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn websocket(attr: TokenStream, item: TokenStream) -> TokenStream {
    websocket::expand(attr, item)
}
