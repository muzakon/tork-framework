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
    let handler_ident = format_ident!("__tork_handler_{}", fn_name);
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
    let call = quote! { #handler_ident(#(#call_args),*).await };

    let mut emit_func = func.clone();
    emit_func.sig.ident = handler_ident.clone();

    Ok(quote! {
        #emit_func

        #vis fn #fn_name() -> #krate::Route {
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

        #[doc(hidden)]
        #vis fn #route_fn() -> #krate::Route {
            #fn_name()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{parse_quote, parse_str};

    #[test]
    fn sse_args_parse_known_keys_and_reject_unknown_ones() {
        let args: SseArgs = parse_str(
            "\"/events\", method = POST, event = \"tick\", response_model = Tick, summary = \"sum\", description = \"desc\", tags = [\"stream\"], __prefix = \"/api\"",
        )
        .unwrap();
        assert_eq!(args.path.value(), "/events");
        assert_eq!(args.method.unwrap().to_string(), "POST");
        assert_eq!(args.event.unwrap().value(), "tick");
        assert_eq!(args.tags.len(), 1);
        assert_eq!(args.prefix_hint.unwrap().value(), "/api");

        let error = match parse_str::<SseArgs>("\"/events\", nope = 1") {
            Ok(_) => panic!("expected parse failure"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown sse attribute"));
    }

    #[test]
    fn expand_sse_emits_streaming_builder_and_event_override() {
        let args: SseArgs = parse_str(
            "\"/events/{room}\", method = POST, event = \"tick\", response_model = Tick, summary = \"sum\", description = \"desc\", tags = [\"stream\"]",
        )
        .unwrap();
        let func: ItemFn = parse_quote! {
            async fn events(room: String, user: AuthUser) -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(tokens.contains("Route :: new ("));
        assert!(tokens.contains("Method :: POST"));
        assert!(tokens.contains(". streaming ()"));
        assert!(tokens.contains(". response_schema :: < Tick > ()"));
        assert!(tokens.contains("IntoResponse :: into_response"));
        assert!(tokens.contains("\"tick\""));
        assert!(tokens.contains("__extract_path_param"));
    }

    #[test]
    fn sse_args_parse_rejects_missing_path() {
        let error = match parse_str::<SseArgs>("") {
            Ok(_) => panic!("empty input must fail"),
            Err(e) => e,
        };
        assert!(error.to_string().contains("expected a route path string"));
    }

    #[test]
    fn sse_args_parse_rejects_non_string_path() {
        let error = match parse_str::<SseArgs>("summary = \"x\"") {
            Ok(_) => panic!("non-string path must fail"),
            Err(e) => e,
        };
        assert!(error.to_string().contains("expected a route path string"));
    }

    #[test]
    fn sse_args_parse_tolerates_trailing_comma() {
        let args: SseArgs = parse_str("\"/events\",").unwrap();
        assert_eq!(args.path.value(), "/events");
        assert!(args.method.is_none());
        assert!(args.event.is_none());
        assert!(args.summary.is_none());
    }

    #[test]
    fn sse_args_parse_rejects_missing_equals() {
        let error = match parse_str::<SseArgs>("\"/events\", summary \"sum\"") {
            Ok(_) => panic!("missing `=` must fail"),
            Err(e) => e,
        };
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn sse_args_parse_rejects_invalid_tags_inner() {
        let error = match parse_str::<SseArgs>("\"/events\", tags = [123]") {
            Ok(_) => panic!("non-str tag must fail"),
            Err(e) => e,
        };
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn sse_args_parse_rejects_wrong_typed_values() {
        for input in [
            "\"/e\", method = 42",
            "\"/e\", event = 42",
            "\"/e\", response_model = \"string\"",
            "\"/e\", summary = 1",
            "\"/e\", description = 1",
            "\"/e\", __prefix = 1",
        ] {
            match parse_str::<SseArgs>(input) {
                Ok(_) => panic!("input must fail: {input}"),
                Err(e) => assert!(!e.to_string().is_empty(), "input: {input}"),
            }
        }
    }

    #[test]
    fn expand_uses_default_method_when_method_attribute_absent() {
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(tokens.contains("Method :: GET"));
    }

    #[test]
    fn expand_uses_default_method_post_sse() {
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("POST", args, func).unwrap().to_string();
        assert!(tokens.contains("Method :: POST"));
    }

    #[test]
    fn expand_with_prefix_hint_concatenates_path() {
        let args: SseArgs = parse_str("\"/events\", __prefix = \"/api\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(tokens.contains("Route :: new"));
        assert!(tokens.contains("Method :: GET"));
    }

    #[test]
    fn expand_emits_hidden_route_function() {
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(tokens.contains("__tork_route_events"));
        assert!(tokens.contains("__tork_handler_events"));
    }

    #[test]
    fn expand_omits_summary_description_event_response_model_when_absent() {
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(!tokens.contains(". summary ("), "summary should not be present");
        assert!(!tokens.contains(". description ("), "description should not be present");
        assert!(!tokens.contains(". event ("), "event should not be present");
        assert!(!tokens.contains(". response_schema"), "response_schema should not be present");
        assert!(!tokens.contains(". tag ("), "tag should not be present");
    }

    #[test]
    fn expand_with_empty_tags_emits_no_tag_calls() {
        let args: SseArgs = parse_str("\"/events\", tags = []").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(!tokens.contains(". tag ("));
    }

    #[test]
    fn expand_emits_handler_renamed_function() {
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events() -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        let tokens = expand_sse("GET", args, func).unwrap().to_string();
        assert!(tokens.contains("fn __tork_handler_events"));
    }

    #[test]
    fn expand_propagates_handler_parts_errors() {
        // `self` receiver is rejected by `build_handler_parts`.
        let args: SseArgs = parse_str("\"/events\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn events(self) -> tork::Result<tork::Sse<Tick>> { todo!() }
        };
        match expand_sse("GET", args, func) {
            Ok(_) => panic!("self receiver must fail"),
            Err(e) => assert!(!e.to_string().is_empty()),
        }
    }
}
