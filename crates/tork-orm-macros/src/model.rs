//! The `#[derive(Model)]` macro.
//!
//! Turns a struct into a database model by generating its table metadata, a
//! [`FromRow`] implementation that reads each field from its like-named column,
//! and the insert/primary-key value accessors the query layer uses.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Data, DeriveInput, Fields, Ident, LitInt, LitStr, Path, PathSegment, Token, parse_macro_input,
};

use crate::common::{krate, option_inner, sql_type_for, to_snake};

/// Container options parsed from `#[table(...)]`.
#[derive(Default)]
struct TableArgs {
    name: Option<LitStr>,
}

impl Parse for TableArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = TableArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "name" => {
                    input.parse::<Token![=]>()?;
                    args.name = Some(input.parse()?);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown table option `{other}`"),
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

/// Per-field options parsed from `#[field(...)]`.
#[derive(Default)]
struct FieldArgs {
    primary_key: bool,
    auto: bool,
    unique: bool,
    varchar_len: Option<u32>,
    foreign_key: Option<Path>,
    column: Option<String>,
}

impl Parse for FieldArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = FieldArgs::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();
            match name.as_str() {
                "primary_key" => args.primary_key = true,
                "auto" => args.auto = true,
                "unique" => args.unique = true,
                "varchar" => {
                    // varchar(length = N)
                    let content;
                    syn::parenthesized!(content in input);
                    let length_key: Ident = content.parse()?;
                    if length_key != "length" {
                        return Err(syn::Error::new(
                            length_key.span(),
                            "expected `length` inside `varchar(...)`",
                        ));
                    }
                    content.parse::<Token![=]>()?;
                    let lit: LitInt = content.parse()?;
                    args.varchar_len = Some(lit.base10_parse()?);
                }
                "foreign_key" => {
                    input.parse::<Token![=]>()?;
                    args.foreign_key = Some(input.parse()?);
                }
                "column" => {
                    input.parse::<Token![=]>()?;
                    let lit: LitStr = input.parse()?;
                    args.column = Some(lit.value());
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown field option `{other}`"),
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

/// Expands `#[derive(Model)]`.
pub fn expand(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand_model(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_model(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(Model)] supports only structs",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(Model)] supports only structs with named fields",
        ));
    };

    // Resolve the table name from `#[table(name = "...")]`, defaulting to the
    // snake_case of the struct name.
    let mut table_args = TableArgs::default();
    for attr in &input.attrs {
        if attr.path().is_ident("table") {
            table_args = attr.parse_args()?;
        }
    }
    let table_name = table_args
        .name
        .map(|lit| lit.value())
        .unwrap_or_else(|| to_snake(&ident.to_string()));

    let krate = krate();

    let mut column_defs: Vec<TokenStream2> = Vec::new();
    let mut column_consts: Vec<TokenStream2> = Vec::new();
    let mut from_row_fields: Vec<TokenStream2> = Vec::new();
    let mut insert_entries: Vec<TokenStream2> = Vec::new();
    let mut primary_key: Option<(Ident, String)> = None;

    for field in &named.named {
        let field_ident = field.ident.as_ref().expect("named field");
        let field_ty = &field.ty;

        let mut args = FieldArgs::default();
        for attr in &field.attrs {
            if attr.path().is_ident("field") {
                args = attr.parse_args()?;
            }
        }

        let column_name = args
            .column
            .clone()
            .unwrap_or_else(|| field_ident.to_string());

        let nullable = option_inner(field_ty).is_some();
        let base_ty = option_inner(field_ty).unwrap_or(field_ty);
        let sql_type = match args.varchar_len {
            Some(length) => quote!(#krate::SqlType::Varchar(#length)),
            None => sql_type_for(base_ty),
        };

        let foreign_key = match &args.foreign_key {
            Some(path) => {
                let (ty_path, column) = split_foreign_key(path)?;
                let column_lit = LitStr::new(&column, path.segments.last().unwrap().ident.span());
                quote!(::core::option::Option::Some(#krate::ForeignKeyDef {
                    table: <#ty_path as #krate::Model>::TABLE,
                    column: #column_lit,
                }))
            }
            None => quote!(::core::option::Option::None),
        };

        let primary_key_flag = args.primary_key;
        let auto_flag = args.auto;
        let unique_flag = args.unique;
        // `unique` is recorded for future migrations; silence the unused read.
        let _ = unique_flag;

        column_defs.push(quote!(#krate::ColumnDef {
            name: #column_name,
            sql_type: #sql_type,
            primary_key: #primary_key_flag,
            auto: #auto_flag,
            nullable: #nullable,
            foreign_key: #foreign_key,
        }));

        // A typed column handle constant, used as `User::is_active`. Nullable
        // columns carry their inner type so they compare against plain values;
        // `is_null` covers the null case.
        column_consts.push(quote!(
            #[allow(non_upper_case_globals)]
            pub const #field_ident: #krate::Column<Self, #base_ty> =
                #krate::Column::new(#table_name, #column_name);
        ));

        from_row_fields.push(quote!(#field_ident: row.get(#column_name)?));

        // Auto-assigned columns are filled by the database, so they are not written.
        if !auto_flag {
            insert_entries.push(quote!(
                (#column_name, #krate::BindValue::to_value(&self.#field_ident))
            ));
        }

        if primary_key_flag {
            if primary_key.is_some() {
                return Err(syn::Error::new_spanned(
                    field_ident,
                    "a model may declare only one primary key column",
                ));
            }
            primary_key = Some((field_ident.clone(), column_name));
        }
    }

    let Some((pk_field, pk_column)) = primary_key else {
        return Err(syn::Error::new_spanned(
            ident,
            "#[derive(Model)] requires one field marked `#[field(primary_key)]`",
        ));
    };

    Ok(quote! {
        impl #ident {
            #(#column_consts)*
        }

        impl #krate::FromRow for #ident {
            fn from_row(row: &#krate::Row) -> #krate::Result<Self> {
                ::core::result::Result::Ok(Self {
                    #(#from_row_fields),*
                })
            }
        }

        impl #krate::Model for #ident {
            const TABLE: &'static str = #table_name;
            const COLUMNS: &'static [#krate::ColumnDef] = &[
                #(#column_defs),*
            ];
            const PRIMARY_KEY: &'static str = #pk_column;

            fn insert_values(&self) -> ::std::vec::Vec<(&'static str, #krate::Value)> {
                ::std::vec![
                    #(#insert_entries),*
                ]
            }

            fn primary_key_value(&self) -> #krate::Value {
                #krate::BindValue::to_value(&self.#pk_field)
            }
        }
    })
}

/// Splits a `Type::column` foreign-key path into the referenced type path and the
/// column name. For example `User::id` yields (`User`, `"id"`).
fn split_foreign_key(path: &Path) -> syn::Result<(Path, String)> {
    let count = path.segments.len();
    if count < 2 {
        return Err(syn::Error::new_spanned(
            path,
            "foreign_key must be written as `Type::column`",
        ));
    }
    let column = path.segments[count - 1].ident.to_string();
    let mut segments: Punctuated<PathSegment, Token![::]> = Punctuated::new();
    for segment in path.segments.iter().take(count - 1) {
        segments.push(segment.clone());
    }
    let type_path = Path {
        leading_colon: path.leading_colon,
        segments,
    };
    Ok((type_path, column))
}
