//! The HTTP method route macros (`#[get]`, `#[post]`, ...).
//!
//! A route macro leaves the handler function untouched and emits, next to it, a
//! hidden registration function named `__tork_route_<fn>` that returns a
//! `tork::Route`. The `#[api_router]` module macro discovers and calls these.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, ExprLit, FnArg, GenericArgument, Ident, ItemFn, Lit, LitStr, Pat, Path,
    PathArguments, ReturnType, Token, Type, bracketed,
};

use crate::common::{krate, path_param_names};

/// Parsed attributes of a route macro.
struct RouteArgs {
    path: LitStr,
    response_model: Option<Type>,
    summary: Option<LitStr>,
    description: Option<LitStr>,
    status_code: Option<Expr>,
    tags: Vec<LitStr>,
    /// Scoped observability hooks attached to this route, by function path. Each
    /// kind is repeatable.
    on_request: Vec<Path>,
    on_response: Vec<Path>,
    on_error: Vec<Path>,
    on_validation_error: Vec<Path>,
    /// Enclosing router prefix, injected by `#[api_router]` so that path
    /// parameters declared in the prefix are classified correctly. Not part of
    /// the public attribute surface.
    prefix_hint: Option<LitStr>,
}

impl Parse for RouteArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "expected a route path string as the first argument, e.g. #[get(\"/users\")]",
            )
        })?;

        let mut args = RouteArgs {
            path,
            response_model: None,
            summary: None,
            description: None,
            status_code: None,
            tags: Vec::new(),
            on_request: Vec::new(),
            on_response: Vec::new(),
            on_error: Vec::new(),
            on_validation_error: Vec::new(),
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
                "response_model" => args.response_model = Some(input.parse()?),
                "summary" => args.summary = Some(input.parse()?),
                "description" => args.description = Some(input.parse()?),
                "status_code" => args.status_code = Some(input.parse()?),
                "on_request" => args.on_request.push(input.parse()?),
                "on_response" => args.on_response.push(input.parse()?),
                "on_error" => args.on_error.push(input.parse()?),
                "on_validation_error" => args.on_validation_error.push(input.parse()?),
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
                        format!("unknown route attribute `{other}`"),
                    ));
                }
            }
        }

        Ok(args)
    }
}

