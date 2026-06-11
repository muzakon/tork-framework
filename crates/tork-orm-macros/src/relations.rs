//! The `#[relations]` attribute macro.
//!
//! Applied to an `impl` block, it rewrites each method annotated with
//! `#[has_many(...)]` or `#[belongs_to(...)]` into an accessor returning a
//! [`Relation`] descriptor. The method body is generated, so the source method has
//! an empty body purely as a declaration.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{ImplItem, ItemImpl, LitStr, Path, Token, Type, parse_macro_input};

use crate::common::krate;

/// The relation kind named by the method attribute.
enum Kind {
    HasMany,
    BelongsTo,
}

/// The parsed contents of a `#[has_many(...)]` / `#[belongs_to(...)]` attribute.
struct RelationArgs {
    related: Path,
    foreign_key: Path,
}

impl Parse for RelationArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let related: Path = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: syn::Ident = input.parse()?;
        if key != "foreign_key" {
            return Err(syn::Error::new(key.span(), "expected `foreign_key = ...`"));
        }
        input.parse::<Token![=]>()?;
        let foreign_key: Path = input.parse()?;
        Ok(RelationArgs {
            related,
            foreign_key,
        })
    }
}

/// Expands `#[relations]` over an impl block.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    match expand_impl(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_impl(input: ItemImpl) -> syn::Result<TokenStream2> {
    let self_ty = &input.self_ty;
    let mut methods: Vec<TokenStream2> = Vec::new();

    for item in &input.items {
        let ImplItem::Fn(method) = item else {
            return Err(syn::Error::new_spanned(
                item,
                "#[relations] supports only method declarations",
            ));
        };

        let (kind, args) = parse_relation_attr(method)?;
        methods.push(build_accessor(self_ty, method, kind, args)?);
    }

    Ok(quote! {
        impl #self_ty {
            #(#methods)*
        }
    })
}

/// Reads the single relation attribute from a method.
fn parse_relation_attr(method: &syn::ImplItemFn) -> syn::Result<(Kind, RelationArgs)> {
    let mut found: Option<(Kind, RelationArgs)> = None;
    for attr in &method.attrs {
        let kind = if attr.path().is_ident("has_many") {
            Kind::HasMany
        } else if attr.path().is_ident("belongs_to") {
            Kind::BelongsTo
        } else {
            return Err(syn::Error::new_spanned(
                attr,
                "methods in #[relations] need a `#[has_many(...)]` or `#[belongs_to(...)]` attribute",
            ));
        };
        if found.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "a relation method may have only one relation attribute",
            ));
        }
        found = Some((kind, attr.parse_args()?));
    }
    found.ok_or_else(|| {
        syn::Error::new_spanned(
            &method.sig,
            "relation method needs a `#[has_many(...)]` or `#[belongs_to(...)]` attribute",
        )
    })
}

/// Builds the accessor method returning the relation descriptor.
fn build_accessor(
    self_ty: &Type,
    method: &syn::ImplItemFn,
    kind: Kind,
    args: RelationArgs,
) -> syn::Result<TokenStream2> {
    let krate = krate();
    let vis = &method.vis;
    let name = &method.sig.ident;
    let related = &args.related;
    let fk_column = foreign_key_column(&args.foreign_key)?;

    let body = match kind {
        // parent.parent_key = child.child_key
        Kind::HasMany => quote! {
            #krate::Relation::has_many(
                <#self_ty as #krate::Model>::TABLE,
                <#self_ty as #krate::Model>::PRIMARY_KEY,
                <#related as #krate::Model>::TABLE,
                #fk_column,
            )
        },
        // local.local_key = parent.parent_key
        Kind::BelongsTo => quote! {
            #krate::Relation::belongs_to(
                <#self_ty as #krate::Model>::TABLE,
                #fk_column,
                <#related as #krate::Model>::TABLE,
                <#related as #krate::Model>::PRIMARY_KEY,
            )
        },
    };

    Ok(quote! {
        #vis fn #name() -> #krate::Relation<#self_ty, #related> {
            #body
        }
    })
}

/// Extracts the column name from a `Type::column` foreign-key path.
fn foreign_key_column(path: &Path) -> syn::Result<LitStr> {
    let segment = path.segments.last().ok_or_else(|| {
        syn::Error::new_spanned(path, "foreign_key must be written as `Type::column`")
    })?;
    if path.segments.len() < 2 {
        return Err(syn::Error::new_spanned(
            path,
            "foreign_key must be written as `Type::column`",
        ));
    }
    Ok(LitStr::new(&segment.ident.to_string(), segment.ident.span()))
}
