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
use syn::{bracketed, parenthesized, token, Attribute, Ident, Item, ItemMod, LitStr, Meta, Token};

use crate::common::krate;

/// Parsed attributes of `#[api_router(...)]`.
struct ApiRouterArgs {
    prefix: Option<LitStr>,
    tags: Vec<LitStr>,
    /// The router-level `throttle`, captured as the tokens following the key
    /// (either `= "name"` or `(...)`) so it can be re-injected into each route.
    throttle: Option<TokenStream>,
    /// The router-level `security` list, captured as `[ ... ]` so it can be
    /// re-injected into each HTTP route as `__router_security`.
    security: Option<TokenStream>,
}

impl Parse for ApiRouterArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ApiRouterArgs {
            prefix: None,
            tags: Vec::new(),
            throttle: None,
            security: None,
        };

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            // `throttle` accepts `= "name"` or a parenthesized `(...)` group;
            // capture it verbatim to re-inject into the routes.
            if key == "throttle" {
                if input.peek(token::Paren) {
                    let content;
                    parenthesized!(content in input);
                    let inner: TokenStream = content.parse()?;
                    args.throttle = Some(quote! { (#inner) });
                } else {
                    input.parse::<Token![=]>()?;
                    if input.peek(token::Bracket) {
                        let content;
                        bracketed!(content in input);
                        let inner: TokenStream = content.parse()?;
                        args.throttle = Some(quote! { = [#inner] });
                    } else {
                        let value: LitStr = input.parse()?;
                        args.throttle = Some(quote! { = #value });
                    }
                }
                if input.is_empty() {
                    break;
                }
                input.parse::<Token![,]>()?;
                continue;
            }

            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "prefix" => args.prefix = Some(input.parse()?),
                "tags" => {
                    let content;
                    bracketed!(content in input);
                    let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                    args.tags = items.into_iter().collect();
                }
                "security" => {
                    let content;
                    bracketed!(content in input);
                    let inner: TokenStream = content.parse()?;
                    args.security = Some(quote! { [#inner] });
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
pub fn expand(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
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

    // Inject the router-level throttle into each route as a hidden
    // `__router_throttle`, which a route applies only when it has no `throttle`
    // of its own — so endpoint policies override the router default for free.
    if let Some(throttle) = &args.throttle {
        for item in &mut items {
            if let Item::Fn(func) = item {
                for attr in &mut func.attrs {
                    if is_route_attr(attr) {
                        inject_router_throttle(attr, throttle);
                    }
                }
            }
        }
    }

    // Inject the router-level security into each HTTP route as a hidden
    // `__router_security`, applied only when the route declares none of its own.
    // SSE and WebSocket routes are skipped, since they do not document OpenAPI
    // security requirements.
    if let Some(security) = &args.security {
        for item in &mut items {
            if let Item::Fn(func) = item {
                for attr in &mut func.attrs {
                    if is_http_route_attr(attr) {
                        inject_router_security(attr, security);
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

/// Appends a hidden `__router_throttle <value>` argument to a route attribute,
/// where `<value>` is the captured `= "name"` or `(...)` from the router.
fn inject_router_throttle(attr: &mut Attribute, throttle: &TokenStream) {
    if let Meta::List(list) = &mut attr.meta {
        let existing = &list.tokens;
        list.tokens = quote! { #existing, __router_throttle #throttle };
    }
}

/// Appends a hidden `__router_security = [ ... ]` argument to a route attribute.
fn inject_router_security(attr: &mut Attribute, security: &TokenStream) {
    if let Meta::List(list) = &mut attr.meta {
        let existing = &list.tokens;
        list.tokens = quote! { #existing, __router_security = #security };
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
                "get" | "post" | "put" | "patch" | "delete" | "sse" | "post_sse" | "websocket"
            )
        })
        .unwrap_or(false)
}

/// Returns `true` if `attr` is one of the HTTP method route macros (not SSE or
/// WebSocket), which are the routes that carry OpenAPI security requirements.
fn is_http_route_attr(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .map(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "get" | "post" | "put" | "patch" | "delete"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn api_router_args_parse_and_reject_unknown_keys() {
        let args: ApiRouterArgs = syn::parse_quote!(prefix = "/v1", tags = ["users", "admin"]);
        assert_eq!(args.prefix.unwrap().value(), "/v1");
        assert_eq!(args.tags.len(), 2);

        let err = match syn::parse2::<ApiRouterArgs>(quote!(unknown = "x")) {
            Ok(_) => panic!("expected parse error"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown api_router attribute"));
    }

    #[test]
    fn inject_prefix_hint_appends_hidden_prefix_argument() {
        let mut attr: Attribute = parse_quote!(#[get("/users/{id}")]);
        inject_prefix_hint(&mut attr, "/api");
        assert!(quote!(#attr).to_string().contains("__prefix = \"/api\""));
    }

    #[test]
    fn api_router_parses_security_and_injects_it_into_http_routes() {
        let args: ApiRouterArgs = parse_quote!(prefix = "/api", security = ["bearerAuth"]);
        assert!(args.security.is_some());

        // A bracket list capture is re-injected onto an HTTP route only.
        let security = args.security.unwrap();
        let mut get_attr: Attribute = parse_quote!(#[get("/")]);
        inject_router_security(&mut get_attr, &security);
        assert!(quote!(#get_attr)
            .to_string()
            .contains("__router_security = [\"bearerAuth\"]"));

        // SSE and WebSocket attrs are not HTTP routes, so security is not injected.
        let sse_attr: Attribute = parse_quote!(#[tork::sse("/stream")]);
        assert!(!is_http_route_attr(&sse_attr));
        let get_only: Attribute = parse_quote!(#[get("/")]);
        assert!(is_http_route_attr(&get_only));
    }

    #[test]
    fn is_route_attr_matches_supported_macros() {
        let get_attr: Attribute = parse_quote!(#[get("/")]);
        let sse_attr: Attribute = parse_quote!(#[tork::sse("/stream")]);
        let ws_attr: Attribute = parse_quote!(#[tork::websocket("/ws")]);
        let post_sse_attr: Attribute = parse_quote!(#[tork::post_sse("/stream")]);
        let put_attr: Attribute = parse_quote!(#[put("/")]);
        let patch_attr: Attribute = parse_quote!(#[patch("/")]);
        let delete_attr: Attribute = parse_quote!(#[delete("/")]);
        let other: Attribute = parse_quote!(#[derive(Clone)]);
        assert!(is_route_attr(&get_attr));
        assert!(is_route_attr(&sse_attr));
        assert!(is_route_attr(&ws_attr));
        assert!(is_route_attr(&post_sse_attr));
        assert!(is_route_attr(&put_attr));
        assert!(is_route_attr(&patch_attr));
        assert!(is_route_attr(&delete_attr));
        assert!(!is_route_attr(&other));
    }

    #[test]
    fn expand_module_rejects_non_inline_modules() {
        let args: ApiRouterArgs = syn::parse_quote!(prefix = "/v1");
        let module: ItemMod = parse_quote!(
            mod users;
        );
        assert!(expand_module(args, module)
            .unwrap_err()
            .to_string()
            .contains("inline module body"));
    }

    #[test]
    fn expand_module_injects_prefix_and_builds_router_fn() {
        let args: ApiRouterArgs = syn::parse_quote!(prefix = "/v1", tags = ["users"]);
        let module: ItemMod = parse_quote! {
            pub mod users {
                #[get("/{id}")]
                async fn show() -> &'static str { "ok" }

                fn helper() {}

                #[tork::websocket("/live")]
                async fn live(ws: tork::WebSocket) -> tork::Result<()> { let _ = ws; Ok(()) }
            }
        };
        let tokens = expand_module(args, module).unwrap().to_string();
        assert!(tokens.contains("pub mod users"));
        assert!(tokens.contains("pub fn router"));
        assert!(tokens.contains("prefix"));
        assert!(tokens.contains("/v1"));
        assert!(tokens.contains("tags"));
        assert!(tokens.contains("users"));
        assert!(tokens.contains("__tork_route_show"));
        assert!(tokens.contains("__tork_route_live"));
        assert!(tokens.contains("__prefix = \"/v1\""));
    }
}
