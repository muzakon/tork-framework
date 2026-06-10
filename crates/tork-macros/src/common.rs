//! Shared helpers for the procedural macros.

use proc_macro2::TokenStream;
use quote::quote;

/// Returns the path to the facade crate that all generated code references.
///
/// Using the `tork` facade (rather than `tork-core`) means generated code
/// compiles inside user crates that depend only on `tork`.
pub fn krate() -> TokenStream {
    quote! { ::tork }
}

/// Extracts the placeholder names from a route path.
///
/// For example, `"/users/{user_id}/orders/{order_id}"` yields `["user_id",
/// "order_id"]`. The wildcard marker in `{*rest}` is stripped.
pub fn path_param_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = path.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'{' {
            if let Some(offset) = path[index + 1..].find('}') {
                let inner = &path[index + 1..index + 1 + offset];
                names.push(inner.trim_start_matches('*').to_owned());
                index += offset + 2;
                continue;
            }
        }
        index += 1;
    }

    names
}
