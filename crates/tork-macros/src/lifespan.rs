//! The `#[tork::lifespan]` attribute macro.
//!
//! Applied to an inherent impl block containing an async `startup` (and optional
//! async `shutdown`), it rewrites the block into a `Lifespan` implementation for
//! the same type. The type must also be a `Resources` container.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ImplItem, ItemImpl, Visibility};

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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn prepare_rejects_trait_impls_and_non_functions() {
        let mut item_impl: ItemImpl = parse_quote! {
            impl SomeTrait for App {
                async fn startup(&self) -> tork::Result<()> { Ok(()) }
            }
        };
        assert!(prepare(&mut item_impl)
            .unwrap_err()
            .to_string()
            .contains("inherent impl block"));

        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                const VALUE: usize = 1;
            }
        };
        assert!(prepare(&mut item_impl)
            .unwrap_err()
            .to_string()
            .contains("may only contain `startup` and `shutdown`"));
    }

    #[test]
    fn prepare_rejects_invalid_method_names_and_sync_methods() {
        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                async fn startup() -> tork::Result<()> { Ok(()) }
                async fn helper() -> tork::Result<()> { Ok(()) }
            }
        };
        assert!(prepare(&mut item_impl)
            .unwrap_err()
            .to_string()
            .contains("put helper methods in a separate impl block"));

        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                fn startup() -> tork::Result<()> { Ok(()) }
            }
        };
        assert!(prepare(&mut item_impl)
            .unwrap_err()
            .to_string()
            .contains("must be async"));
    }

    #[test]
    fn prepare_requires_startup_and_strips_visibility() {
        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                pub async fn shutdown() -> tork::Result<()> { Ok(()) }
            }
        };
        assert!(prepare(&mut item_impl)
            .unwrap_err()
            .to_string()
            .contains("requires an async `startup` method"));

        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                pub async fn startup() -> tork::Result<()> { Ok(()) }
                pub(crate) async fn shutdown() -> tork::Result<()> { Ok(()) }
            }
        };
        prepare(&mut item_impl).unwrap();
        for item in &item_impl.items {
            let ImplItem::Fn(func) = item else {
                panic!("expected method");
            };
            assert!(matches!(func.vis, Visibility::Inherited));
        }
    }

    #[test]
    fn expand_emits_lifespan_impl() {
        let mut item_impl: ItemImpl = parse_quote! {
            impl App {
                pub async fn startup() -> tork::Result<()> { Ok(()) }
                async fn shutdown() -> tork::Result<()> { Ok(()) }
            }
        };
        prepare(&mut item_impl).unwrap();
        let self_ty = &item_impl.self_ty;
        let methods = &item_impl.items;
        let tokens = quote! {
            impl tork::Lifespan for #self_ty {
                #(#methods)*
            }
        }
        .to_string();
        assert!(tokens.contains("impl tork :: Lifespan for App"));
        assert!(tokens.contains("async fn startup"));
        assert!(tokens.contains("async fn shutdown"));
        assert!(!tokens.contains("pub async fn startup"));
    }
}
