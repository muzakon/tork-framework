//! The `#[derive(QueryResult)]` macro.
//!
//! Generates a [`FromRow`] implementation for a projection DTO: each field is read
//! from the like-named result column. Unlike a model, a query result has no table
//! or column metadata; it only maps rows.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::common::krate;

/// Expands `#[derive(QueryResult)]`.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_query_result(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_query_result(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(QueryResult)] supports only structs",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(QueryResult)] supports only structs with named fields",
        ));
    };

    let krate = krate();
    let fields = named.named.iter().map(|field| {
        let field_ident = field.ident.as_ref().expect("named field");
        let column = field_ident.to_string();
        quote!(#field_ident: row.get(#column)?)
    });

    Ok(quote! {
        impl #krate::FromRow for #ident {
            fn from_row(row: &#krate::Row) -> #krate::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #(#fields),*
                })
            }
        }
    })
}
