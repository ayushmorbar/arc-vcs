use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    Data, Field, Fields, GenericParam, Generics, Index, Type, parse_quote, spanned::Spanned as _,
};

pub fn add_trait_bounds(mut generics: Generics, core_path: &TokenStream) -> Generics {
    for param in &mut generics.params {
        if let GenericParam::Type(type_param) = param {
            type_param.bounds.push(parse_quote!(#core_path::store::content_hash::ContentHash));
        }
    }
    generics
}

pub fn generate_hash_impl(data: &Data, core_path: &TokenStream) -> TokenStream {
    match data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let hash_statements = fields.named.iter().map(|f| {
                    let field_name = &f.ident;
                    let ty = &f.ty;
                    quote_spanned! {ty.span()=>
                        <#ty as #core_path::store::content_hash::ContentHash>::hash_update(
                            &self.#field_name,
                            state,
                        );
                    }
                });
                quote! { #(#hash_statements)* }
            }
            Fields::Unnamed(fields) => {
                let hash_statements = fields.unnamed.iter().enumerate().map(|(i, f)| {
                    let index = Index::from(i);
                    let ty = &f.ty;
                    quote_spanned! {ty.span()=>
                        <#ty as #core_path::store::content_hash::ContentHash>::hash_update(
                            &self.#index,
                            state,
                        );
                    }
                });
                quote! { #(#hash_statements)* }
            }
            Fields::Unit => quote! {},
        },
        Data::Enum(data) => {
            let match_hash_statements = data.variants.iter().enumerate().map(|(i, v)| {
                let variant_id = &v.ident;
                match &v.fields {
                    Fields::Named(fields) => {
                        let bindings = enum_bindings(fields.named.iter());
                        let hash_statements =
                            hash_statements_for_enum_fields(i, fields.named.iter(), core_path);
                        quote_spanned! {v.span()=>
                            Self::#variant_id { #(#bindings),* } => {
                                #(#hash_statements)*
                            }
                        }
                    }
                    Fields::Unnamed(fields) => {
                        let bindings = enum_bindings(fields.unnamed.iter());
                        let hash_statements = hash_statements_for_enum_fields(
                            i,
                            fields.unnamed.iter(),
                            core_path,
                        );
                        quote_spanned! {v.span()=>
                            Self::#variant_id( #(#bindings),* ) => {
                                #(#hash_statements)*
                            }
                        }
                    }
                    Fields::Unit => {
                        let ix = index_to_ordinal(i);
                        quote_spanned! {v.span()=>
                            Self::#variant_id => {
                                #core_path::store::content_hash::ContentHash::hash_update(&#ix, state);
                            }
                        }
                    }
                }
            });
            quote! {
                match self {
                    #(#match_hash_statements)*
                }
            }
        }
        Data::Union(_) => unimplemented!("ContentHash cannot be derived for unions."),
    }
}

fn index_to_ordinal(ix: usize) -> u32 {
    u32::try_from(ix).expect("The number of enum variants overflows a u32.")
}

fn enum_bindings_with_type<'a>(fields: impl IntoIterator<Item = &'a Field>) -> Vec<(Type, Ident)> {
    fields
        .into_iter()
        .enumerate()
        .map(|(i, f)| (f.ty.clone(), f.ident.clone().unwrap_or(format_ident!("field_{}", i))))
        .collect::<Vec<_>>()
}

fn enum_bindings<'a>(fields: impl IntoIterator<Item = &'a Field>) -> Vec<Ident> {
    enum_bindings_with_type(fields).into_iter().map(|(_, binding)| binding).collect()
}

