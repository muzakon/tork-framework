//! The `#[websocket]` macro.
//!
//! Generates a `GET` route whose handler validates the WebSocket handshake,
//! resolves dependencies, returns `101 Switching Protocols`, and spawns a task
//! that drives the user's handler over the upgraded connection. The handler takes
//! a `WebSocket` parameter and returns `tork::Result<()>`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{FnArg, Ident, ItemFn, LitStr, Pat, Token, Type, bracketed};

use crate::common::{krate, parse_duration_ms, parse_size, path_param_names};
use crate::route::{RouteThrottle, parse_throttle, throttle_check_tokens};

/// Parsed attributes of `#[websocket(...)]`.
struct WsArgs {
    path: LitStr,
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
    max_message_size: Option<usize>,
    max_frame_size: Option<usize>,
    idle_timeout_ms: Option<u64>,
    origins: Vec<LitStr>,
    allow_any_origin: bool,
    incoming: Option<Type>,
    outgoing: Option<Type>,
    throttle: Option<RouteThrottle>,
    router_throttle: Option<RouteThrottle>,
    /// Enclosing router prefix, injected by `#[api_router]`.
    prefix_hint: Option<LitStr>,
}

impl Parse for WsArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "expected a route path string as the first argument, e.g. #[websocket(\"/ws\")]",
            )
        })?;

        let mut args = WsArgs {
            path,
            summary: None,
            description: None,
            tags: Vec::new(),
            max_message_size: None,
            max_frame_size: None,
            idle_timeout_ms: None,
            origins: Vec::new(),
            allow_any_origin: false,
            incoming: None,
            outgoing: None,
            throttle: None,
            router_throttle: None,
            prefix_hint: None,
        };

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }

            let key: Ident = input.parse()?;

            if key == "throttle" {
                args.throttle = Some(parse_throttle(input)?);
                continue;
            }
            if key == "__router_throttle" {
                args.router_throttle = Some(parse_throttle(input)?);
                continue;
            }

            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "summary" => args.summary = Some(input.parse()?),
                "description" => args.description = Some(input.parse()?),
                "__prefix" => args.prefix_hint = Some(input.parse()?),
                "max_message_size" => {
                    let value: LitStr = input.parse()?;
                    args.max_message_size = Some(parse_size(&value)?);
                }
                "max_frame_size" => {
                    let value: LitStr = input.parse()?;
                    args.max_frame_size = Some(parse_size(&value)?);
                }
                "idle_timeout" => {
                    let value: LitStr = input.parse()?;
                    args.idle_timeout_ms = Some(parse_duration_ms(&value)?);
                }
                "allow_any_origin" => {
                    let value: syn::LitBool = input.parse()?;
                    args.allow_any_origin = value.value;
                }
                "incoming" => args.incoming = Some(input.parse()?),
                "outgoing" => args.outgoing = Some(input.parse()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.tags = items.into_iter().collect();
                }
                "origins" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.origins = items.into_iter().collect();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown websocket attribute `{other}`"),
                    ));
                }
            }
        }

        Ok(args)
    }
}

