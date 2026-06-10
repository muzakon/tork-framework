//! Procedural macros for the Tork web framework.
//!
//! Every macro here emits code that refers to the public API through the `tork`
//! facade crate (for example `::tork::Router`), never through `tork-core`
//! directly. This lets generated code compile inside user crates that depend
//! only on `tork`.

use proc_macro::TokenStream;

mod common;
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
