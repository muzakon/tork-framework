//! The `#[derive(AppError)]` macro.
//!
//! Generates `From<T> for tork::Error` so a user error converts into a framework
//! error through `?`. The error value is stored as a typed source, which lets a
//! registered `exception_handler::<T>()` recover it. A type-level `#[status(...)]`
//! attribute selects the default HTTP status (used when no handler is registered).

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Expr, ExprLit, Lit, parse_macro_input};

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
    let attr = input.attrs.iter().find(|attr| attr.path().is_ident("status"));

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
