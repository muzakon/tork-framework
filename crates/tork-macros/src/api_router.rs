//! The `#[api_router]` module macro.
//!
//! Applied to an inline module, it discovers the functions annotated with a
//! route macro (`#[get]`, `#[post]`, ...), re-emits the module unchanged so those
//! macros still expand, and appends a `router()` function that assembles the
//! module's routes under a shared prefix and tag set.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Ident, Item, ItemMod, LitStr, Meta, Token, bracketed};

use crate::common::krate;

/// Parsed attributes of `#[api_router(...)]`.
struct ApiRouterArgs {
    prefix: Option<LitStr>,
    tags: Vec<LitStr>,
}

impl Parse for ApiRouterArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ApiRouterArgs {
            prefix: None,
            tags: Vec::new(),
        };

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "prefix" => args.prefix = Some(input.parse()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.tags = items.into_iter().collect();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown api_router attribute `{other}`"),
                    ));
                }
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(args)
    }
}

/// Expands `#[api_router]` over an inline module.
pub fn expand(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let args = match syn::parse::<ApiRouterArgs>(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let module = match syn::parse::<ItemMod>(item) {
        Ok(module) => module,
        Err(error) => return error.to_compile_error().into(),
    };

    match expand_module(args, module) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Builds the re-emitted module with an appended `router()` function.
fn expand_module(args: ApiRouterArgs, module: ItemMod) -> syn::Result<TokenStream> {
    let mut items = match &module.content {
        Some((_brace, items)) => items.clone(),
        None => {
            return Err(syn::Error::new_spanned(
                &module,
                "#[api_router] requires an inline module body",
            ));
        }
    };

    // Discover route handlers in source order.
    let route_fns: Vec<Ident> = items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(func) if func.attrs.iter().any(is_route_attr) => Some(func.sig.ident.clone()),
            _ => None,
        })
        .collect();

    // Inject the router prefix into each route attribute so that path parameters
    // declared in the prefix are classified correctly by the route macro.
    if let Some(prefix) = &args.prefix {
        let prefix_value = prefix.value();
        for item in &mut items {
            if let Item::Fn(func) = item {
                for attr in &mut func.attrs {
                    if is_route_attr(attr) {
                        inject_prefix_hint(attr, &prefix_value);
                    }
                }
            }
        }
    }

    let krate = krate();

    let prefix_call = match &args.prefix {
        Some(prefix) => quote! { .prefix(#prefix) },
        None => quote! {},
    };
    let tags_call = if args.tags.is_empty() {
        quote! {}
    } else {
        let tags = &args.tags;
        quote! { .tags(&[#(#tags),*]) }
    };
    let route_calls = route_fns.iter().map(|name| {
        let route_fn = format_ident!("__tork_route_{}", name);
        quote! { .route(#route_fn()) }
    });

    let router_fn = quote! {
        /// Builds the router for this module, including all of its routes.
        pub fn router() -> #krate::Router {
            #krate::Router::new()
                #prefix_call
                #tags_call
                #(#route_calls)*
        }
    };

    let attrs = &module.attrs;
    let vis = &module.vis;
    let ident = &module.ident;

    Ok(quote! {
        #(#attrs)*
        #vis mod #ident {
            #(#items)*

            #router_fn
        }
    })
}

/// Appends a hidden `__prefix = "<prefix>"` argument to a route attribute.
///
/// The route macro uses this only to classify path parameters; the route's
/// stored path remains the local one, so router composition is unaffected.
fn inject_prefix_hint(attr: &mut Attribute, prefix: &str) {
    if let Meta::List(list) = &mut attr.meta {
        let existing = &list.tokens;
        let prefix_lit = LitStr::new(prefix, Span::call_site());
        list.tokens = quote! { #existing, __prefix = #prefix_lit };
    }
}

/// Returns `true` if `attr` is one of the route or SSE macros.
///
/// Matches on the final path segment, so both `#[get]` and `#[tork::get]` are
/// recognized.
fn is_route_attr(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .map(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "sse" | "post_sse"
            )
        })
        .unwrap_or(false)
}
