//! The `#[sse]` and `#[post_sse]` macros.
//!
//! Like the route macros, these leave the handler function untouched and emit a
//! hidden `__tork_route_<fn>` registration function. The handler returns
//! `Result<Sse<T>>`; the generated glue applies the default event name and turns
//! the `Sse` into a streaming `text/event-stream` response.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, ItemFn, LitStr, Token, Type, bracketed};

use crate::common::krate;
use crate::route::{HandlerParts, build_handler_parts};

/// Parsed attributes of an `#[sse]` / `#[post_sse]` macro.
struct SseArgs {
    path: LitStr,
    method: Option<Ident>,
    event: Option<LitStr>,
    response_model: Option<Type>,
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
    /// Enclosing router prefix, injected by `#[api_router]`.
    prefix_hint: Option<LitStr>,
}

impl Parse for SseArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "expected a route path string as the first argument, e.g. #[sse(\"/events\")]",
            )
        })?;

        let mut args = SseArgs {
            path,
            method: None,
            event: None,
            response_model: None,
            summary: None,
            description: None,
            tags: Vec::new(),
            prefix_hint: None,
        };

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break; // tolerate a trailing comma
            }

            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "method" => args.method = Some(input.parse()?),
                "event" => args.event = Some(input.parse()?),
                "response_model" => args.response_model = Some(input.parse()?),
                "summary" => args.summary = Some(input.parse()?),
                "description" => args.description = Some(input.parse()?),
                "__prefix" => args.prefix_hint = Some(input.parse()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.tags = items.into_iter().collect();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown sse attribute `{other}`"),
                    ));
                }
            }
        }

        Ok(args)
    }
}

/// Shared implementation for `#[sse]` (default `GET`) and `#[post_sse]` (`POST`).
pub fn expand(
    default_method: &str,
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = match syn::parse::<SseArgs>(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let func = match syn::parse::<ItemFn>(item) {
        Ok(func) => func,
        Err(error) => return error.to_compile_error().into(),
    };

    match expand_sse(default_method, args, func) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Builds the streaming handler closure and the registration function.
fn expand_sse(default_method: &str, args: SseArgs, func: ItemFn) -> syn::Result<TokenStream> {
    let krate = krate();
    let fn_name = func.sig.ident.clone();
    let vis = func.vis.clone();
    let route_fn = format_ident!("__tork_route_{}", fn_name);

    let method = args
        .method
        .as_ref()
        .map(|ident| ident.to_string().to_uppercase())
        .unwrap_or_else(|| default_method.to_owned());
    let method_ident = Ident::new(&method, Span::call_site());

    let path = &args.path;
    let full_path = match &args.prefix_hint {
        Some(prefix) => format!("{}{}", prefix.value(), args.path.value()),
        None => args.path.value(),
    };

    let HandlerParts {
        bindings,
        call_args,
        ..
    } = build_handler_parts(&krate, &func, &full_path)?;

    let mut builder = quote! { #krate::Route::new(#krate::Method::#method_ident, #path, handler) };
    if let Some(summary) = &args.summary {
        builder = quote! { #builder.summary(#summary) };
    }
    if let Some(description) = &args.description {
        builder = quote! { #builder.description(#description) };
    }
    for tag in &args.tags {
        builder = quote! { #builder.tag(#tag) };
    }
    // SSE responses are documented as a `text/event-stream` of the data type.
    builder = quote! { #builder.streaming() };
    if let Some(response_model) = &args.response_model {
        builder = quote! { #builder.response_schema::<#response_model>() };
    }

    // The `event` attribute sets the stream's default event name.
    let apply_event = match &args.event {
        Some(event) => quote! { .event(#event) },
        None => quote! {},
    };
    let call = quote! { #fn_name(#(#call_args),*).await };

    Ok(quote! {
        #func

        #[doc(hidden)]
        #vis fn #route_fn() -> #krate::Route {
            let handler: #krate::HandlerFn = ::std::sync::Arc::new(
                |ctx: #krate::RequestContext|
                    -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                    ::std::boxed::Box::pin(async move {
                        #(#bindings)*
                        match #call {
                            ::core::result::Result::Ok(sse) => ::core::result::Result::Ok(
                                #krate::IntoResponse::into_response(sse #apply_event),
                            ),
                            ::core::result::Result::Err(error) => {
                                ::core::result::Result::Err(error)
                            }
                        }
                    })
                },
            );
            #builder
        }
    })
}
