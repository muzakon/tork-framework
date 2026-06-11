//! The `#[settings]` attribute macro.
//!
//! Turns a struct into a typed configuration: it derives `serde` deserialization
//! and `garde` validation, translates `#[setting(...)]` constraints into `garde`
//! attributes (reusing the same mapping as `#[api_model]`), turns
//! `#[setting(default = ...)]` into a serde default, and generates a `load()`
//! method built on `tork::SettingsLoader`. Generated code refers to the underlying
//! crates through the `tork` facade, so a user crate only needs `tork` (and
//! `garde`, whose derive hardcodes its own path).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    Attribute, Expr, ExprLit, Fields, Ident, ItemStruct, Lit, LitInt, LitStr, Token, bracketed,
    parse_macro_input,
};

use crate::api_model::{bound_parts, coerce_bound, exclusive_check, to_snake};

/// Container-level options parsed from `#[settings(...)]`.
#[derive(Default)]
struct ContainerArgs {
    prefix: Option<LitStr>,
    env_file: Option<LitStr>,
    config_file: Option<LitStr>,
    files: Vec<LitStr>,
    secrets_dir: Option<LitStr>,
}

impl Parse for ContainerArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ContainerArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "prefix" => args.prefix = Some(input.parse()?),
                "env_file" => args.env_file = Some(input.parse()?),
                "config_file" => args.config_file = Some(input.parse()?),
                "secrets_dir" => args.secrets_dir = Some(input.parse()?),
                "files" => {
                    let content;
                    bracketed!(content in input);
                    let paths = content.parse_terminated(<LitStr as Parse>::parse, Token![,])?;
                    args.files = paths.into_iter().collect();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown settings option `{other}`"),
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

/// Constraints parsed from a field's `#[setting(...)]` attribute.
#[derive(Default)]
struct SettingArgs {
    default: Option<Expr>,
    min_length: Option<LitInt>,
    max_length: Option<LitInt>,
    ge: Option<Expr>,
    le: Option<Expr>,
    gt: Option<Expr>,
    lt: Option<Expr>,
    email: bool,
    secret: bool,
    nested: bool,
    /// A bare `default` flag: fill an absent value from the type's `Default`.
    default_flag: bool,
    custom: Vec<Expr>,
}

impl Parse for SettingArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = SettingArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();
            match name.as_str() {
                // Bare flags (no `= value`).
                "email" => args.email = true,
                "secret" => args.secret = true,
                "nested" => args.nested = true,
                // `default` is either a bare flag (use the type's `Default`) or a
                // value (`default = expr`).
                "default" => {
                    if input.peek(Token![=]) {
                        input.parse::<Token![=]>()?;
                        args.default = Some(input.parse()?);
                    } else {
                        args.default_flag = true;
                    }
                }
                _ => {
                    input.parse::<Token![=]>()?;
                    match name.as_str() {
                        "min_length" => args.min_length = Some(input.parse()?),
                        "max_length" => args.max_length = Some(input.parse()?),
                        "ge" => args.ge = Some(input.parse()?),
                        "le" => args.le = Some(input.parse()?),
                        "gt" => args.gt = Some(input.parse()?),
                        "lt" => args.lt = Some(input.parse()?),
                        "custom" => args.custom.push(input.parse()?),
                        other => {
                            return Err(syn::Error::new(
                                key.span(),
                                format!("unknown setting constraint `{other}`"),
                            ));
                        }
                    }
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

/// Expands `#[settings]` over a named struct.
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
                "#[settings] supports only structs with named fields",
            ));
        }
    };

    let struct_ident = &item.ident;
    let vis = &item.vis;
    let generics = &item.generics;
    let struct_attrs = &item.attrs;

    let mut field_tokens = Vec::new();
    let mut extra_fns = Vec::new();
    // Field initializers for a generated `Default` impl, and whether every field
    // can be defaulted (so the struct can stand in as an absent nested group).
    let mut default_inits = Vec::new();
    let mut all_defaultable = true;

    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;

        // Split `#[setting(...)]` from the field's other attributes.
        let mut args = SettingArgs::default();
        let mut preserved: Vec<&Attribute> = Vec::new();
        for attr in &field.attrs {
            if attr.path().is_ident("setting") {
                args = attr.parse_args()?;
            } else {
                preserved.push(attr);
            }
        }

        let mut garde_rules: Vec<TokenStream2> = Vec::new();

        // A secret is never validated and never carries other constraints.
        if !args.secret {
            if args.min_length.is_some() || args.max_length.is_some() {
                let parts = bound_parts(
                    args.min_length.as_ref().map(|l| quote!(#l)),
                    args.max_length.as_ref().map(|l| quote!(#l)),
                );
                garde_rules.push(quote!(length(#parts)));
            }
            if args.ge.is_some() || args.le.is_some() {
                let parts = bound_parts(
                    args.ge.as_ref().map(|e| coerce_bound(e, field_ty)),
                    args.le.as_ref().map(|e| coerce_bound(e, field_ty)),
                );
                garde_rules.push(quote!(range(#parts)));
            }
            if let Some(bound) = &args.gt {
                let (check_fn, call) =
                    exclusive_check(struct_ident, field_ident, "gt", bound, field_ty);
                extra_fns.push(check_fn);
                garde_rules.push(quote!(custom(#call)));
            }
            if let Some(bound) = &args.lt {
                let (check_fn, call) =
                    exclusive_check(struct_ident, field_ident, "lt", bound, field_ty);
                extra_fns.push(check_fn);
                garde_rules.push(quote!(custom(#call)));
            }
            if args.email {
                garde_rules.push(quote!(email));
            }
            for custom in &args.custom {
                garde_rules.push(quote!(custom(#custom)));
            }
            if args.nested {
                garde_rules.push(quote!(dive));
            }
        }

        // garde requires every field to be annotated; unconstrained and secret
        // fields are skipped.
        let garde_attr = if garde_rules.is_empty() {
            quote!(#[garde(skip)])
        } else {
            quote!(#[garde(#(#garde_rules),*)])
        };

        // Decide how an absent value is filled. A declared default uses a generated
        // function; a nested group, `Option`, or `Vec` falls back to its own
        // `Default`. Anything else stays required, and a missing value errors.
        let serde_attr = if let Some(default) = &args.default {
            let fn_ident = format_ident!(
                "__tork_default_{}_{}",
                to_snake(&struct_ident.to_string()),
                field_ident
            );
            let value = default_value(default);
            extra_fns.push(quote! {
                #[doc(hidden)]
                fn #fn_ident() -> #field_ty { #value }
            });
            default_inits.push(quote!(#field_ident: #fn_ident()));
            let fn_name = fn_ident.to_string();
            quote!(#[serde(default = #fn_name)])
        } else if args.default_flag {
            // Fill an absent value from the type's `Default` (a nested group, an
            // `Option`, a `Vec`, or any `Default` type).
            default_inits.push(quote!(#field_ident: ::core::default::Default::default()));
            quote!(#[serde(default)])
        } else {
            all_defaultable = false;
            quote!()
        };

        let field_vis = &field.vis;
        field_tokens.push(quote! {
            #(#preserved)*
            #serde_attr
            #garde_attr
            #field_vis #field_ident: #field_ty,
        });
    }

    let load_chain = load_chain(&container);

    // When every field is defaultable, generate a `Default` impl so the struct can
    // serve as an absent nested group (and so a fully-defaulted config loads even
    // when no source is present).
    let default_impl = if all_defaultable && generics.params.is_empty() {
        quote! {
            impl ::core::default::Default for #struct_ident {
                fn default() -> Self {
                    Self { #(#default_inits),* }
                }
            }
        }
    } else {
        quote!()
    };

    Ok(quote! {
        #(#struct_attrs)*
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::tork::__serde::Deserialize,
            ::tork::__garde::Validate,
        )]
        #[serde(crate = "::tork::__serde")]
        #vis struct #struct_ident #generics {
            #(#field_tokens)*
        }

        #default_impl

        #(#extra_fns)*

        impl #struct_ident {
            /// Loads and validates the configuration from the declared sources.
            ///
            /// Returns an error at boot when a source cannot be parsed or the
            /// value fails validation.
            pub fn load() -> ::tork::Result<Self> {
                ::tork::SettingsLoader::<Self>::new()
                    #load_chain
                    .load()
            }
        }
    })
}

/// Builds the `SettingsLoader` builder chain from the container options.
fn load_chain(container: &ContainerArgs) -> TokenStream2 {
    let mut chain = TokenStream2::new();
    if let Some(env_file) = &container.env_file {
        chain.extend(quote!(.env_file(#env_file)));
    }
    if let Some(prefix) = &container.prefix {
        chain.extend(quote!(.prefix(#prefix)));
    }
    if let Some(config_file) = &container.config_file {
        chain.extend(quote!(.config_file(#config_file)));
    }
    for file in &container.files {
        chain.extend(quote!(.file(#file)));
    }
    if let Some(secrets_dir) = &container.secrets_dir {
        chain.extend(quote!(.secrets_dir(#secrets_dir)));
    }
    chain
}

/// Produces the default value expression for a field. A string literal is
/// converted with `.into()` so `default = "..."` works for any field whose type
/// implements `From<&str>` (for example `String` or [`tork::SecretString`]); the
/// function's return type makes the conversion unambiguous. Other expressions
/// (numbers, booleans, paths) pass through unchanged.
fn default_value(expr: &Expr) -> TokenStream2 {
    let is_str_lit = matches!(expr, Expr::Lit(ExprLit { lit: Lit::Str(_), .. }));
    if is_str_lit {
        quote!(::core::convert::Into::into(#expr))
    } else {
        quote!(#expr)
    }
}