/// Shared implementation for every method macro.
pub fn route_impl(
    method: &str,
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = match syn::parse::<RouteArgs>(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let func = match syn::parse::<ItemFn>(item) {
        Ok(func) => func,
        Err(error) => return error.to_compile_error().into(),
    };

    match expand_route(method, args, func) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Builds the handler closure and the registration function.
fn expand_route(method: &str, args: RouteArgs, func: ItemFn) -> syn::Result<TokenStream> {
    let krate = krate();
    let fn_name = func.sig.ident.clone();
    let vis = func.vis.clone();
    let route_fn = format_ident!("__tork_route_{}", fn_name);
    let method_ident = Ident::new(method, Span::call_site());
    let path = &args.path;

    // Classify against the full path (enclosing prefix + local path), so path
    // parameters declared in an `#[api_router]` prefix are recognized.
    let full_path = match &args.prefix_hint {
        Some(prefix) => format!("{}{}", prefix.value(), args.path.value()),
        None => args.path.value(),
    };
    // A handler with `#[file]` / `#[form]` parameters (or bare file types) reads a
    // multipart body once and distributes the fields; otherwise the parameters are
    // path parameters and dependencies.
    let multipart = has_form_params(&func);
    let (bindings, call_args, prelude, request_body) = if multipart {
        let MultipartParts {
            bindings,
            call_args,
            prelude,
        } = build_multipart_parts(&krate, &func, &full_path)?;
        (bindings, call_args, prelude, None)
    } else {
        let HandlerParts {
            bindings,
            call_args,
            request_body,
        } = build_handler_parts(&krate, &func, &full_path)?;
        (bindings, call_args, TokenStream::new(), request_body)
    };

    // Re-emit the function without the form parameter attributes so it compiles.
    let emit_func = strip_form_attrs(&func);

    let status_code = status_code_tokens(&krate, args.status_code.as_ref());

    // The response schema comes from the declared response model, or otherwise
    // from the handler's `Result<T>` inner type.
    let return_inner = result_ok_type(&func.sig.output);
    let response_schema_ty = args
        .response_model
        .clone()
        .or_else(|| return_inner.clone());

    // Builder chain for the route's metadata.
    let mut builder = quote! {
        #krate::Route::new(#krate::Method::#method_ident, #path, handler)
            .status_code(#status_code)
    };
    if let Some(summary) = &args.summary {
        builder = quote! { #builder.summary(#summary) };
    }
    if let Some(description) = &args.description {
        builder = quote! { #builder.description(#description) };
    }
    for tag in &args.tags {
        builder = quote! { #builder.tag(#tag) };
    }
    if let Some(response_model) = &args.response_model {
        builder = quote! { #builder.response_model::<#response_model>() };
    }
    if let Some(body) = &request_body {
        builder = quote! { #builder.request_schema::<#body>() };
    }
    if let Some(schema_ty) = &response_schema_ty {
        builder = quote! { #builder.response_schema::<#schema_ty>() };
    }

    // Scoped, route-level observability hooks, by function path.
    for hook in &args.on_request {
        builder = quote! { #builder.on_request(#hook) };
    }
    for hook in &args.on_response {
        builder = quote! { #builder.on_response(#hook) };
    }
    for hook in &args.on_error {
        builder = quote! { #builder.on_error(#hook) };
    }
    for hook in &args.on_validation_error {
        builder = quote! { #builder.on_validation_error(#hook) };
    }

    // When a response model is declared, convert the handler's value into it
    // before serializing; otherwise serialize the value as-is.
    let call = quote! { #fn_name(#(#call_args),*).await };
    let finish = match &args.response_model {
        Some(response_model) => {
            quote! { #krate::__finish_into::<_, #response_model, _>(#call, #status_code) }
        }
        None => quote! { #krate::__finish(#call, #status_code) },
    };

    Ok(quote! {
        #emit_func

        #[doc(hidden)]
        #vis fn #route_fn() -> #krate::Route {
            let handler: #krate::HandlerFn = ::std::sync::Arc::new(
                |ctx: #krate::RequestContext|
                    -> #krate::BoxFuture<'static, #krate::Result<#krate::Response>> {
                    ::std::boxed::Box::pin(async move {
                        #prelude
                        #(#bindings)*
                        #finish
                    })
                },
            );
            #builder
        }
    })
}

/// The pieces of a generated handler closure: per-parameter bindings, the call
/// argument list, and the detected request-body type (if any).
pub(crate) struct HandlerParts {
    pub bindings: Vec<TokenStream>,
    pub call_args: Vec<Ident>,
    pub request_body: Option<Type>,
}

/// Builds the handler bindings shared by the route and SSE macros.
///
/// Each parameter becomes a binding: a path parameter when its name matches a
/// `{placeholder}` in `full_path`, otherwise a dependency resolved through
/// `FromRequest`. A `Valid<T>` / `Json<T>` parameter is recorded as the request
/// body. Errors propagate as `Err` so the dispatch boundary renders them.
pub(crate) fn build_handler_parts(
    krate: &TokenStream,
    func: &ItemFn,
    full_path: &str,
) -> syn::Result<HandlerParts> {
    let placeholders = path_param_names(full_path);

    let mut bindings = Vec::new();
    let mut call_args = Vec::new();
    let mut request_body: Option<Type> = None;

    for input in &func.sig.inputs {
        let pat_type = match input {
            FnArg::Typed(pat_type) => pat_type,
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(
                    receiver,
                    "route handlers cannot take `self`",
                ));
            }
        };

        let ident = match pat_type.pat.as_ref() {
            Pat::Ident(pat_ident) => pat_ident.ident.clone(),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "route handler parameters must be simple identifiers",
                ));
            }
        };
        let ty = pat_type.ty.as_ref();
        let name = ident.to_string();

        if placeholders.contains(&name) {
            bindings.push(quote! {
                let #ident: #ty = match #krate::__extract_path_param(&ctx, #name) {
                    ::core::result::Result::Ok(value) => value,
                    ::core::result::Result::Err(error) => {
                        return ::core::result::Result::Err(error);
                    }
                };
            });
        } else {
            if let Some(body) = body_inner_type(ty) {
                if request_body.is_some() {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "a handler may declare at most one request body",
                    ));
                }
                request_body = Some(body.clone());
            }
            bindings.push(quote! {
                let #ident = match <#ty as #krate::FromRequest>::from_request(&ctx).await {
                    ::core::result::Result::Ok(value) => value,
                    ::core::result::Result::Err(error) => {
                        return ::core::result::Result::Err(error);
                    }
                };
            });
        }

        call_args.push(ident);
    }

    Ok(HandlerParts {
        bindings,
        call_args,
        request_body,
    })
}

/// The generated pieces for a multipart (form/file) handler.
pub(crate) struct MultipartParts {
    pub bindings: Vec<TokenStream>,
    pub call_args: Vec<Ident>,
    pub prelude: TokenStream,
}

/// Whether a handler has any `#[file]` / `#[form]` parameter or bare file type.
fn has_form_params(func: &ItemFn) -> bool {
    func.sig.inputs.iter().any(|input| match input {
        FnArg::Typed(pat_type) => {
            pat_type
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("file") || attr.path().is_ident("form"))
                || file_kind(unwrap_multiplicity(&pat_type.ty).1).is_some()
        }
        FnArg::Receiver(_) => false,
    })
}

/// Builds the bindings for a multipart handler: one parse, then a take per field.
fn build_multipart_parts(
    krate: &TokenStream,
    func: &ItemFn,
    full_path: &str,
) -> syn::Result<MultipartParts> {
    let placeholders = path_param_names(full_path);
    let mut bindings = Vec::new();
    let mut call_args = Vec::new();

    for input in &func.sig.inputs {
        let pat_type = match input {
            FnArg::Typed(pat_type) => pat_type,
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new_spanned(receiver, "route handlers cannot take `self`"));
            }
        };
        let ident = match pat_type.pat.as_ref() {
            Pat::Ident(pat_ident) => pat_ident.ident.clone(),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "route handler parameters must be simple identifiers",
                ));
            }
        };
        let ty = pat_type.ty.as_ref();
        let name = ident.to_string();

        // A multipart handler cannot also declare another body encoding.
        if body_inner_type(ty).is_some() || first_generic_arg(ty, &["Form", "Multipart"]).is_some() {
            return Err(syn::Error::new_spanned(
                ty,
                "a request can only have one body encoding: a handler with #[file]/#[form] \
                 parameters cannot also take a JSON, Form, or Multipart body",
            ));
        }

        let file_attr = pat_type.attrs.iter().find(|attr| attr.path().is_ident("file"));
        let form_attr = pat_type.attrs.iter().find(|attr| attr.path().is_ident("form"));
        let (multiplicity, inner) = unwrap_multiplicity(ty);

        if file_attr.is_some() || file_kind(inner).is_some() {
            let kind = file_kind(inner).ok_or_else(|| {
                syn::Error::new_spanned(
                    ty,
                    "a #[file] parameter must be FileBytes or UploadFile (optionally Option/Vec)",
                )
            })?;
            let field = attr_name(file_attr).unwrap_or_else(|| name.clone());
            bindings.push(file_binding(krate, &ident, kind, multiplicity, &field));
        } else if let Some(attr) = form_attr {
            let field = attr_name(Some(attr)).unwrap_or_else(|| name.clone());
            bindings.push(text_binding(krate, &ident, inner, multiplicity, &field));
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

    let prelude = quote! {
        let mut __form = #krate::__parse_multipart(&ctx, #krate::UploadConfig::new()).await?;
    };
    Ok(MultipartParts {
        bindings,
        call_args,
        prelude,
    })
}

