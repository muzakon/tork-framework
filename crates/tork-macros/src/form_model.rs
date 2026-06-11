//! The `#[derive(FormModel)]` macro.
//!
//! Generates a `FromMultipart` implementation that binds each field from a parsed
//! multipart body: file fields (`FileBytes` / `UploadFile`, optionally `Option` or
//! `Vec`) via the file takers, and text fields via the typed value takers, then
//! runs the `#[field(...)]` constraints. File fields are recognized by a `#[file]`
//! attribute or by their type.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, GenericArgument, Ident, LitInt, LitStr,
    PathArguments, Type, parse_macro_input,
};

use crate::common::krate;

/// Expands `#[derive(FormModel)]`.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "#[derive(FormModel)] requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(FormModel)] can only be derived for structs",
            ));
        }
    };

    let krate = krate();
    let ident = &input.ident;

    let mut bindings = Vec::new();
    let mut checks = Vec::new();
    let mut names = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let (multiplicity, inner) = multiplicity(&field.ty);

        let file_attr = field.attrs.iter().find(|attr| attr.path().is_ident("file"));
        let is_file = file_attr.is_some() || is_file_type(inner).is_some();

        // The multipart field name defaults to the Rust field name.
        let name = field_name(field, file_attr)?.unwrap_or_else(|| field_ident.to_string());

        if is_file {
            let kind = is_file_type(inner).ok_or_else(|| {
                syn::Error::new_spanned(
                    &field.ty,
                    "a #[file] field must be a FileBytes or UploadFile (optionally Option/Vec)",
                )
            })?;
            bindings.push(file_binding(&krate, field_ident, kind, multiplicity, &name));
        } else {
            bindings.push(text_binding(&krate, field_ident, inner, multiplicity, &name));
            if let Some(constraints) = field_constraints(field)? {
                checks.push(text_checks(&krate, field_ident, multiplicity, &name, &constraints));
            }
        }

        names.push(field_ident);
    }

    Ok(quote! {
        impl #krate::FromMultipart for #ident {
            async fn from_multipart(form: &mut #krate::MultipartForm) -> #krate::Result<Self> {
                #(#bindings)*
                let mut __errors: ::std::vec::Vec<#krate::ErrorDetail> = ::std::vec::Vec::new();
                #(#checks)*
                if !__errors.is_empty() {
                    return ::core::result::Result::Err(
                        #krate::Error::unprocessable("the submitted form failed validation")
                            .with_code("VALIDATION_ERROR")
                            .with_details(__errors),
                    );
                }
                ::core::result::Result::Ok(#ident { #(#names),* })
            }
        }
    })
}

/// How many values a field binds.
#[derive(Clone, Copy)]
enum Multiplicity {
    One,
    Optional,
    Many,
}

/// Which file type a field binds.
#[derive(Clone, Copy)]
enum FileKind {
    Bytes,
    Upload,
}

/// Splits a field type into its multiplicity and inner type.
fn multiplicity(ty: &Type) -> (Multiplicity, &Type) {
    if let Some(inner) = generic_arg(ty, "Option") {
        (Multiplicity::Optional, inner)
    } else if let Some(inner) = generic_arg(ty, "Vec") {
        (Multiplicity::Many, inner)
    } else {
        (Multiplicity::One, ty)
    }
}

/// Returns the file kind if `ty` is `FileBytes` or `UploadFile`.
fn is_file_type(ty: &Type) -> Option<FileKind> {
    match last_segment(ty)?.to_string().as_str() {
        "FileBytes" => Some(FileKind::Bytes),
        "UploadFile" => Some(FileKind::Upload),
        _ => None,
    }
}

/// Builds the binding for a file field.
fn file_binding(
    krate: &TokenStream2,
    ident: &Ident,
    kind: FileKind,
    multiplicity: Multiplicity,
    name: &str,
) -> TokenStream2 {
    let missing = quote! {
        || #krate::Error::unprocessable(::std::format!("missing file field `{}`", #name))
    };
    match (kind, multiplicity) {
        (FileKind::Bytes, Multiplicity::One) => quote! {
            let #ident = form.take_file_bytes(#name).await?.ok_or_else(#missing)?;
        },
        (FileKind::Bytes, Multiplicity::Optional) => quote! {
            let #ident = form.take_file_bytes(#name).await?;
        },
        (FileKind::Bytes, Multiplicity::Many) => quote! {
            let #ident = form.take_file_bytes_list(#name).await?;
        },
        (FileKind::Upload, Multiplicity::One) => quote! {
            let #ident = form.take_upload_file(#name).ok_or_else(#missing)?;
        },
        (FileKind::Upload, Multiplicity::Optional) => quote! {
            let #ident = form.take_upload_file(#name);
        },
        (FileKind::Upload, Multiplicity::Many) => quote! {
            let #ident = form.take_upload_file_list(#name);
        },
    }
}

