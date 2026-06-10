//! The `#[tork::lifespan]` attribute macro.
//!
//! Applied to an inherent impl block containing an async `startup` (and optional
//! async `shutdown`), it rewrites the block into a `Lifespan` implementation for
//! the same type. The type must also be a `Resources` container.

use proc_macro::TokenStream;
use quote::quote;
use syn::{ImplItem, ItemImpl, Visibility, parse_macro_input};

use crate::common::krate;

/// Expands `#[tork::lifespan]` over an inherent impl block.
pub fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_impl = parse_macro_input!(item as ItemImpl);
    if let Err(error) = prepare(&mut item_impl) {
        return error.to_compile_error().into();
    }

    let krate = krate();
    let self_ty = &item_impl.self_ty;
    let methods = &item_impl.items;

    quote! {
        impl #krate::Lifespan for #self_ty {
            #(#methods)*
        }
    }
    .into()
}

/// Validates the impl and strips per-method visibility (trait impl methods take
/// the trait's visibility).
fn prepare(item_impl: &mut ItemImpl) -> syn::Result<()> {
    if item_impl.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[tork::lifespan] must be applied to an inherent impl block",
        ));
    }

    let mut has_startup = false;
    for item in &mut item_impl.items {
        let ImplItem::Fn(func) = item else {
            return Err(syn::Error::new_spanned(
                &*item,
                "#[tork::lifespan] impl may only contain `startup` and `shutdown`",
            ));
        };

        match func.sig.ident.to_string().as_str() {
            "startup" => has_startup = true,
            "shutdown" => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    &func.sig,
                    "#[tork::lifespan] impl may only contain `startup` and `shutdown`; \
                     put helper methods in a separate impl block",
                ));
            }
        }

        if func.sig.asyncness.is_none() {
            return Err(syn::Error::new_spanned(
                &func.sig,
                "lifespan methods must be async",
            ));
        }

        func.vis = Visibility::Inherited;
    }

    if !has_startup {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[tork::lifespan] requires an async `startup` method",
        ));
    }

    Ok(())
}
