//! Shared helpers for the ORM derive macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{GenericArgument, PathArguments, Type};

/// The path generated code uses to reach the ORM's public API.
///
/// Everything is referenced through the `tork-orm` facade so generated code
/// compiles in user crates that depend only on `tork-orm`.
pub fn krate() -> TokenStream {
    quote!(::tork_orm)
}

/// Returns the inner type of `Option<T>`, or `None` for any other type.
pub fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

/// Returns `true` if `ty` is the named single-segment type (e.g. `i64`, `String`).
fn is_ident(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|s| s.ident == name))
}

/// Returns `true` if `ty` is `Vec<u8>`.
fn is_byte_vec(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Vec" {
        return false;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    args.args.iter().any(|arg| matches!(arg, GenericArgument::Type(inner) if is_ident(inner, "u8")))
}

/// Maps a Rust field type (the inner type for an `Option`) to a `SqlType`
/// expression. Unrecognized types map to `Text`, which suits string-backed
/// custom types such as enums stored by their name.
pub fn sql_type_for(ty: &Type) -> TokenStream {
    let krate = krate();
    if is_ident(ty, "bool") {
        quote!(#krate::SqlType::Boolean)
    } else if is_ident(ty, "i32") || is_ident(ty, "u32") {
        quote!(#krate::SqlType::Integer)
    } else if is_ident(ty, "i64") || is_ident(ty, "u64") {
        quote!(#krate::SqlType::BigInt)
    } else if is_ident(ty, "f64") || is_ident(ty, "f32") {
        quote!(#krate::SqlType::Real)
    } else if is_byte_vec(ty) {
        quote!(#krate::SqlType::Blob)
    } else if is_ident(ty, "OffsetDateTime") || is_ident(ty, "DateTimeUtc") {
        quote!(#krate::SqlType::Timestamp)
    } else {
        quote!(#krate::SqlType::Text)
    }
}

/// Converts a `PascalCase` identifier to `snake_case`.
pub fn to_snake(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (index, ch) in input.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
