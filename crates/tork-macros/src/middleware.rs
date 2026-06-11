//! The `#[middleware]` attribute macro.
//!
//! Turns an `async fn(request, next) -> Result<Response>` into a unit struct of
//! the same name that implements `Middleware`, so it can be passed to
//! `App::middleware`. A concrete generated impl (rather than a blanket impl over
//! function types) is what keeps the `Middleware` trait free of coherence
//! conflicts with the built-in middleware structs.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Visibility, parse_macro_input};

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

    let param_count = func
        .sig
        .inputs
        .iter()
        .filter(|input| matches!(input, FnArg::Typed(_)))
        .count();
    if func.sig.inputs.iter().any(|i| matches!(i, FnArg::Receiver(_))) {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`#[middleware]` functions cannot take `self`",
        ));
    }
    if param_count != 2 {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "`#[middleware]` functions must take exactly two parameters: (request, next)",
        ));
    }

    let krate = krate();
    let attrs = func.attrs.clone();
    let vis = func.vis.clone();
    let ident = func.sig.ident.clone();
    let inner_ident = format_ident!("__tork_middleware_{}", ident);

    // Keep the user's function verbatim (with its own signature), renamed and
    // made private; the generated `handle` simply boxes a call to it. This means
    // the user's parameter and return types are compiled as written.
    let mut inner = func;
    inner.attrs.clear();
    inner.vis = Visibility::Inherited;
    inner.sig.ident = inner_ident.clone();

    Ok(quote! {
        #(#attrs)*
        #[allow(non_camel_case_types)]
        #[derive(::core::clone::Clone, ::core::marker::Copy)]
        #vis struct #ident;

        #inner

        impl #krate::Middleware for #ident {
            fn handle(
                &self,
                request: #krate::Request,
                next: #krate::Next,
            ) -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                ::std::boxed::Box::pin(#inner_ident(request, next))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn expand_fn_rejects_sync_self_and_wrong_arity() {
        let func: ItemFn = parse_quote! {
            fn audit(request: tork::Request, next: tork::Next) -> tork::Result<tork::Response> { todo!() }
        };
        assert!(expand_fn(func)
            .unwrap_err()
            .to_string()
            .contains("async fn"));

        let func: ItemFn = parse_quote! {
            async fn audit(&self, request: tork::Request, next: tork::Next) -> tork::Result<tork::Response> { todo!() }
        };
        assert!(expand_fn(func)
            .unwrap_err()
            .to_string()
            .contains("cannot take `self`"));

        let func: ItemFn = parse_quote! {
            async fn audit(request: tork::Request) -> tork::Result<tork::Response> { todo!() }
        };
        assert!(expand_fn(func)
            .unwrap_err()
            .to_string()
            .contains("exactly two parameters"));
    }

    #[test]
    fn expand_fn_emits_struct_and_handle_impl() {
        let func: ItemFn = parse_quote! {
            #[doc = "middleware"]
            pub async fn audit(
                request: tork::Request,
                next: tork::Next,
            ) -> tork::Result<tork::Response> {
                next.run(request).await
            }
        };
        let tokens = expand_fn(func).unwrap().to_string();
        assert!(tokens.contains("struct audit"));
        assert!(tokens.contains("__tork_middleware_audit"));
        assert!(tokens.contains("Middleware for audit"));
        assert!(tokens.contains("Box :: pin"));
    }
}