fn hash_statements_for_enum_fields<'a>(
    index: usize,
    fields: impl IntoIterator<Item = &'a Field>,
    core_path: &TokenStream,
) -> Vec<TokenStream> {
    let ix = index_to_ordinal(index);
    let typed_bindings = enum_bindings_with_type(fields);
    let mut hash_statements = Vec::with_capacity(typed_bindings.len() + 1);
    hash_statements
        .push(quote! {#core_path::store::content_hash::ContentHash::hash_update(&#ix, state);});
    for (ty, binding) in &typed_bindings {
        hash_statements.push(quote_spanned! {binding.span()=>
            <#ty as #core_path::store::content_hash::ContentHash>::hash_update(#binding, state);
        });
    }
    hash_statements
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;
    use syn::parse_quote;

    fn core() -> TokenStream {
        quote!(::arc_core)
    }

    fn generics_to_string(g: &Generics) -> String {
        let mut tokens = proc_macro2::TokenStream::new();
        g.to_tokens(&mut tokens);
        tokens.to_string()
    }

    fn parse_data(input: proc_macro2::TokenStream) -> Data {
        let derive_input: syn::DeriveInput = syn::parse2(input).expect("valid derive input");
        derive_input.data
    }

    #[test]
    fn index_to_ordinal_zero() {
        assert_eq!(index_to_ordinal(0), 0u32);
    }

    #[test]
    fn index_to_ordinal_typical() {
        assert_eq!(index_to_ordinal(3), 3u32);
    }

    #[test]
    fn index_to_ordinal_max_u32() {
        assert_eq!(index_to_ordinal(u32::MAX as usize), u32::MAX);
    }

    #[test]
    #[should_panic(expected = "overflows a u32")]
    fn index_to_ordinal_overflow() {
        let _ = index_to_ordinal(u32::MAX as usize + 1);
    }

    #[test]
    fn add_trait_bounds_empty_generics() {
        let generics: Generics = parse_quote!();
        let result = add_trait_bounds(generics, &core());
        assert_eq!(result.params.len(), 0);
    }

    #[test]
    fn add_trait_bounds_single_type_param() {
        let generics: Generics = parse_quote!(<T>);
        let result = add_trait_bounds(generics, &core());
        let s = generics_to_string(&result);
        assert!(s.contains("ContentHash"), "Expected ContentHash bound added, got: {s}");
    }

    #[test]
    fn add_trait_bounds_preserves_non_type_params() {
        let generics: Generics = parse_quote!(<'a, T, const N: usize>);
        let result = add_trait_bounds(generics, &core());
        let s = generics_to_string(&result);
        assert!(s.contains("'a"), "Expected lifetime preserved, got: {s}");
        assert!(s.contains("ContentHash"), "Expected ContentHash bound, got: {s}");
    }

    #[test]
    fn generate_hash_impl_struct_named_fields() {
        let data = parse_data(quote! {
            struct Foo {
                x: i32,
                y: String,
            }
        });
        let result = generate_hash_impl(&data, &core());
        let s = result.to_string();
        assert!(s.contains("hash_update"), "Expected hash_update calls, got: {s}");
        assert!(s.contains("x"), "Expected field x hashed, got: {s}");
        assert!(s.contains("y"), "Expected field y hashed, got: {s}");
    }

    #[test]
    fn generate_hash_impl_struct_unnamed_fields() {
        let data = parse_data(quote! {
            struct Bar(i32, String);
        });
        let result = generate_hash_impl(&data, &core());
        let s = result.to_string();
        assert!(s.contains("hash_update"), "Expected hash_update calls, got: {s}");
    }

    #[test]
    fn generate_hash_impl_struct_unit() {
        let data = parse_data(quote! {
            struct Baz;
        });
        let result = generate_hash_impl(&data, &core());
        let s = result.to_string().trim().to_string();
        assert!(s.is_empty(), "Expected empty body for unit struct, got: {s}");
    }

    #[test]
    fn generate_hash_impl_enum_with_variants() {
        let data = parse_data(quote! {
            enum MyEnum {
                Unit,
                Unnamed(i32, String),
                Named { x: i32, y: String },
            }
        });
        let result = generate_hash_impl(&data, &core());
        let s = result.to_string();
        assert!(s.contains("match"), "Expected match expression, got: {s}");
        assert!(s.contains("Unit"), "Expected Unit variant, got: {s}");
        assert!(s.contains("Unnamed"), "Expected Unnamed variant, got: {s}");
        assert!(s.contains("Named"), "Expected Named variant, got: {s}");
    }

    #[test]
    fn generate_hash_impl_enum_unit_variant_hashes_ordinal() {
        let data = parse_data(quote! {
            enum E { A, B }
        });
        let result = generate_hash_impl(&data, &core());
        let s = result.to_string();
        assert!(s.contains('0'), "Expected ordinal 0, got: {s}");
        assert!(s.contains('1'), "Expected ordinal 1, got: {s}");
    }

    #[test]
    #[should_panic]
    fn generate_hash_impl_union_unimplemented() {
        let data = parse_data(quote! {
            union Foo { x: i32, y: f32 }
        });
        let _ = generate_hash_impl(&data, &core());
    }

    #[test]
    fn enum_bindings_named_fields() {
        let data = parse_data(quote! {
            enum E { V { x: i32, y: String } }
        });
        let variant = match &data {
            Data::Enum(e) => e.variants.iter().next().unwrap(),
            _ => unreachable!(),
        };
        let bindings = enum_bindings(variant.fields.iter());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0], "x");
        assert_eq!(bindings[1], "y");
    }

    #[test]
    fn enum_bindings_unnamed_fields() {
        let data = parse_data(quote! {
            enum E { V(i32, String) }
        });
        let variant = match &data {
            Data::Enum(e) => e.variants.iter().next().unwrap(),
            _ => unreachable!(),
        };
        let bindings = enum_bindings(variant.fields.iter());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0], "field_0");
        assert_eq!(bindings[1], "field_1");
    }

    #[test]
    fn enum_bindings_empty() {
        let data = parse_data(quote! {
            enum E { V }
        });
        let variant = match &data {
            Data::Enum(e) => e.variants.iter().next().unwrap(),
            _ => unreachable!(),
        };
        let bindings = enum_bindings(variant.fields.iter());
        assert!(bindings.is_empty());
    }

    #[test]
    fn enum_bindings_with_type_returns_typed_pairs() {
        let data = parse_data(quote! {
            enum E { V(i32, String) }
        });
        let variant = match &data {
            Data::Enum(e) => e.variants.iter().next().unwrap(),
            _ => unreachable!(),
        };
        let typed = enum_bindings_with_type(variant.fields.iter());
        assert_eq!(typed.len(), 2);
        assert_eq!(typed[0].1, "field_0");
        assert_eq!(typed[1].1, "field_1");
    }

    #[test]
    fn hash_statements_for_enum_fields_includes_ordinal_and_field_hashes() {
        let data = parse_data(quote! {
            enum E { V(i32, String) }
        });
        let variant = match &data {
            Data::Enum(e) => e.variants.iter().next().unwrap(),
            _ => unreachable!(),
        };
        let stmts = hash_statements_for_enum_fields(2, variant.fields.iter(), &core());
        let s = stmts.iter().map(|ts| ts.to_string()).collect::<Vec<_>>().join(" ");
        assert!(s.contains('2'), "Expected ordinal 2, got: {s}");
        assert!(s.contains("hash_update"), "Expected hash_update calls, got: {s}");
    }
}
