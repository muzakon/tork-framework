//! The `#[api_model]` attribute macro.
//!
//! Turns a struct into a request/response model by deriving serde
//! (de)serialization, `garde` validation, and `schemars` JSON Schema, and by
//! translating concise `#[field(...)]` constraints into the corresponding
//! `garde` and `schemars` attributes. Generated code refers to the underlying
//! crates through the `tork` facade (`::tork::__serde` / `__garde` / `__schemars`),
//! so a user crate only needs to depend on `tork`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    Attribute, Expr, Fields, GenericArgument, Ident, ItemStruct, LitInt, LitStr, PathArguments,
    Token, Type, parse_macro_input,
};

/// Container-level options parsed from `#[api_model(...)]`.
#[derive(Default)]
struct ContainerArgs {
    rename_all: Option<LitStr>,
}

impl Parse for ContainerArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ContainerArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "rename_all" => args.rename_all = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown api_model option `{other}`"),
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

/// Constraints parsed from a field's `#[field(...)]` attribute.
#[derive(Default)]
struct FieldArgs {
    min_length: Option<LitInt>,
    max_length: Option<LitInt>,
    ge: Option<Expr>,
    le: Option<Expr>,
    gt: Option<Expr>,
    lt: Option<Expr>,
    title: Option<LitStr>,
    description: Option<LitStr>,
}

impl Parse for FieldArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = FieldArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "min_length" => args.min_length = Some(input.parse()?),
                "max_length" => args.max_length = Some(input.parse()?),
                "ge" => args.ge = Some(input.parse()?),
                "le" => args.le = Some(input.parse()?),
                "gt" => args.gt = Some(input.parse()?),
                "lt" => args.lt = Some(input.parse()?),
                "title" => args.title = Some(input.parse()?),
                "description" => args.description = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown field constraint `{other}`"),
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

