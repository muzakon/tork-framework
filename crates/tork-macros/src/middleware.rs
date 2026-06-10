//! The `#[middleware]` attribute macro.
//!
//! Turns an `async fn(request, next) -> Result<Response>` into a unit struct of
//! the same name that implements `Middleware`, so it can be passed to
//! `App::middleware`. A concrete generated impl (rather than a blanket impl over
//! function types) is what keeps the `Middleware` trait free of coherence
//! conflicts with the built-in middleware structs.

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, parse_macro_input};

use crate::common::krate;

/// Expands `#[middleware]` over an async fn.
pub fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    match expand_fn(func) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_fn(func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            func.sig.fn_token,
            "`#[middleware]` can only be applied to an async fn",
        ));
    }

    let mut params = Vec::new();
    for input in &func.sig.inputs {
        match input {
            FnArg::Typed(pat_type) => params.push((*pat_type.pat).clone()),
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "`#[middleware]` functions cannot take `self`",
                ));
            }
        }
    }
    if params.len() != 2 {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`#[middleware]` functions must take exactly two parameters: (request, next)",
        ));
    }

    let krate = krate();
    let attrs = &func.attrs;
    let vis = &func.vis;
    let ident = &func.sig.ident;
    let block = &func.block;
    let request_pat = &params[0];
    let next_pat = &params[1];

    Ok(quote! {
        #(#attrs)*
        #[allow(non_camel_case_types)]
        #[derive(::core::clone::Clone, ::core::marker::Copy)]
        #vis struct #ident;

        impl #krate::Middleware for #ident {
            fn handle(
                &self,
                #request_pat: #krate::Request,
                #next_pat: #krate::Next,
            ) -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                ::std::boxed::Box::pin(async move #block)
            }
        }
    })
}