/// Whether a parameter binds one value, an optional value, or many.
#[derive(Clone, Copy)]
enum Multiplicity {
    One,
    Optional,
    Many,
}

/// Which file type a parameter binds.
#[derive(Clone, Copy)]
enum FileKind {
    Bytes,
    Upload,
}

/// Splits a type into its multiplicity and inner type.
fn unwrap_multiplicity(ty: &Type) -> (Multiplicity, &Type) {
    if let Some(inner) = first_generic_arg(ty, &["Option"]) {
        (Multiplicity::Optional, inner)
    } else if let Some(inner) = first_generic_arg(ty, &["Vec"]) {
        (Multiplicity::Many, inner)
    } else {
        (Multiplicity::One, ty)
    }
}

/// Returns the file kind if `ty` is `FileBytes` or `UploadFile`.
fn file_kind(ty: &Type) -> Option<FileKind> {
    let Type::Path(path) = ty else { return None };
    match path.path.segments.last()?.ident.to_string().as_str() {
        "FileBytes" => Some(FileKind::Bytes),
        "UploadFile" => Some(FileKind::Upload),
        _ => None,
    }
}

/// Builds the binding for a file parameter from `__form`.
fn file_binding(
    krate: &TokenStream,
    ident: &Ident,
    kind: FileKind,
    multiplicity: Multiplicity,
    name: &str,
) -> TokenStream {
    let missing = quote! {
        || #krate::Error::unprocessable(::std::format!("missing file field `{}`", #name))
    };
    match (kind, multiplicity) {
        (FileKind::Bytes, Multiplicity::One) => quote! {
            let #ident = __form.take_file_bytes(#name).await?.ok_or_else(#missing)?;
        },
        (FileKind::Bytes, Multiplicity::Optional) => quote! {
            let #ident = __form.take_file_bytes(#name).await?;
        },
        (FileKind::Bytes, Multiplicity::Many) => quote! {
            let #ident = __form.take_file_bytes_list(#name).await?;
        },
        (FileKind::Upload, Multiplicity::One) => quote! {
            let #ident = __form.take_upload_file(#name).ok_or_else(#missing)?;
        },
        (FileKind::Upload, Multiplicity::Optional) => quote! {
            let #ident = __form.take_upload_file(#name);
        },
        (FileKind::Upload, Multiplicity::Many) => quote! {
            let #ident = __form.take_upload_file_list(#name);
        },
    }
}

