//! The `#[tork::dependency]` attribute macro.
//!
//! Applied to an `impl` block containing an async `resolve` function, it
//! generates a `FromRequest` implementation that resolves each of `resolve`'s
//! parameters (recursively, through `FromRequest`) and then calls `resolve`.
//! Because the generated implementation is concrete, there is no blanket impl
//! and therefore no coherence conflict with the built-in extractors.

use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItem, ImplItemFn, ItemImpl, Pat, parse_macro_input};

use crate::common::krate;

/// Expands `#[tork::dependency]` over an `impl` block.
pub fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    match expand_impl(item_impl) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Generates the `FromRequest` impl alongside the original `impl` block.
fn expand_impl(item_impl: ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let krate = krate();
    let self_ty = &item_impl.self_ty;

    let resolve = find_resolve(&item_impl)?;
    if resolve.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &resolve.sig,
            "`resolve` must be an async fn",
        ));
    }

    // Resolve every `resolve` parameter through `FromRequest`, in order.
    let mut bindings = Vec::new();
    let mut call_args = Vec::new();
    for input in &resolve.sig.inputs {
        let pat_type = match input {
            FnArg::Typed(pat_type) => pat_type,
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "`resolve` cannot take `self`",
                ));
            }
        };

        let ident = match pat_type.pat.as_ref() {
            Pat::Ident(pat_ident) => pat_ident.ident.clone(),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "`resolve` parameters must be simple identifiers",
                ));
            }
        };
        let ty = pat_type.ty.as_ref();

        bindings.push(quote! {
            let #ident = <#ty as #krate::FromRequest>::from_request(ctx).await?;
        });
        call_args.push(ident);
    }

    Ok(quote! {
        #item_impl

        impl #krate::FromRequest for #self_ty {
            fn from_request(
                ctx: & #krate::RequestContext,
            ) -> impl ::core::future::Future<Output = #krate::Result<Self>> + Send {
                async move {
                    #(#bindings)*
                    Self::resolve(#(#call_args),*).await
                }
            }
        }
    })
}

/// Locates the `resolve` function within the impl block.
fn find_resolve(item_impl: &ItemImpl) -> syn::Result<&ImplItemFn> {
    item_impl
        .items
        .iter()
        .find_map(|item| match item {
            ImplItem::Fn(func) if func.sig.ident == "resolve" => Some(func),
            _ => None,
        })
        .ok_or_else(|| {
            syn::Error::new_spanned(
                item_impl,
                "#[tork::dependency] requires an async fn `resolve` in the impl block",
            )
        })
}
