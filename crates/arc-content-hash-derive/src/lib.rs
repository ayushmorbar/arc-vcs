mod content_hash;
mod demono;

extern crate proc_macro;

use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(ContentHash)]
pub fn derive_content_hash(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let core_path = core_crate_path();

    let hash_impl = content_hash::generate_hash_impl(&input.data, &core_path);
    let generics = content_hash::add_trait_bounds(input.generics, &core_path);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = quote! {
        #[automatically_derived]
        impl #impl_generics #core_path::store::content_hash::ContentHash for #name #ty_generics
        #where_clause {
            fn hash_update(&self, state: &mut #core_path::store::content_hash::Hasher) {
                #hash_impl
            }
        }
    };

    expanded.into()
}

/// De-monomorphize generic function parameters by routing through one
/// dynamically-dispatched inner implementation.
#[proc_macro_attribute]
pub fn demono(
    _attrs: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    demono::inner(input.into()).into()
}

fn core_crate_path() -> proc_macro2::TokenStream {
    match crate_name("arc-core") {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{name}");
            quote!(::#ident)
        }
        Err(_) => {
            let ident = format_ident!("arc_core");
            quote!(::#ident)
        }
    }
}
