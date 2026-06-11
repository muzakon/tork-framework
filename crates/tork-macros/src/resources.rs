//! The `#[derive(Resources)]` macro.
//!
//! Generates a `Resources` implementation that registers each `#[resource]`
//! field (by a clone) into the registry, and a `FromRequest` implementation for
//! each resource field type so that resources can be injected directly into
//! services and handlers.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, Type, parse_macro_input};

use crate::common::krate;

/// Returns `true` if the type's final path segment is `Arc`.
fn is_arc(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().map(|s| s.ident == "Arc").unwrap_or(false))
}

/// Expands `#[derive(Resources)]` over a named struct.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_derive(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let fields: Vec<&Field> = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => named.named.iter().collect(),
            // A container with no resources is allowed.
            Fields::Unit => Vec::new(),
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "#[derive(Resources)] requires named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "#[derive(Resources)] can only be derived for structs",
            ));
        }
    };

    let krate = krate();
    let ident = &input.ident;

    let mut inserts = Vec::new();
    let mut extractors = Vec::new();

    for field in fields {
        if !field.attrs.iter().any(|attr| attr.path().is_ident("resource")) {
            continue;
        }

        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;

        inserts.push(quote! {
            registry.insert(::core::clone::Clone::clone(&self.#field_ident));
        });

        // Each resource type resolves itself from the registry, so it can be
        // injected directly (`db: Db`) or as a field of an `#[derive(Inject)]`
        // service. `Arc<T>` resources are covered by the blanket `FromRequest for
        // Arc<T>` in the core crate (the orphan rules forbid generating one here),
        // so only a registry insert is emitted for them.
        if !is_arc(field_ty) {
            extractors.push(quote! {
                impl #krate::FromRequest for #field_ty {
                    fn from_request(
                        ctx: & #krate::RequestContext,
                    ) -> impl ::core::future::Future<Output = #krate::Result<Self>> + Send {
                        let resolved = ctx.resource::<#field_ty>();
                        async move { resolved }
                    }
                }
            });
        }
    }

    Ok(quote! {
        impl #krate::Resources for #ident {
            fn register(&self, registry: &mut #krate::StateMap) {
                #(#inserts)*
            }
        }

        #(#extractors)*
    })
}
