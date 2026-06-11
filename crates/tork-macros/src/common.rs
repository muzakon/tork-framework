//! Shared helpers for the procedural macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, GenericArgument, Ident, LitBool, LitInt, LitStr, Meta, PathArguments, Token, Type};

/// Returns the path to the facade crate that all generated code references.
///
/// Using the `tork` facade (rather than `tork-core`) means generated code
/// compiles inside user crates that depend only on `tork`.
pub fn krate() -> TokenStream {
    quote! { ::tork }
}

/// Extracts the placeholder names from a route path.
///
/// For example, `"/users/{user_id}/orders/{order_id}"` yields `["user_id",
/// "order_id"]`. The wildcard marker in `{*rest}` is stripped.
pub fn path_param_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = path.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'{' {
            if let Some(offset) = path[index + 1..].find('}') {
                let inner = &path[index + 1..index + 1 + offset];
                names.push(inner.trim_start_matches('*').to_owned());
                index += offset + 2;
                continue;
            }
        }
        index += 1;
    }

    names
}

// --- Shared form / file helpers (used by the route and form-model macros) ---

/// How many values a field binds.
#[derive(Clone, Copy)]
pub(crate) enum Multiplicity {
    One,
    Optional,
    Many,
}

/// Which file type a field binds.
#[derive(Clone, Copy)]
pub(crate) enum FileKind {
    Bytes,
    Upload,
}

/// Returns the inner type of `Wrapper<T>` when the final segment matches `wrapper`.
pub(crate) fn generic_arg<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
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

/// Splits a field type into its multiplicity and inner type.
pub(crate) fn unwrap_multiplicity(ty: &Type) -> (Multiplicity, &Type) {
    if let Some(inner) = generic_arg(ty, "Option") {
        (Multiplicity::Optional, inner)
    } else if let Some(inner) = generic_arg(ty, "Vec") {
        (Multiplicity::Many, inner)
    } else {
        (Multiplicity::One, ty)
    }
}

/// Returns the file kind if `ty` is `FileBytes` or `UploadFile`.
pub(crate) fn file_kind(ty: &Type) -> Option<FileKind> {
    let Type::Path(path) = ty else { return None };
    match path.path.segments.last()?.ident.to_string().as_str() {
        "FileBytes" => Some(FileKind::Bytes),
        "UploadFile" => Some(FileKind::Upload),
        _ => None,
    }
}

/// Parses a byte size such as `"64KB"`, `"1MB"`, or a plain byte count.
pub(crate) fn parse_size(value: &LitStr) -> syn::Result<usize> {
    let text = value.value();
    let lower = text.trim().to_ascii_lowercase();
    let units: [(&str, usize); 7] = [
        ("mib", 1024 * 1024),
        ("kib", 1024),
        ("mb", 1024 * 1024),
        ("kb", 1024),
        ("m", 1024 * 1024),
        ("k", 1024),
        ("b", 1),
    ];
    for (suffix, multiplier) in units {
        if let Some(number) = lower.strip_suffix(suffix) {
            let parsed: usize = number
                .trim()
                .parse()
                .map_err(|_| syn::Error::new(value.span(), format!("invalid byte size `{text}`")))?;
            return Ok(parsed * multiplier);
        }
    }
    lower
        .parse::<usize>()
        .map_err(|_| syn::Error::new(value.span(), format!("invalid byte size `{text}`")))
}

/// Parses a duration such as `"60s"`, `"2m"`, `"500ms"`, or plain seconds.
pub(crate) fn parse_duration_ms(value: &LitStr) -> syn::Result<u64> {
    let text = value.value();
    let lower = text.trim().to_ascii_lowercase();
    let units: [(&str, u64); 4] = [("ms", 1), ("s", 1000), ("m", 60_000), ("h", 3_600_000)];
    for (suffix, multiplier) in units {
        if let Some(number) = lower.strip_suffix(suffix) {
            let parsed: u64 = number
                .trim()
                .parse()
                .map_err(|_| syn::Error::new(value.span(), format!("invalid duration `{text}`")))?;
            return Ok(parsed * multiplier);
        }
    }
    lower
        .parse::<u64>()
        .map(|secs| secs * 1000)
        .map_err(|_| syn::Error::new(value.span(), format!("invalid duration `{text}`")))
}

/// Constraints parsed from a `#[file(...)]` attribute.
#[derive(Default)]
pub(crate) struct FileArgs {
    pub name: Option<String>,
    pub max_size: Option<usize>,
    pub content_types: Vec<String>,
    pub sniff: bool,
}

