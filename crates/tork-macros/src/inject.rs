//! The `#[derive(Inject)]` macro.
//!
//! Generates a `FromRequest` implementation that builds the struct by resolving
//! each field through `FromRequest`. Fields may be resources, other `Inject`
//! services, or built-in extractors.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Type, parse_macro_input};

use crate::common::krate;

/// Returns `true` if the type's final path segment is `Logger`.
fn is_logger(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|s| s.ident == "Logger"))
}

/// Reads a `context = "..."` value from an `#[inject(...)]` or `#[logger(...)]`
/// attribute, if present.
fn context_attr(attrs: &[Attribute], name: &str) -> syn::Result<Option<String>> {
    let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident(name)) else {
        return Ok(None);
    };
    let mut context = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("context") {
            let value: LitStr = meta.value()?.parse()?;
            context = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("expected `context = \"...\"`"))
        }
    })?;
    Ok(context)
}

/// Expands `#[derive(Inject)]` over a named struct.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "#[derive(Inject)] requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(Inject)] can only be derived for structs",
            ));
        }
    };

    let krate = krate();
    let ident = &input.ident;
    // A struct-level `#[inject(context = "...")]` sets the default context for the
    // logger fields; otherwise the struct name is used.
    let container_context = context_attr(&input.attrs, "inject")?;

    let mut bindings = Vec::new();
    let mut names = Vec::new();
    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;

        if is_logger(field_ty) {
            // A `Logger` field is given a context: a field-level `#[logger(context)]`,
            // then the struct-level default, then the struct name.
            let context = context_attr(&field.attrs, "logger")?
                .or_else(|| container_context.clone())
                .unwrap_or_else(|| ident.to_string());
            bindings.push(quote! {
                let #field_ident = <#field_ty as #krate::FromRequest>::from_request(ctx)
                    .await?
                    .for_context(#context);
            });
        } else {
            bindings.push(quote! {
                let #field_ident = <#field_ty as #krate::FromRequest>::from_request(ctx).await?;
            });
        }
        names.push(field_ident);
    }

    Ok(quote! {
        impl #krate::FromRequest for #ident {
            fn from_request(
                ctx: & #krate::RequestContext,
            ) -> impl ::core::future::Future<Output = #krate::Result<Self>> + Send {
                async move {
                    // A test client may substitute this service with a pre-built
                    // instance; otherwise it is constructed from its fields.
                    if let ::core::option::Option::Some(__overridden) =
                        #krate::__take_override::<Self>(ctx)
                    {
                        return ::core::result::Result::Ok(__overridden);
                    }
                    #(#bindings)*
                    ::core::result::Result::Ok(#ident { #(#names),* })
                }
            }
        }
    })
}
