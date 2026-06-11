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

/// Parsed attributes of `#[websocket(...)]`.
struct WsArgs {
    path: LitStr,
    summary: Option<LitStr>,
    description: Option<LitStr>,
    tags: Vec<LitStr>,
    max_message_size: Option<usize>,
    max_frame_size: Option<usize>,
    idle_timeout_ms: Option<u64>,
    incoming: Option<Type>,
    outgoing: Option<Type>,
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
            incoming: None,
            outgoing: None,
            prefix_hint: None,
        };

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }

            let key: Ident = input.parse()?;
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
                "incoming" => args.incoming = Some(input.parse()?),
                "outgoing" => args.outgoing = Some(input.parse()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.tags = items.into_iter().collect();
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
                let #ident = #krate::WebSocket::from_request_context(&ctx, #config_expr)?;
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

    Ok(quote! {
        #func

        #[doc(hidden)]
        #vis fn #route_fn() -> #krate::Route {
            let handler: #krate::HandlerFn = ::std::sync::Arc::new(
                |ctx: #krate::RequestContext|
                    -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                    ::std::boxed::Box::pin(async move {
                        // Validate the handshake and resolve dependencies before
                        // the upgrade; any failure rejects with an HTTP error.
                        let __response = #krate::__ws_handshake(&ctx)?;
                        #(#bindings)*
                        #krate::__rt::spawn(async move {
                            if let ::core::result::Result::Err(__error) =
                                #fn_name(#(#call_args),*).await
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
