//! The `#[derive(FormModel)]` macro.
//!
//! Generates a `FromMultipart` implementation that binds each field from a parsed
//! multipart body: file fields (`FileBytes` / `UploadFile`, optionally `Option` or
//! `Vec`) via the file takers, and text fields via the typed value takers, then
//! runs the `#[field(...)]` constraints. File fields are recognized by a `#[file]`
//! attribute or by their type, and `#[file(max_size/content_types/sniff)]` is
//! enforced after binding.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Ident, LitInt};

use crate::common::{
    file_binding, file_kind, file_validation, form_property, form_schema_body, krate,
    parse_file_args, text_binding, unwrap_multiplicity, Multiplicity,
};

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
    let mut schema_inserts = Vec::new();
    let mut schema_required = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().expect("named field");
        let (multiplicity, inner) = unwrap_multiplicity(&field.ty);

        let file_attr = field.attrs.iter().find(|attr| attr.path().is_ident("file"));
        let form_attr = field.attrs.iter().find(|attr| attr.path().is_ident("form"));

        if file_attr.is_some() || file_kind(inner).is_some() {
            let kind = file_kind(inner).ok_or_else(|| {
                syn::Error::new_spanned(
                    &field.ty,
                    "a #[file] field must be a FileBytes or UploadFile (optionally Option/Vec)",
                )
            })?;
            let args = match file_attr {
                Some(attr) => parse_file_args(attr)?,
                None => Default::default(),
            };
            let name = args.name.clone().unwrap_or_else(|| field_ident.to_string());
            bindings.push(file_binding(&krate, field_ident, kind, multiplicity, &name));
            let validation = file_validation(&krate, field_ident, kind, multiplicity, &args);
            if !validation.is_empty() {
                bindings.push(validation);
            }
            let (insert, required) = form_property(&krate, &name, true, inner, multiplicity);
            schema_inserts.push(insert);
            if required {
                schema_required.push(name);
            }
        } else {
            let name = match form_attr {
                Some(attr) => parse_file_args(attr)?
                    .name
                    .unwrap_or_else(|| field_ident.to_string()),
                None => field_ident.to_string(),
            };
            bindings.push(text_binding(
                &krate,
                field_ident,
                inner,
                multiplicity,
                &name,
            ));
            if let Some(constraints) = field_constraints(field)? {
                checks.push(text_checks(
                    &krate,
                    field_ident,
                    multiplicity,
                    &name,
                    &constraints,
                ));
            }
            let (insert, required) = form_property(&krate, &name, false, inner, multiplicity);
            schema_inserts.push(insert);
            if required {
                schema_required.push(name);
            }
        }

        names.push(field_ident);
    }

    let schema_body = form_schema_body(&krate, &schema_inserts, &schema_required);

    Ok(quote! {
        impl #krate::FromMultipart for #ident {
            async fn from_multipart(__form: &mut #krate::MultipartForm) -> #krate::Result<Self> {
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

            fn form_schema(generator: &mut #krate::__schemars::SchemaGenerator) -> #krate::__schemars::Schema {
                #schema_body
            }
        }
    })
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
        Multiplicity::Optional => {
            quote! { if let ::core::option::Option::Some(__value) = &#ident { #body } }
        }
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
    let Some(attr) = field
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("field"))
    else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn numeric_checks_and_text_checks_cover_all_multiplicities() {
        let constraints = Constraints {
            min_length: Some(parse_quote!(1)),
            max_length: Some(parse_quote!(5)),
            ge: Some(parse_quote!(1)),
            le: Some(parse_quote!(9)),
            gt: Some(parse_quote!(2)),
            lt: Some(parse_quote!(8)),
        };
        assert_eq!(numeric_checks(&constraints).len(), 4);

        let one = text_checks(
            &krate(),
            &parse_quote!(value),
            Multiplicity::One,
            "name",
            &constraints,
        )
        .to_string();
        assert!(one.contains("TOO_SHORT"));
        assert!(one.contains("TOO_LONG"));

        let optional = text_checks(
            &krate(),
            &parse_quote!(value),
            Multiplicity::Optional,
            "name",
            &constraints,
        )
        .to_string();
        assert!(optional.contains("Option :: Some"));

        let many = text_checks(
            &krate(),
            &parse_quote!(value),
            Multiplicity::Many,
            "name",
            &constraints,
        )
        .to_string();
        assert!(many.is_empty());
    }

    #[test]
    fn field_constraints_parse_known_keys_and_ignore_unknown_ones() {
        let field: syn::Field = parse_quote! {
            #[field(min_length = 1, max_length = 5, ge = 1, le = 9, gt = 2, lt = 8, title = "ignored")]
            name: String
        };
        let constraints = field_constraints(&field).unwrap().unwrap();
        assert_eq!(constraints.min_length.unwrap().base10_digits(), "1");
        assert_eq!(constraints.max_length.unwrap().base10_digits(), "5");
        assert!(constraints.ge.is_some());
        assert!(constraints.lt.is_some());

        let field: syn::Field = parse_quote!(name: String);
        assert!(field_constraints(&field).unwrap().is_none());
    }

    #[test]
    fn expand_derive_rejects_invalid_inputs_and_emits_bindings() {
        let tuple: DeriveInput = parse_quote!(
            struct Bad(String);
        );
        assert!(expand_derive(tuple)
            .unwrap_err()
            .to_string()
            .contains("requires a struct with named fields"));

        let enum_input: DeriveInput = parse_quote!(
            enum Bad {
                A,
            }
        );
        assert!(expand_derive(enum_input)
            .unwrap_err()
            .to_string()
            .contains("can only be derived for structs"));

        let invalid_file: DeriveInput = parse_quote! {
            struct Upload {
                #[file]
                payload: String,
            }
        };
        assert!(expand_derive(invalid_file)
            .unwrap_err()
            .to_string()
            .contains("FileBytes or UploadFile"));

        let valid: DeriveInput = parse_quote! {
            struct Upload {
                #[file(max_size = "1mb", content_types = ["image/png"], sniff = true)]
                avatar: tork::FileBytes,
                #[form(name = "display_name")]
                #[field(min_length = 1)]
                name: String,
            }
        };
        let tokens = expand_derive(valid).unwrap().to_string();
        assert!(tokens.contains("FromMultipart for Upload"));
        assert!(tokens.contains("take_file_bytes"));
        assert!(tokens.contains("display_name"));
        assert!(tokens.contains("VALIDATION_ERROR"));
    }
}