/// Expands `#[websocket(...)]`.
pub fn expand(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let args = match syn::parse::<WsArgs>(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let func = match syn::parse::<ItemFn>(item) {
        Ok(func) => func,
        Err(error) => return error.to_compile_error().into(),
    };

    match expand_ws(args, func) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Builds the upgrade handler and the registration function.
fn expand_ws(args: WsArgs, func: ItemFn) -> syn::Result<TokenStream> {
    let krate = krate();
    let fn_name = func.sig.ident.clone();
    let vis = func.vis.clone();
    let handler_ident = format_ident!("__tork_handler_{}", fn_name);
    let route_fn = format_ident!("__tork_route_{}", fn_name);
    let path = &args.path;

    let full_path = match &args.prefix_hint {
        Some(prefix) => format!("{}{}", prefix.value(), args.path.value()),
        None => args.path.value(),
    };
    let placeholders = path_param_names(&full_path);

    // Build the route-level WebSocket config expression from the size/timeout attrs.
    let mut config_expr = quote! { #krate::WebSocketConfig::new() };
    if let Some(bytes) = args.max_message_size {
        config_expr = quote! { #config_expr.max_message_size(#bytes) };
    }
    if let Some(bytes) = args.max_frame_size {
        config_expr = quote! { #config_expr.max_frame_size(#bytes) };
    }
    if let Some(ms) = args.idle_timeout_ms {
        config_expr =
            quote! { #config_expr.idle_timeout(::core::time::Duration::from_millis(#ms)) };
    }
    for origin in &args.origins {
        config_expr = quote! { #config_expr.allow_origin(#origin) };
    }
    if args.allow_any_origin {
        config_expr = quote! { #config_expr.allow_any_origin() };
    }

    // Bind each parameter: the `WebSocket` parameter is the socket, the rest are
    // path parameters or dependencies. Bindings run before the upgrade is spawned.
    let mut bindings = Vec::new();
    let mut call_args = Vec::new();
    let mut socket_count = 0usize;

    for input in &func.sig.inputs {
        let pat_type = match input {
            FnArg::Typed(pat_type) => pat_type,
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "websocket handlers cannot take `self`",
                ));
            }
        };
        let ident = match pat_type.pat.as_ref() {
            Pat::Ident(pat_ident) => pat_ident.ident.clone(),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "websocket handler parameters must be simple identifiers",
                ));
            }
        };
        let ty = pat_type.ty.as_ref();
        let name = ident.to_string();

        if is_websocket_type(ty) {
            socket_count += 1;
            bindings.push(quote! {
                let #ident = #krate::WebSocket::from_request_context(&ctx, __ws_config.clone())?;
            });
        } else if placeholders.contains(&name) {
            bindings.push(quote! {
                let #ident: #ty = #krate::__extract_path_param(&ctx, #name)?;
            });
        } else {
            bindings.push(quote! {
                let #ident = <#ty as #krate::FromRequest>::from_request(&ctx).await?;
            });
        }

        call_args.push(ident);
    }

    if socket_count != 1 {
        return Err(syn::Error::new_spanned(
            &func.sig,
            "a #[websocket] handler must take exactly one `WebSocket` parameter",
        ));
    }

    let method_ident = Ident::new("GET", Span::call_site());
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
    // Mark the route as a WebSocket channel and record its message schemas for
    // AsyncAPI documentation.
    builder = quote! { #builder.websocket() };
    if let Some(incoming) = &args.incoming {
        builder = quote! { #builder.ws_incoming::<#incoming>() };
    }
    if let Some(outgoing) = &args.outgoing {
        builder = quote! { #builder.ws_outgoing::<#outgoing>() };
    }

    let throttle_check = throttle_check_tokens(
        &krate,
        args.throttle.as_ref(),
        args.router_throttle.as_ref(),
        "GET",
        &full_path,
    );

    let mut emit_func = func.clone();
    emit_func.sig.ident = handler_ident.clone();

    Ok(quote! {
        #emit_func

        #vis fn #fn_name() -> #krate::Route {
            let handler: #krate::HandlerFn = ::std::sync::Arc::new(
                |ctx: #krate::RequestContext|
                    -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                    ::std::boxed::Box::pin(async move {
                        #throttle_check
                        let __ws_config = #config_expr;
                        // Validate the handshake and resolve dependencies before
                        // the upgrade; any failure rejects with an HTTP error.
                        let __response = #krate::__ws_handshake(&ctx, __ws_config.clone())?;
                        #(#bindings)*
                        #krate::__rt::spawn(async move {
                            if let ::core::result::Result::Err(__error) =
                                #handler_ident(#(#call_args),*).await
                            {
                                ::std::eprintln!("tork: websocket handler error: {__error}");
                            }
                        });
                        ::core::result::Result::Ok(__response)
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

/// Returns `true` if `ty`'s final path segment is `WebSocket`.
fn is_websocket_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "WebSocket";
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{parse_quote, parse_str};

    #[test]
    fn ws_args_parse_options_and_reject_unknown_keys() {
        let args: WsArgs = parse_str(
            "\"/ws\", summary = \"sum\", description = \"desc\", max_message_size = \"2kb\", max_frame_size = \"1kb\", idle_timeout = \"5s\", incoming = Inbound, outgoing = Outbound, tags = [\"chat\"], origins = [\"https://app.example.com\"], allow_any_origin = true, __prefix = \"/api\"",
        )
        .unwrap();
        assert_eq!(args.path.value(), "/ws");
        assert_eq!(args.summary.unwrap().value(), "sum");
        assert_eq!(args.max_message_size, Some(2048));
        assert_eq!(args.max_frame_size, Some(1024));
        assert_eq!(args.idle_timeout_ms, Some(5000));
        assert_eq!(args.tags.len(), 1);
        assert_eq!(args.origins.len(), 1);
        assert!(args.allow_any_origin);
        assert_eq!(args.prefix_hint.unwrap().value(), "/api");

        let error = match parse_str::<WsArgs>("\"/ws\", nope = 1") {
            Ok(_) => panic!("expected parse failure"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown websocket attribute"));
    }

    #[test]
    fn websocket_type_detection_and_expand_errors_are_covered() {
        assert!(is_websocket_type(&parse_quote!(tork::WebSocket)));
        assert!(!is_websocket_type(&parse_quote!(String)));

        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn handler(self) -> tork::Result<()> { Ok(()) }
        };
        assert!(expand_ws(args, func)
            .unwrap_err()
            .to_string()
            .contains("cannot take `self`"));

        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn handler((socket): tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        assert!(expand_ws(args, func)
            .unwrap_err()
            .to_string()
            .contains("simple identifiers"));

        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn handler(id: String) -> tork::Result<()> { Ok(()) }
        };
        assert!(expand_ws(args, func)
            .unwrap_err()
            .to_string()
            .contains("exactly one `WebSocket` parameter"));
    }

    #[test]
    fn expand_ws_emits_handshake_bindings_and_metadata() {
        let args: WsArgs = parse_str(
            "\"/ws/{room}\", summary = \"sum\", description = \"desc\", max_message_size = \"2kb\", max_frame_size = \"1kb\", idle_timeout = \"10s\", incoming = InMsg, outgoing = OutMsg, tags = [\"chat\"], origins = [\"https://app.example.com\"]",
        )
        .unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket, room: String, user: AuthUser) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("__ws_handshake"));
        assert!(tokens.contains("WebSocketConfig :: new () . max_message_size"));
        assert!(tokens.contains(". allow_origin (\"https://app.example.com\")"));
        assert!(tokens.contains("__extract_path_param"));
        assert!(tokens.contains("FromRequest"));
        assert!(tokens.contains(". websocket ()"));
        assert!(tokens.contains(". ws_incoming :: < InMsg > ()"));
        assert!(tokens.contains(". ws_outgoing :: < OutMsg > ()"));
    }

    #[test]
    fn ws_args_parse_rejects_missing_path() {
        let error = match parse_str::<WsArgs>("") {
            Ok(_) => panic!("empty input must fail"),
            Err(e) => e,
        };
        assert!(error.to_string().contains("expected a route path string"));
    }

    #[test]
    fn ws_args_parse_rejects_non_string_path() {
        let error = match parse_str::<WsArgs>("summary = \"x\"") {
            Ok(_) => panic!("non-string path must fail"),
            Err(e) => e,
        };
        assert!(error.to_string().contains("expected a route path string"));
    }

    #[test]
    fn ws_args_parse_tolerates_trailing_comma() {
        let args: WsArgs = parse_str("\"/ws\",").unwrap();
        assert_eq!(args.path.value(), "/ws");
    }

    #[test]
    fn ws_args_parse_rejects_wrong_typed_values() {
        for input in [
            "\"/ws\", summary = 1",
            "\"/ws\", description = 1",
            "\"/ws\", incoming = 42",
            "\"/ws\", outgoing = 42",
        ] {
            assert!(
                parse_str::<WsArgs>(input).is_err(),
                "input must fail: {input}"
            );
        }
    }

    #[test]
    fn ws_args_parse_rejects_invalid_size_and_duration() {
        assert!(parse_str::<WsArgs>("\"/ws\", max_message_size = \"oops\"").is_err());
        assert!(parse_str::<WsArgs>("\"/ws\", max_frame_size = \"wat\"").is_err());
        assert!(parse_str::<WsArgs>("\"/ws\", idle_timeout = \"soon\"").is_err());
    }

    #[test]
    fn expand_ws_with_prefix_hint_concatenates_path() {
        let args: WsArgs = parse_str("\"/{room}\", __prefix = \"/api\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket, room: String) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("__extract_path_param"));
        assert!(tokens.contains("\"room\""));
    }

    #[test]
    fn expand_ws_emits_hidden_route_and_handler_functions() {
        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("__tork_route_chat"));
        assert!(tokens.contains("__tork_handler_chat"));
        assert!(tokens.contains("Method :: GET"));
    }

    #[test]
    fn expand_ws_max_message_size_alone() {
        let args: WsArgs = parse_str("\"/ws\", max_message_size = \"2kb\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("max_message_size"));
        assert!(tokens.contains("2048"));
        assert!(!tokens.contains("max_frame_size"));
        assert!(!tokens.contains("idle_timeout"));
    }

    #[test]
    fn expand_ws_max_frame_size_alone() {
        let args: WsArgs = parse_str("\"/ws\", max_frame_size = \"1kb\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("max_frame_size"));
        assert!(tokens.contains("1024"));
        assert!(!tokens.contains("max_message_size"));
        assert!(!tokens.contains("idle_timeout"));
    }

    #[test]
    fn expand_ws_idle_timeout_alone() {
        let args: WsArgs = parse_str("\"/ws\", idle_timeout = \"5s\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("idle_timeout"));
        assert!(tokens.contains("5000"));
        assert!(!tokens.contains("max_message_size"));
        assert!(!tokens.contains("max_frame_size"));
    }

    #[test]
    fn expand_ws_allow_any_origin_alone() {
        let args: WsArgs = parse_str("\"/ws\", allow_any_origin = true").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("allow_any_origin"));
    }

    #[test]
    fn expand_ws_rejects_more_than_one_websocket_param() {
        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(a: tork::WebSocket, b: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let error = match expand_ws(args, func) {
            Ok(_) => panic!("two WebSocket params must fail"),
            Err(e) => e,
        };
        assert!(error.to_string().contains("exactly one `WebSocket` parameter"));
    }

    #[test]
    fn expand_ws_omits_optional_attrs_when_absent() {
        let args: WsArgs = parse_str("\"/ws\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(!tokens.contains("summary"), "summary should not appear");
        assert!(!tokens.contains("description"), "description should not appear");
        assert!(!tokens.contains("tag"), "tag should not appear");
        assert!(!tokens.contains("max_message_size"));
        assert!(!tokens.contains("max_frame_size"));
        assert!(!tokens.contains("idle_timeout"));
        assert!(!tokens.contains("ws_incoming"));
        assert!(!tokens.contains("ws_outgoing"));
    }

    #[test]
    fn expand_ws_emits_description_only() {
        let args: WsArgs = parse_str("\"/ws\", description = \"d\"").unwrap();
        let func: ItemFn = parse_quote! {
            async fn chat(socket: tork::WebSocket) -> tork::Result<()> { Ok(()) }
        };
        let tokens = expand_ws(args, func).unwrap().to_string();
        assert!(tokens.contains("description"));
        assert!(tokens.contains("\"d\""));
    }

    #[test]
    fn is_websocket_type_rejects_non_path_types() {
        assert!(!is_websocket_type(&parse_quote!(&str)));
        assert!(!is_websocket_type(&parse_quote!(String)));
        assert!(!is_websocket_type(&parse_quote!(Vec<u8>)));
    }
}