/// Parses a `#[file(...)]` attribute (a bare `#[file]` yields the defaults).
pub(crate) fn parse_file_args(attr: &Attribute) -> syn::Result<FileArgs> {
    let mut args = FileArgs::default();
    if matches!(attr.meta, Meta::Path(_)) {
        return Ok(args);
    }
    attr.parse_nested_meta(|meta| {
        let key = meta
            .path
            .get_ident()
            .map(|ident| ident.to_string())
            .unwrap_or_default();
        match key.as_str() {
            "name" => {
                let value: LitStr = meta.value()?.parse()?;
                args.name = Some(value.value());
            }
            "max_size" | "max_size_each" => {
                let value: LitStr = meta.value()?.parse()?;
                args.max_size = Some(parse_size(&value)?);
            }
            "content_types" => {
                let value = meta.value()?;
                let content;
                syn::bracketed!(content in value);
                let items = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                args.content_types = items.into_iter().map(|item| item.value()).collect();
            }
            "sniff" => {
                args.sniff = match meta.value() {
                    Ok(value) => value.parse::<LitBool>()?.value,
                    Err(_) => true,
                };
            }
            "max_files" => {
                let _ = meta.value()?.parse::<LitInt>()?;
            }
            _ => {
                let _ = meta.value().and_then(|value| value.parse::<Expr>());
            }
        }
        Ok(())
    })?;
    Ok(args)
}

/// Whether a `#[file]` attribute declares any validation rule.
pub(crate) fn has_file_rule(args: &FileArgs) -> bool {
    args.max_size.is_some() || !args.content_types.is_empty() || args.sniff
}

