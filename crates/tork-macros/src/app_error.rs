//! The `#[derive(AppError)]` macro.
//!
//! Generates `From<T> for tork::Error` so a user error converts into a framework
//! error through `?`. The error value is stored as a typed source, which lets a
//! registered `exception_handler::<T>()` recover it. A type-level `#[status(...)]`
//! attribute selects the default HTTP status (used when no handler is registered).

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Expr, ExprLit, Lit};

use crate::common::krate;

/// Expands `#[derive(AppError)]`.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let krate = krate();
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let kind = error_kind(&input)?;

    Ok(quote! {
        impl #impl_generics ::core::convert::From<#ident #ty_generics> for #krate::Error
            #where_clause
        {
            fn from(value: #ident #ty_generics) -> Self {
                let message = ::std::string::ToString::to_string(&value);
                #krate::Error::new(#krate::ErrorKind::#kind, message).with_source(value)
            }
        }
    })
}

/// Resolves the `ErrorKind` variant from the optional `#[status(...)]` attribute.
///
/// The argument may be an HTTP status code (for example `503`) or an `ErrorKind`
/// variant name (for example `ServiceUnavailable`). Defaults to `Internal`.
fn error_kind(input: &DeriveInput) -> syn::Result<proc_macro2::Ident> {
    let attr = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("status"));

    let Some(attr) = attr else {
        return Ok(ident("Internal"));
    };

    match attr.parse_args::<Expr>()? {
        Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        }) => {
            let code: u16 = int.base10_parse()?;
            kind_for_status(code)
                .map(ident)
                .ok_or_else(|| syn::Error::new_spanned(&int, unsupported_status(code)))
        }
        Expr::Path(path) => {
            let name = path
                .path
                .get_ident()
                .ok_or_else(|| syn::Error::new_spanned(&path, "expected an `ErrorKind` variant"))?;
            if is_kind_variant(&name.to_string()) {
                Ok(name.clone())
            } else {
                Err(syn::Error::new_spanned(
                    name,
                    format!("`{name}` is not an `ErrorKind` variant"),
                ))
            }
        }
        other => Err(syn::Error::new_spanned(
            other,
            "`#[status(...)]` expects a status code (for example `503`) or an `ErrorKind` variant",
        )),
    }
}

/// Maps an HTTP status code to its `ErrorKind` variant name.
fn kind_for_status(code: u16) -> Option<&'static str> {
    let variant = match code {
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        409 => "Conflict",
        413 => "PayloadTooLarge",
        422 => "Unprocessable",
        429 => "TooManyRequests",
        500 => "Internal",
        503 => "ServiceUnavailable",
        504 => "GatewayTimeout",
        _ => return None,
    };
    Some(variant)
}

/// Reports whether `name` is a supported `ErrorKind` variant.
fn is_kind_variant(name: &str) -> bool {
    matches!(
        name,
        "BadRequest"
            | "Unauthorized"
            | "Forbidden"
            | "NotFound"
            | "MethodNotAllowed"
            | "Conflict"
            | "PayloadTooLarge"
            | "Unprocessable"
            | "TooManyRequests"
            | "Internal"
            | "ServiceUnavailable"
            | "GatewayTimeout"
    )
}

/// Builds an identifier on the call site span.
fn ident(name: &str) -> proc_macro2::Ident {
    proc_macro2::Ident::new(name, proc_macro2::Span::call_site())
}

/// Message listing the supported status codes for an unsupported one.
fn unsupported_status(code: u16) -> String {
    format!(
        "status code `{code}` has no matching `ErrorKind`; supported codes are 400, 401, 403, \
         404, 405, 409, 413, 422, 429, 500, 503, 504"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn kind_helpers_cover_supported_and_unsupported_values() {
        assert_eq!(kind_for_status(400), Some("BadRequest"));
        assert_eq!(kind_for_status(503), Some("ServiceUnavailable"));
        assert_eq!(kind_for_status(999), None);
        assert!(is_kind_variant("Internal"));
        assert!(!is_kind_variant("Nope"));
        assert_eq!(ident("Foo").to_string(), "Foo");
        assert!(unsupported_status(418).contains("418"));
    }

    #[test]
    fn error_kind_defaults_and_accepts_code_or_variant() {
        let input: DeriveInput = parse_quote! {
            struct MyError;
        };
        assert_eq!(error_kind(&input).unwrap().to_string(), "Internal");

        let input: DeriveInput = parse_quote! {
            #[status(503)]
            struct MyError;
        };
        assert_eq!(
            error_kind(&input).unwrap().to_string(),
            "ServiceUnavailable"
        );

        let input: DeriveInput = parse_quote! {
            #[status(Forbidden)]
            struct MyError;
        };
        assert_eq!(error_kind(&input).unwrap().to_string(), "Forbidden");
    }

    #[test]
    fn error_kind_rejects_invalid_status_forms() {
        let input: DeriveInput = parse_quote! {
            #[status(418)]
            struct MyError;
        };
        assert!(error_kind(&input)
            .unwrap_err()
            .to_string()
            .contains("supported codes"));

        let input: DeriveInput = parse_quote! {
            #[status(foo::Bar)]
            struct MyError;
        };
        assert!(error_kind(&input)
            .unwrap_err()
            .to_string()
            .contains("expected an `ErrorKind` variant"));

        let input: DeriveInput = parse_quote! {
            #[status(Nope)]
            struct MyError;
        };
        assert!(error_kind(&input)
            .unwrap_err()
            .to_string()
            .contains("is not an `ErrorKind` variant"));

        let input: DeriveInput = parse_quote! {
            #[status("bad")]
            struct MyError;
        };
        assert!(error_kind(&input)
            .unwrap_err()
            .to_string()
            .contains("expects a status code"));
    }

    #[test]
    fn expand_derive_emits_from_impl() {
        let input: DeriveInput = parse_quote! {
            #[status(429)]
            struct RateLimited;
        };
        let tokens = expand_derive(input).unwrap().to_string();
        assert!(tokens.contains("From < RateLimited > for"));
        assert!(tokens.contains("ErrorKind :: TooManyRequests"));
        assert!(tokens.contains("with_source"));
    }
}
