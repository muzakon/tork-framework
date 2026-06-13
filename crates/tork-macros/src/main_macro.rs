//! The `#[tork::main]` attribute macro.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

use crate::common::krate;

/// Expands `#[tork::main]` over an `async fn`.
pub fn expand(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);

    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            func.sig.fn_token,
            "`#[tork::main]` can only be applied to an async fn",
        )
        .to_compile_error()
        .into();
    }

    let attrs = &func.attrs;
    let vis = &func.vis;
    let ident = &func.sig.ident;
    let output = &func.sig.output;
    let block = &func.block;
    let krate = krate();

    quote! {
        #(#attrs)*
        #vis fn #ident() #output {
            #krate::__rt::block_on(async move #block)
        }
    }
    .into()
}
