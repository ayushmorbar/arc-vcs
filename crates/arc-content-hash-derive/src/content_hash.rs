use proc_macro2::Ident;
use proc_macro2::TokenStream;
use quote::format_ident;
use quote::quote;
use quote::quote_spanned;
use syn::Data;
use syn::Field;
use syn::Fields;
use syn::GenericParam;
use syn::Generics;
use syn::Index;
use syn::Type;
use syn::parse_quote;
use syn::spanned::Spanned as _;

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