/// Builds a `FileRule` expression from the parsed file arguments.
pub(crate) fn file_rule_tokens(krate: &TokenStream, args: &FileArgs) -> TokenStream {
    let max_size = match args.max_size {
        Some(bytes) => quote! { ::core::option::Option::Some(#bytes) },
        None => quote! { ::core::option::Option::None },
    };
    let content_types = &args.content_types;
    let sniff = args.sniff;
    quote! {
        #krate::FileRule {
            max_size: #max_size,
            content_types: &[#(#content_types),*],
            sniff: #sniff,
        }
    }
}

/// Builds the binding for a file field, taken from `__form` by `name`.
pub(crate) fn file_binding(
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
        // `UploadFile` is read and validated through `&mut`, so its bindings are
        // mutable; the allow covers handlers that only inspect metadata.
        (FileKind::Upload, Multiplicity::One) => quote! {
            #[allow(unused_mut)]
            let mut #ident = __form.take_upload_file(#name).ok_or_else(#missing)?;
        },
        (FileKind::Upload, Multiplicity::Optional) => quote! {
            #[allow(unused_mut)]
            let mut #ident = __form.take_upload_file(#name);
        },
        (FileKind::Upload, Multiplicity::Many) => quote! {
            #[allow(unused_mut)]
            let mut #ident = __form.take_upload_file_list(#name);
        },
    }
}

/// Builds the binding for a text field, taken from `__form` by `name`.
pub(crate) fn text_binding(
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

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{Attribute, parse_quote};

    #[test]
    fn path_param_names_strip_wildcards() {
        assert_eq!(
            path_param_names("/users/{user_id}/files/{*rest}"),
            vec!["user_id".to_owned(), "rest".to_owned()]
        );
    }

    #[test]
    fn parse_size_understands_suffixes() {
        assert_eq!(parse_size(&parse_quote!("64KB")).unwrap(), 64 * 1024);
        assert_eq!(parse_size(&parse_quote!("2MiB")).unwrap(), 2 * 1024 * 1024);
        assert!(parse_size(&parse_quote!("wat")).is_err());
    }

    #[test]
    fn parse_duration_ms_understands_units() {
        assert_eq!(parse_duration_ms(&parse_quote!("500ms")).unwrap(), 500);
        assert_eq!(parse_duration_ms(&parse_quote!("2m")).unwrap(), 120_000);
        assert_eq!(parse_duration_ms(&parse_quote!("3")).unwrap(), 3_000);
        assert!(parse_duration_ms(&parse_quote!("soon")).is_err());
    }

    #[test]
    fn unwrap_multiplicity_and_file_kind_detect_supported_shapes() {
        let optional: Type = parse_quote!(Option<FileBytes>);
        let many: Type = parse_quote!(Vec<UploadFile>);
        let plain: Type = parse_quote!(String);

        let (optional_multi, optional_inner) = unwrap_multiplicity(&optional);
        let (many_multi, many_inner) = unwrap_multiplicity(&many);
        let (plain_multi, plain_inner) = unwrap_multiplicity(&plain);

        assert!(matches!(optional_multi, Multiplicity::Optional));
        assert!(matches!(many_multi, Multiplicity::Many));
        assert!(matches!(plain_multi, Multiplicity::One));
        assert!(matches!(file_kind(optional_inner), Some(FileKind::Bytes)));
        assert!(matches!(file_kind(many_inner), Some(FileKind::Upload)));
        assert!(file_kind(plain_inner).is_none());
    }

    #[test]
    fn parse_file_args_reads_validation_flags() {
        let attr: Attribute =
            parse_quote!(#[file(name = "avatar", max_size = "64KB", content_types = ["image/png"], sniff = true)]);

        let args = parse_file_args(&attr).unwrap();

        assert_eq!(args.name.as_deref(), Some("avatar"));
        assert_eq!(args.max_size, Some(64 * 1024));
        assert_eq!(args.content_types, vec!["image/png".to_owned()]);
        assert!(args.sniff);
        assert!(has_file_rule(&args));
    }
}

/// Builds the schema property value for one form field, and whether it is
/// required. The generated code reads the surrounding `generator` variable.
pub(crate) fn form_property(
    krate: &TokenStream,
    name: &str,
    is_file: bool,
    inner: &Type,
    multiplicity: Multiplicity,
) -> (TokenStream, bool) {
    let value = if is_file {
        match multiplicity {
            Multiplicity::Many => quote! {
                #krate::__serde_json::json!({
                    "type": "array",
                    "items": { "type": "string", "format": "binary" }
                })
            },
            _ => quote! {
                #krate::__serde_json::json!({ "type": "string", "format": "binary" })
            },
        }
    } else {
        let sub = quote! { generator.subschema_for::<#inner>().to_value() };
        match multiplicity {
            Multiplicity::Many => quote! {
                #krate::__serde_json::json!({ "type": "array", "items": #sub })
            },
            _ => sub,
        }
    };
    let insert = quote! { __properties.insert(#name.to_owned(), #value); };
    (insert, matches!(multiplicity, Multiplicity::One))
}

/// Builds the body of a form schema function from property inserts and the names
/// of the required fields.
pub(crate) fn form_schema_body(
    krate: &TokenStream,
    inserts: &[TokenStream],
    required: &[String],
) -> TokenStream {
    quote! {
        let mut __properties = #krate::__serde_json::Map::new();
        #(#inserts)*
        let __schema = #krate::__serde_json::json!({
            "type": "object",
            "properties": #krate::__serde_json::Value::Object(__properties),
            "required": [ #(#required),* ],
        });
        #krate::__schemars::Schema::try_from(__schema)
            .unwrap_or_else(|_| #krate::__schemars::json_schema!({ "type": "object" }))
    }
}

/// Builds the validation statement for a file field, applied after binding.
pub(crate) fn file_validation(
    krate: &TokenStream,
    ident: &Ident,
    kind: FileKind,
    multiplicity: Multiplicity,
    args: &FileArgs,
) -> TokenStream {
    if !has_file_rule(args) {
        return TokenStream::new();
    }
    let rule = file_rule_tokens(krate, args);
    match (kind, multiplicity) {
        (FileKind::Bytes, Multiplicity::One) => quote! {
            #krate::__validate_file_bytes(&#ident, &#rule)?;
        },
        (FileKind::Bytes, Multiplicity::Optional) => quote! {
            if let ::core::option::Option::Some(__f) = &#ident {
                #krate::__validate_file_bytes(__f, &#rule)?;
            }
        },
        (FileKind::Bytes, Multiplicity::Many) => quote! {
            { let __rule = #rule; for __f in &#ident { #krate::__validate_file_bytes(__f, &__rule)?; } }
        },
        (FileKind::Upload, Multiplicity::One) => quote! {
            #krate::__validate_upload(&mut #ident, &#rule).await?;
        },
        (FileKind::Upload, Multiplicity::Optional) => quote! {
            if let ::core::option::Option::Some(__f) = &mut #ident {
                #krate::__validate_upload(__f, &#rule).await?;
            }
        },
        (FileKind::Upload, Multiplicity::Many) => quote! {
            { let __rule = #rule; for __f in &mut #ident { #krate::__validate_upload(__f, &__rule).await?; } }
        },
    }
}
