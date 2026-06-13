//! The `#[tork::dependency]` attribute macro.
//!
//! Applied to an `impl` block containing an async `resolve` function, it
//! generates a `FromRequest` implementation that resolves each of `resolve`'s
//! parameters (recursively, through `FromRequest`) and then calls `resolve`.
//! Because the generated implementation is concrete, there is no blanket impl
//! and therefore no coherence conflict with the built-in extractors.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ImplItem, ImplItemFn, ItemImpl, Pat};

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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn find_resolve_finds_method_and_reports_missing_one() {
        let item_impl: ItemImpl = parse_quote! {
            impl Service {
                async fn resolve(dep: Dep) -> tork::Result<Self> { todo!() }
            }
        };
        assert_eq!(find_resolve(&item_impl).unwrap().sig.ident, "resolve");

        let missing: ItemImpl = parse_quote! {
            impl Service {
                async fn build() -> tork::Result<Self> { todo!() }
            }
        };
        assert!(find_resolve(&missing)
            .unwrap_err()
            .to_string()
            .contains("requires an async fn `resolve`"));
    }

    #[test]
    fn expand_impl_rejects_invalid_resolve_shapes() {
        let item_impl: ItemImpl = parse_quote! {
            impl Service {
                fn resolve(dep: Dep) -> tork::Result<Self> { todo!() }
            }
        };
        assert!(expand_impl(item_impl)
            .unwrap_err()
            .to_string()
            .contains("must be an async fn"));

        let item_impl: ItemImpl = parse_quote! {
            impl Service {
                async fn resolve(self, dep: Dep) -> tork::Result<Self> { todo!() }
            }
        };
        assert!(expand_impl(item_impl)
            .unwrap_err()
            .to_string()
            .contains("cannot take `self`"));

        let item_impl: ItemImpl = parse_quote! {
            impl Service {
                async fn resolve((dep): Dep) -> tork::Result<Self> { todo!() }
            }
        };
        assert!(expand_impl(item_impl)
            .unwrap_err()
            .to_string()
            .contains("simple identifiers"));
    }

    #[test]
    fn expand_impl_emits_from_request_resolution() {
        let item_impl: ItemImpl = parse_quote! {
            impl Service {
                async fn resolve(dep: Dep, logger: Logger) -> tork::Result<Self> { todo!() }
            }
        };
        let tokens = expand_impl(item_impl).unwrap().to_string();
        assert!(tokens.contains("FromRequest for Service"));
        assert!(tokens.contains("let dep ="));
        assert!(tokens.contains("let logger ="));
        assert!(tokens.contains("Self :: resolve"));
        assert!(tokens.contains("dep"));
        assert!(tokens.contains("logger"));
    }
}
