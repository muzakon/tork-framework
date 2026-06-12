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
    Expr, ExprLit, FnArg, GenericArgument, Ident, ItemFn, Lit, LitInt, LitStr, Pat, Path,
    PathArguments, ReturnType, Token, Type, bracketed, parenthesized,
};

use crate::common::{
    file_binding, file_kind, file_validation, form_property, form_schema_body, krate,
    parse_file_args, parse_size, path_param_names, text_binding, unwrap_multiplicity,
};

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
    /// Route-level multipart upload limits, from `upload(...)`.
    upload: Option<UploadArgs>,
    /// Enclosing router prefix, injected by `#[api_router]` so that path
    /// parameters declared in the prefix are classified correctly. Not part of
    /// the public attribute surface.
    prefix_hint: Option<LitStr>,
}

/// Route-level multipart upload limits parsed from `upload(...)`.
#[derive(Default)]
struct UploadArgs {
    max_body_size: Option<usize>,
    max_file_size: Option<usize>,
    memory_threshold: Option<usize>,
    max_files: Option<usize>,
}

impl Parse for UploadArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = UploadArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "max_body_size" => args.max_body_size = Some(parse_size(&input.parse()?)?),
                "max_file_size" => args.max_file_size = Some(parse_size(&input.parse()?)?),
                "memory_threshold" => args.memory_threshold = Some(parse_size(&input.parse()?)?),
                "max_files" => args.max_files = Some(input.parse::<LitInt>()?.base10_parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown upload option `{other}`"),
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

/// Builds an `UploadConfig` expression from the route's `upload(...)` limits.
fn upload_config_tokens(krate: &TokenStream, args: &UploadArgs) -> TokenStream {
    let mut config = quote! { #krate::UploadConfig::new() };
    if let Some(bytes) = args.max_body_size {
        config = quote! { #config.max_body_size(#bytes) };
    }
    if let Some(bytes) = args.max_file_size {
        config = quote! { #config.max_file_size(#bytes) };
    }
    if let Some(bytes) = args.memory_threshold {
        config = quote! { #config.memory_threshold(#bytes) };
    }
    if let Some(count) = args.max_files {
        config = quote! { #config.max_files(#count) };
    }
    config
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
            upload: None,
            prefix_hint: None,
        };

        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break; // tolerate a trailing comma
            }

            let key: Ident = input.parse()?;

            // `upload(...)` is a parenthesized list rather than `key = value`.
            if key == "upload" {
                let content;
                parenthesized!(content in input);
                args.upload = Some(content.parse()?);
                continue;
            }

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
    // The user-named function becomes the route factory; the original async body is
    // re-emitted under a hidden name and called by the generated handler closure.
    let handler_ident = format_ident!("__tork_handler_{}", fn_name);
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
    // `request_meta` is the builder segment describing the request body for
    // OpenAPI; `extra_items` holds any generated top-level item (the form schema
    // function for a parameter-based multipart handler).
    let multipart = has_form_params(&func);
    let (bindings, call_args, prelude, request_meta, extra_items) = if multipart {
        let route_upload = args.upload.as_ref().map(|u| upload_config_tokens(&krate, u));
        let MultipartParts {
            bindings,
            call_args,
            prelude,
            schema_inserts,
            schema_required,
        } = build_multipart_parts(&krate, &func, &full_path, &route_upload)?;
        // Generate a free function returning the form object schema, recorded as
        // the multipart request body.
        let schema_fn = format_ident!("__tork_form_schema_{}", fn_name);
        let schema_body = form_schema_body(&krate, &schema_inserts, &schema_required);
        let extra_items = quote! {
            #[doc(hidden)]
            fn #schema_fn(generator: &mut #krate::__schemars::SchemaGenerator) -> #krate::__schemars::Schema {
                #schema_body
            }
        };
        let request_meta = quote! {
            .request_schema_fn(#schema_fn)
            .request_kind(#krate::RequestBodyKind::Multipart)
        };
        (bindings, call_args, prelude, request_meta, extra_items)
    } else {
        let HandlerParts {
            bindings,
            call_args,
            request_body,
        } = build_handler_parts(&krate, &func, &full_path)?;
        let request_meta = match &request_body {
            Some(BodyForm::Json(ty)) => quote! { .request_schema::<#ty>() },
            Some(BodyForm::Urlencoded(ty)) => quote! {
                .request_schema::<#ty>().request_kind(#krate::RequestBodyKind::Form)
            },
            Some(BodyForm::Multipart(ty)) => quote! {
                .request_schema_fn(<#ty as #krate::FromMultipart>::form_schema)
                    .request_kind(#krate::RequestBodyKind::Multipart)
            },
            None => TokenStream::new(),
        };
        (bindings, call_args, TokenStream::new(), request_meta, TokenStream::new())
    };

    // Re-emit the function (renamed, without the form parameter attributes) so it
    // compiles and can be called by the generated handler closure.
    let mut emit_func = strip_form_attrs(&func);
    emit_func.sig.ident = handler_ident.clone();

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
    builder = quote! { #builder #request_meta };
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
    let call = quote! { #handler_ident(#(#call_args),*).await };
    let finish = match &args.response_model {
        Some(response_model) => {
            quote! { #krate::__finish_into::<_, #response_model, _>(#call, #status_code) }
        }
        None => quote! { #krate::__finish(#call, #status_code) },
    };

    Ok(quote! {
        #emit_func

        #extra_items

        // The route factory, named after the handler so it can be passed to
        // `App::include` / `Router::include`.
        #vis fn #fn_name() -> #krate::Route {
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

        // Backward-compatible alias used by `Router::route(...)` call sites and the
        // `#[api_router]` auto-registration.
        #[doc(hidden)]
        #vis fn #route_fn() -> #krate::Route {
            #fn_name()
        }
    })
}

/// The request-body encoding declared by a handler parameter, with the inner
/// model type. Drives the OpenAPI request-body content type.
pub(crate) enum BodyForm {
    /// `Valid<T>` / `Json<T>` -> `application/json`.
    Json(Type),
    /// `Form<T>` -> `application/x-www-form-urlencoded`.
    Urlencoded(Type),
    /// `Multipart<T>` -> `multipart/form-data`.
    Multipart(Type),
}

/// The pieces of a generated handler closure: per-parameter bindings, the call
/// argument list, and the detected request body (if any).
pub(crate) struct HandlerParts {
    pub bindings: Vec<TokenStream>,
    pub call_args: Vec<Ident>,
    pub request_body: Option<BodyForm>,
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
    let mut request_body: Option<BodyForm> = None;

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
            if let Some(body) = detect_body_form(ty) {
                if request_body.is_some() {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "a handler may declare at most one request body",
                    ));
                }
                request_body = Some(body);
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
    /// Property inserts describing the form object for the OpenAPI schema.
    pub schema_inserts: Vec<TokenStream>,
    /// Names of the required (non-`Option`, non-`Vec`) form fields.
    pub schema_required: Vec<String>,
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

/// Builds the multipart body parse prelude with the route's upload override.
fn multipart_prelude(krate: &TokenStream, route_upload: &Option<TokenStream>) -> TokenStream {
    let config = route_upload
        .clone()
        .unwrap_or_else(|| quote! { #krate::UploadConfig::new() });
    quote! {
        let mut __form = #krate::__parse_multipart(&ctx, #config).await?;
    }
}

/// Builds the bindings for a multipart handler: one parse, then a take per field.
fn build_multipart_parts(
    krate: &TokenStream,
    func: &ItemFn,
    full_path: &str,
    route_upload: &Option<TokenStream>,
) -> syn::Result<MultipartParts> {
    let placeholders = path_param_names(full_path);
    let mut bindings = Vec::new();
    let mut call_args = Vec::new();
    let mut schema_inserts = Vec::new();
    let mut schema_required = Vec::new();

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
            let args = match file_attr {
                Some(attr) => parse_file_args(attr)?,
                None => Default::default(),
            };
            let field = args.name.clone().unwrap_or_else(|| name.clone());
            bindings.push(file_binding(krate, &ident, kind, multiplicity, &field));
            let validation = file_validation(krate, &ident, kind, multiplicity, &args);
            if !validation.is_empty() {
                bindings.push(validation);
            }
            let (insert, required) = form_property(krate, &field, true, inner, multiplicity);
            schema_inserts.push(insert);
            if required {
                schema_required.push(field);
            }
        } else if let Some(attr) = form_attr {
            let args = parse_file_args(attr)?;
            let field = args.name.clone().unwrap_or_else(|| name.clone());
            bindings.push(text_binding(krate, &ident, inner, multiplicity, &field));
            let (insert, required) = form_property(krate, &field, false, inner, multiplicity);
            schema_inserts.push(insert);
            if required {
                schema_required.push(field);
            }
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

    let prelude = multipart_prelude(krate, route_upload);
    Ok(MultipartParts {
        bindings,
        call_args,
        prelude,
        schema_inserts,
        schema_required,
    })
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

/// Classifies a parameter type as a request body and its encoding, if any.
fn detect_body_form(ty: &Type) -> Option<BodyForm> {
    if let Some(inner) = first_generic_arg(ty, &["Valid", "Json"]) {
        return Some(BodyForm::Json(inner.clone()));
    }
    if let Some(inner) = first_generic_arg(ty, &["Form"]) {
        return Some(BodyForm::Urlencoded(inner.clone()));
    }
    first_generic_arg(ty, &["Multipart"]).map(|inner| BodyForm::Multipart(inner.clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;
    use syn::{parse_quote, parse_str};

    #[test]
    fn upload_args_and_route_args_parse_expected_fields() {
        let upload: UploadArgs = parse_str(
            "max_body_size = \"2mb\", max_file_size = \"1mb\", memory_threshold = \"16kb\", max_files = 3",
        )
        .unwrap();
        assert_eq!(upload.max_body_size, Some(2 * 1024 * 1024));
        assert_eq!(upload.max_file_size, Some(1024 * 1024));
        assert_eq!(upload.memory_threshold, Some(16 * 1024));
        assert_eq!(upload.max_files, Some(3));
        let error = match parse_str::<UploadArgs>("nope = 1") {
            Ok(_) => panic!("expected parse failure"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown upload option"));

        let args: RouteArgs = parse_str(
            "\"/users/{id}\", response_model = UserOut, summary = \"sum\", description = \"desc\", status_code = 201, tags = [\"users\"], on_request = audit, on_response = done, on_error = err, on_validation_error = invalid, __prefix = \"/api\", upload(max_files = 2)",
        )
        .unwrap();
        assert_eq!(args.path.value(), "/users/{id}");
        assert!(args.response_model.is_some());
        assert_eq!(args.tags.len(), 1);
        assert_eq!(args.on_request.len(), 1);
        assert!(args.upload.is_some());

        let error = match parse_str::<RouteArgs>("\"/x\", mystery = 1") {
            Ok(_) => panic!("expected parse failure"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown route attribute"));
    }

    #[test]
    fn helper_functions_detect_body_forms_and_metadata() {
        assert!(matches!(
            detect_body_form(&parse_quote!(tork::Valid<Input>)),
            Some(BodyForm::Json(_))
        ));
        assert!(matches!(
            detect_body_form(&parse_quote!(tork::Form<Input>)),
            Some(BodyForm::Urlencoded(_))
        ));
        assert!(matches!(
            detect_body_form(&parse_quote!(tork::Multipart<Input>)),
            Some(BodyForm::Multipart(_))
        ));
        assert!(detect_body_form(&parse_quote!(String)).is_none());
        assert!(body_inner_type(&parse_quote!(tork::Json<Input>)).is_some());
        assert!(first_generic_arg(&parse_quote!(Result<Item>), &["Result"]).is_some());
        assert!(first_generic_arg(&parse_quote!(String), &["Result"]).is_none());

        let no_status = status_code_tokens(&krate(), None).to_string();
        assert!(no_status.contains("StatusCode :: OK"));
        let numeric = status_code_tokens(&krate(), Some(&parse_quote!(201))).to_string();
        assert!(numeric.contains("from_u16"));
        assert_eq!(
            status_code_tokens(&krate(), Some(&parse_quote!(tork::StatusCode::CREATED))).to_string(),
            "tork :: StatusCode :: CREATED"
        );
    }

    #[test]
    fn build_handler_and_multipart_parts_cover_error_paths() {
        let krate = krate();
        let func: ItemFn = parse_quote! {
            async fn handler(id: String, body: tork::Valid<Input>) -> tork::Result<Output> { todo!() }
        };
        let parts = build_handler_parts(&krate, &func, "/users/{id}").unwrap();
        assert_eq!(parts.call_args.len(), 2);
        assert!(matches!(parts.request_body, Some(BodyForm::Json(_))));
        assert!(parts.bindings[0].to_string().contains("__extract_path_param"));

        let func: ItemFn = parse_quote! {
            async fn bad(self) -> tork::Result<Output> { todo!() }
        };
        let error = match build_handler_parts(&krate, &func, "/") {
            Ok(_) => panic!("expected self rejection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cannot take `self`"));

        let func: ItemFn = parse_quote! {
            async fn bad((id): String) -> tork::Result<Output> { todo!() }
        };
        let error = match build_handler_parts(&krate, &func, "/users/{id}") {
            Ok(_) => panic!("expected identifier rejection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("simple identifiers"));

        let func: ItemFn = parse_quote! {
            async fn bad(a: tork::Valid<A>, b: tork::Json<B>) -> tork::Result<Output> { todo!() }
        };
        let error = match build_handler_parts(&krate, &func, "/") {
            Ok(_) => panic!("expected duplicate body rejection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("at most one request body"));

        let func: ItemFn = parse_quote! {
            async fn upload(#[file] file: tork::FileBytes, body: tork::Json<Input>) -> tork::Result<Output> { todo!() }
        };
        let error = match build_multipart_parts(&krate, &func, "/", &None) {
            Ok(_) => panic!("expected multipart conflict"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("only have one body encoding"));

        let func: ItemFn = parse_quote! {
            async fn upload(#[file] payload: String) -> tork::Result<Output> { todo!() }
        };
        let error = match build_multipart_parts(&krate, &func, "/", &None) {
            Ok(_) => panic!("expected invalid file rejection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("FileBytes or UploadFile"));
    }

    #[test]
    fn route_helper_builders_emit_expected_tokens() {
        let krate = krate();
        let config = upload_config_tokens(
            &krate,
            &UploadArgs {
                max_body_size: Some(1),
                max_file_size: Some(2),
                memory_threshold: Some(3),
                max_files: Some(4),
            },
        )
        .to_string();
        assert!(config.contains("max_body_size"));
        assert!(config.contains("max_file_size"));
        assert!(config.contains("memory_threshold"));
        assert!(config.contains("max_files"));

        let func: ItemFn = parse_quote! {
            async fn upload(#[file] file: tork::FileBytes, #[form(name = "token")] token: String, id: String) -> tork::Result<Output> { todo!() }
        };
        assert!(has_form_params(&func));
        let route_upload = Some(quote!(tork::UploadConfig::new().max_files(2)));
        let parts = build_multipart_parts(&krate, &func, "/users/{id}", &route_upload).unwrap();
        assert_eq!(parts.call_args.len(), 3);
        assert!(parts.prelude.to_string().contains("__parse_multipart"));
        assert_eq!(parts.schema_required, vec!["file".to_owned(), "token".to_owned()]);

        let stripped = strip_form_attrs(&func);
        let rendered = stripped.to_token_stream().to_string();
        assert!(!rendered.contains("file]"));
        assert!(!rendered.contains("form]"));

        let output: ReturnType = parse_quote!(-> tork::Result<UserOut>);
        assert!(result_ok_type(&output).is_some());
    }

    #[test]
    fn helper_functions_cover_default_multipart_and_non_matching_types() {
        let krate = krate();
        let func: ItemFn = parse_quote! {
            async fn plain(id: String, dep: MyDep) -> tork::Result<Output> { todo!() }
        };

        assert!(!has_form_params(&func));
        let parts = build_handler_parts(&krate, &func, "/users/{id}").unwrap();
        assert_eq!(parts.call_args.len(), 2);
        assert!(parts.request_body.is_none());
        assert!(parts.bindings[0].to_string().contains("__extract_path_param"));
        assert!(parts.bindings[1].to_string().contains("FromRequest"));

        let default_prelude = multipart_prelude(&krate, &None).to_string();
        assert!(default_prelude.contains("UploadConfig :: new"));

        let override_prelude = multipart_prelude(
            &krate,
            &Some(quote!(tork::UploadConfig::new().max_files(1))),
        )
        .to_string();
        assert!(override_prelude.contains("max_files"));

        assert!(body_inner_type(&parse_quote!(tork::Form<Input>)).is_none());
        assert!(first_generic_arg(&parse_quote!(std::result::Result<Item, E>), &["Result"]).is_some());
        assert!(first_generic_arg(&parse_quote!(std::vec::Vec<Item>), &["Result"]).is_none());
    }

    #[test]
    fn route_impl_emits_metadata_and_response_model_chains() {
        let args: RouteArgs = parse_quote!(
            "/items/{id}",
            summary = "Fetch item",
            description = "Returns an item",
            status_code = 201,
            tags = ["items", "public"],
            response_model = Output,
            on_request = audit::request,
            on_response = audit::response,
            on_error = audit::error,
            on_validation_error = audit::validation
        );
        let func: ItemFn = parse_quote!(
            pub async fn get_item(id: String) -> tork::Result<Output> {
                todo!()
            }
        );

        let tokens = expand_route("GET", args, func)
            .unwrap()
            .to_string();

        assert!(tokens.contains("__tork_route_get_item"));
        assert!(tokens.contains("Fetch item"));
        assert!(tokens.contains("Returns an item"));
        assert!(tokens.contains("items"));
        assert!(tokens.contains("public"));
        assert!(tokens.contains("response_model"));
        assert!(tokens.contains("response_schema"));
        assert!(tokens.contains("on_request"));
        assert!(tokens.contains("on_response"));
        assert!(tokens.contains("on_error"));
        assert!(tokens.contains("on_validation_error"));
        assert!(tokens.contains("StatusCode :: from_u16"));
    }

    #[test]
    fn route_impl_emits_multipart_and_request_body_metadata() {
        let args: RouteArgs = parse_quote!(
            "/upload/{id}",
            upload(max_files = 2, max_body_size = "2MB")
        );
        let func: ItemFn = parse_quote!(
            async fn upload(
                id: String,
                #[file(name = "avatar", max_size = "64KB", sniff = true)] file: tork::UploadFile,
                #[form(name = "title")] title: String
            ) -> tork::Result<Output> {
                todo!()
            }
        );

        let tokens = expand_route("POST", args, func)
            .unwrap()
            .to_string();

        assert!(tokens.contains("__tork_form_schema_upload"));
        assert!(tokens.contains("request_schema_fn"));
        assert!(tokens.contains("request_kind"));
        assert!(tokens.contains("__parse_multipart"));
        assert!(tokens.contains("take_upload_file"));
        assert!(tokens.contains("take_form_value"));
    }
}
