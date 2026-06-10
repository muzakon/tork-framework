//! Procedural macros for the Tork web framework.
//!
//! Every macro here emits code that refers to the public API through the `tork`
//! facade crate (for example `::tork::Router`), never through `tork-core`
//! directly. This lets generated code compile inside user crates that depend
//! only on `tork`.

use proc_macro::TokenStream;

mod api_model;
mod api_router;
mod common;
mod dependency;
mod main_macro;
mod route;

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