/// Expands `#[api_model]` over a named struct.
pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let container = parse_macro_input!(attr as ContainerArgs);
    let item = parse_macro_input!(item as ItemStruct);
    match expand_struct(container, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_struct(container: ContainerArgs, item: ItemStruct) -> syn::Result<TokenStream2> {
    let fields = match &item.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &item,
                "#[api_model] supports only structs with named fields",
            ));
        }
    };

    let struct_ident = &item.ident;
    let vis = &item.vis;
    let generics = &item.generics;
    let struct_attrs = &item.attrs;

    let mut field_tokens = Vec::new();
    let mut extra_fns = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;

        // Split `#[field(...)]` from the field's other attributes.
        let mut field_args = FieldArgs::default();
        let mut preserved: Vec<&Attribute> = Vec::new();
        for attr in &field.attrs {
            if attr.path().is_ident("field") {
                field_args = attr.parse_args()?;
            } else {
                preserved.push(attr);
            }
        }

        let mut garde_rules: Vec<TokenStream2> = Vec::new();
        let mut schemars_rules: Vec<TokenStream2> = Vec::new();

        // length(min, max) — applies to String length (and inner of Option).
        if field_args.min_length.is_some() || field_args.max_length.is_some() {
            let parts = bound_parts(
                field_args.min_length.as_ref().map(|l| quote!(#l)),
                field_args.max_length.as_ref().map(|l| quote!(#l)),
            );
            garde_rules.push(quote!(length(#parts)));
            schemars_rules.push(quote!(length(#parts)));
        }

        // range(min, max) — inclusive numeric bounds from ge / le.
        if field_args.ge.is_some() || field_args.le.is_some() {
            let parts = bound_parts(
                field_args.ge.as_ref().map(|e| coerce_bound(e, field_ty)),
                field_args.le.as_ref().map(|e| coerce_bound(e, field_ty)),
            );
            garde_rules.push(quote!(range(#parts)));
            schemars_rules.push(quote!(range(#parts)));
        }

        // Exclusive bounds (gt / lt): garde has no native exclusive range, so a
        // custom validator enforces it; schemars carries the exact keyword.
        if let Some(bound) = &field_args.gt {
            let (check_fn, call) =
                exclusive_check(struct_ident, field_ident, "gt", bound, field_ty);
            extra_fns.push(check_fn);
            garde_rules.push(quote!(custom(#call)));
            schemars_rules.push(quote!(extend("exclusiveMinimum" = #bound)));
        }
        if let Some(bound) = &field_args.lt {
            let (check_fn, call) =
                exclusive_check(struct_ident, field_ident, "lt", bound, field_ty);
            extra_fns.push(check_fn);
            garde_rules.push(quote!(custom(#call)));
            schemars_rules.push(quote!(extend("exclusiveMaximum" = #bound)));
        }

        if let Some(title) = &field_args.title {
            schemars_rules.push(quote!(title = #title));
        }
        if let Some(description) = &field_args.description {
            schemars_rules.push(quote!(description = #description));
        }

        // garde requires every field to be annotated; mark unconstrained fields
        // as skipped.
        let garde_attr = if garde_rules.is_empty() {
            quote!(#[garde(skip)])
        } else {
            quote!(#[garde(#(#garde_rules),*)])
        };
        let schemars_attr = if schemars_rules.is_empty() {
            quote!()
        } else {
            quote!(#[schemars(#(#schemars_rules),*)])
        };

        let field_vis = &field.vis;
        field_tokens.push(quote! {
            #(#preserved)*
            #garde_attr
            #schemars_attr
            #field_vis #field_ident: #field_ty,
        });
    }

    let rename_attr = container
        .rename_all
        .map(|rename| quote!(#[serde(rename_all = #rename)]));

    Ok(quote! {
        #(#struct_attrs)*
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::tork::__serde::Serialize,
            ::tork::__serde::Deserialize,
            ::tork::__garde::Validate,
            ::tork::__schemars::JsonSchema,
        )]
        #[serde(crate = "::tork::__serde")]
        #rename_attr
        #[schemars(crate = "::tork::__schemars")]
        #vis struct #struct_ident #generics {
            #(#field_tokens)*
        }

        #(#extra_fns)*
    })
}

/// Builds the `min`/`max` argument list for `length(...)` or `range(...)`.
fn bound_parts(min: Option<TokenStream2>, max: Option<TokenStream2>) -> TokenStream2 {
    match (min, max) {
        (Some(min), Some(max)) => quote!(min = #min, max = #max),
        (Some(min), None) => quote!(min = #min),
        (None, Some(max)) => quote!(max = #max),
        (None, None) => quote!(),
    }
}

/// Generates a hidden garde custom validator enforcing an exclusive bound, plus
/// the path to call it. Handles `Option<T>` by validating only the `Some` case.
fn exclusive_check(
    struct_ident: &Ident,
    field_ident: &Ident,
    kind: &str,
    bound: &Expr,
    field_ty: &Type,
) -> (TokenStream2, Ident) {
    let fn_ident = format_ident!(
        "__tork_{}_{}_{}",
        to_snake(&struct_ident.to_string()),
        field_ident,
        kind
    );
    let (op, word): (TokenStream2, &str) = if kind == "gt" {
        (quote!(>), "greater than")
    } else {
        (quote!(<), "less than")
    };
    let message = format!("must be {word} {}", quote!(#bound));
    let compare_ty = option_inner(field_ty).unwrap_or(field_ty);

    let body = if option_inner(field_ty).is_some() {
        quote! {
            match value {
                ::core::option::Option::Some(value) => {
                    if *value #op (#bound as #compare_ty) {
                        ::core::result::Result::Ok(())
                    } else {
                        ::core::result::Result::Err(::tork::__garde::Error::new(#message))
                    }
                }
                ::core::option::Option::None => ::core::result::Result::Ok(()),
            }
        }
    } else {
        quote! {
            if *value #op (#bound as #compare_ty) {
                ::core::result::Result::Ok(())
            } else {
                ::core::result::Result::Err(::tork::__garde::Error::new(#message))
            }
        }
    };

    let check_fn = quote! {
        #[doc(hidden)]
        fn #fn_ident(
            value: &#field_ty,
            _ctx: &(),
        ) -> ::core::result::Result<(), ::tork::__garde::Error> {
            #body
        }
    };

    (check_fn, fn_ident)
}

/// Coerces an integer literal bound to a float literal when the field type is
/// `f32`/`f64`, so `ge = 0` works on a float field. Other expressions pass
/// through unchanged.
fn coerce_bound(expr: &Expr, field_ty: &Type) -> TokenStream2 {
    let inner = option_inner(field_ty).unwrap_or(field_ty);
    if is_float_type(inner) {
        if let Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        }) = expr
        {
            if let Ok(float) = format!("{}.0", int.base10_digits()).parse::<TokenStream2>() {
                return float;
            }
        }
    }
    quote!(#expr)
}

/// Returns `true` if the type is `f32` or `f64`.
fn is_float_type(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.is_ident("f32") || path.path.is_ident("f64"))
}

/// Returns the inner type of `Option<T>`, or `None` for other types.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Option" {
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

/// Converts a `PascalCase` identifier to `snake_case` for generated fn names.
fn to_snake(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (index, ch) in input.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
