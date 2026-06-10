//! The `#[derive(Inject)]` macro.
//!
//! Generates a `FromRequest` implementation that builds the struct by resolving
//! each field through `FromRequest`. Fields may be resources, other `Inject`
//! services, or built-in extractors.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

use crate::common::krate;

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

    let mut bindings = Vec::new();
    let mut names = Vec::new();
    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;
        bindings.push(quote! {
            let #field_ident = <#field_ty as #krate::FromRequest>::from_request(ctx).await?;
        });
        names.push(field_ident);
    }

    Ok(quote! {
        impl #krate::FromRequest for #ident {
            fn from_request(
                ctx: & #krate::RequestContext,
            ) -> impl ::core::future::Future<Output = #krate::Result<Self>> + Send {
                async move {
                    #(#bindings)*
                    ::core::result::Result::Ok(#ident { #(#names),* })
                }
            }
        }
    })
}