/// Builds the binding for a text (form) parameter from `__form`.
fn text_binding(
    krate: &TokenStream,
    ident: &Ident,
    inner: &Type,
    multiplicity: Multiplicity,
    name: &str,
) -> TokenStream {
    let missing = quote! {
        || #krate::Error::unprocessable(::std::format!("missing form field `{}`", #name))
    };
    match multiplicity {
        Multiplicity::One => quote! {
            let #ident = __form.take_form_value::<#inner>(#name)?.ok_or_else(#missing)?;
        },
        Multiplicity::Optional => quote! {
            let #ident = __form.take_form_value::<#inner>(#name)?;
        },
        Multiplicity::Many => quote! {
            let #ident = __form.take_form_values::<#inner>(#name)?;
        },
    }
}

/// Reads the `name = ".."` override from a `#[file(...)]` / `#[form(...)]` attribute.
fn attr_name(attr: Option<&Attribute>) -> Option<String> {
    let attr = attr?;
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return None;
    }
    let mut name = None;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
        } else {
            // Other keys (max_size, content_types, sniff) are handled at runtime.
            let _ = meta.value().and_then(|value| value.parse::<Expr>());
        }
        Ok(())
    });
    name
}

/// Returns a clone of `func` with the `#[file]` / `#[form]` parameter attributes
/// removed, so the re-emitted function compiles.
fn strip_form_attrs(func: &ItemFn) -> ItemFn {
    let mut func = func.clone();
    for input in &mut func.sig.inputs {
        if let FnArg::Typed(pat_type) = input {
            pat_type
                .attrs
                .retain(|attr| !(attr.path().is_ident("file") || attr.path().is_ident("form")));
        }
    }
    func
}

/// Returns the `T` in a `Result<T>` / `tork::Result<T>` return type.
fn result_ok_type(output: &ReturnType) -> Option<Type> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    first_generic_arg(ty, &["Result"]).cloned()
}

/// Returns the inner `T` of a `Valid<T>` or `Json<T>` parameter type.
fn body_inner_type(ty: &Type) -> Option<&Type> {
    first_generic_arg(ty, &["Valid", "Json"])
}

/// Returns the first generic type argument of a path type whose final segment
/// matches one of `idents` (e.g. `Result`, `Valid`, `Json`).
fn first_generic_arg<'a>(ty: &'a Type, idents: &[&str]) -> Option<&'a Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if !idents.iter().any(|name| segment.ident == name) {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

/// Produces the tokens for the success status code.
///
/// An integer literal is converted via `StatusCode::from_u16`; any other
/// expression is used as-is and must evaluate to a `StatusCode`.
fn status_code_tokens(krate: &TokenStream, status_code: Option<&Expr>) -> TokenStream {
    match status_code {
        None => quote! { #krate::StatusCode::OK },
        Some(Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        })) => {
            quote! { #krate::StatusCode::from_u16(#int).expect("invalid status code") }
        }
        Some(expr) => quote! { #expr },
    }
}