/// Builds the binding for a text field.
fn text_binding(
    krate: &TokenStream2,
    ident: &Ident,
    inner: &Type,
    multiplicity: Multiplicity,
    name: &str,
) -> TokenStream2 {
    let missing = quote! {
        || #krate::Error::unprocessable(::std::format!("missing form field `{}`", #name))
    };
    match multiplicity {
        Multiplicity::One => quote! {
            let #ident = form.take_form_value::<#inner>(#name)?.ok_or_else(#missing)?;
        },
        Multiplicity::Optional => quote! {
            let #ident = form.take_form_value::<#inner>(#name)?;
        },
        Multiplicity::Many => quote! {
            let #ident = form.take_form_values::<#inner>(#name)?;
        },
    }
}

/// Numeric/length constraints parsed from a `#[field(...)]` attribute.
#[derive(Default)]
struct Constraints {
    min_length: Option<LitInt>,
    max_length: Option<LitInt>,
    ge: Option<Expr>,
    le: Option<Expr>,
    gt: Option<Expr>,
    lt: Option<Expr>,
}

/// Generates the validation checks for a single text field.
fn text_checks(
    krate: &TokenStream2,
    ident: &Ident,
    multiplicity: Multiplicity,
    name: &str,
    constraints: &Constraints,
) -> TokenStream2 {
    let mut body = Vec::new();
    if let Some(min) = &constraints.min_length {
        body.push(quote! {
            if __value.chars().count() < #min {
                __errors.push(#krate::ErrorDetail::new(
                    #name, "TOO_SHORT",
                    ::std::format!("must be at least {} characters", #min)));
            }
        });
    }
    if let Some(max) = &constraints.max_length {
        body.push(quote! {
            if __value.chars().count() > #max {
                __errors.push(#krate::ErrorDetail::new(
                    #name, "TOO_LONG",
                    ::std::format!("must be at most {} characters", #max)));
            }
        });
    }
    for (op, expr, issue, message) in numeric_checks(constraints) {
        body.push(quote! {
            if !(*__value #op #expr) {
                __errors.push(#krate::ErrorDetail::new(#name, #issue, #message.to_owned()));
            }
        });
    }

    let body = quote! { #(#body)* };
    match multiplicity {
        Multiplicity::One => quote! { { let __value = &#ident; #body } },
        Multiplicity::Optional => quote! { if let ::core::option::Option::Some(__value) = &#ident { #body } },
        // Repeated fields are not constraint-checked element by element in v1.
        Multiplicity::Many => quote! {},
    }
}

/// Returns the numeric comparison checks (operator, bound, issue, message).
fn numeric_checks(
    constraints: &Constraints,
) -> Vec<(TokenStream2, &Expr, &'static str, &'static str)> {
    let mut out = Vec::new();
    if let Some(ge) = &constraints.ge {
        out.push((quote! { >= }, ge, "TOO_SMALL", "is below the minimum"));
    }
    if let Some(le) = &constraints.le {
        out.push((quote! { <= }, le, "TOO_LARGE", "is above the maximum"));
    }
    if let Some(gt) = &constraints.gt {
        out.push((quote! { > }, gt, "TOO_SMALL", "must be greater"));
    }
    if let Some(lt) = &constraints.lt {
        out.push((quote! { < }, lt, "TOO_LARGE", "must be smaller"));
    }
    out
}

/// Parses `#[field(...)]` constraints, if present.
fn field_constraints(field: &syn::Field) -> syn::Result<Option<Constraints>> {
    let Some(attr) = field.attrs.iter().find(|attr| attr.path().is_ident("field")) else {
        return Ok(None);
    };
    let mut constraints = Constraints::default();
    attr.parse_nested_meta(|meta| {
        let key = meta
            .path
            .get_ident()
            .map(|ident| ident.to_string())
            .unwrap_or_default();
        match key.as_str() {
            "min_length" => constraints.min_length = Some(meta.value()?.parse()?),
            "max_length" => constraints.max_length = Some(meta.value()?.parse()?),
            "ge" => constraints.ge = Some(meta.value()?.parse()?),
            "le" => constraints.le = Some(meta.value()?.parse()?),
            "gt" => constraints.gt = Some(meta.value()?.parse()?),
            "lt" => constraints.lt = Some(meta.value()?.parse()?),
            // Unknown keys (title/description) are ignored for forms.
            _ => {
                let _ = meta.value().and_then(|value| value.parse::<Expr>());
            }
        }
        Ok(())
    })?;
    Ok(Some(constraints))
}

/// Reads the `name = ".."` override from a `#[file(...)]` or `#[form(...)]` attribute.
fn field_name(field: &syn::Field, file_attr: Option<&Attribute>) -> syn::Result<Option<String>> {
    let attr = file_attr.or_else(|| field.attrs.iter().find(|attr| attr.path().is_ident("form")));
    let Some(attr) = attr else { return Ok(None) };
    // A bare `#[file]` / `#[form]` has no arguments.
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok(None);
    }
    let mut name = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
        } else {
            // Other keys (max_size, content_types, sniff) are handled elsewhere.
            let _ = meta.value().and_then(|value| value.parse::<Expr>());
        }
        Ok(())
    })?;
    Ok(name)
}

/// Returns the inner type of `Wrapper<T>` when the outer segment matches `wrapper`.
fn generic_arg<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != wrapper {
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

/// Returns the final path segment identifier of a type.
fn last_segment(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Path(path) => path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}
