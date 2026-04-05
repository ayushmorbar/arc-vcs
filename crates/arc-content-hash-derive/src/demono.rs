use std::collections::{HashMap, HashSet};

use quote::{format_ident, quote};
use syn::{
    FnArg, GenericParam, Ident, Item, ItemFn, Pat, Signature, Type, TypePath,
    TypeReference, TypeTraitObject, WherePredicate, parse2, spanned::Spanned,
};

pub(crate) fn inner(code: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let item: Item = match parse2(code) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };

    let Item::Fn(item_fn) = item else {
        return syn::Error::new(proc_macro2::Span::call_site(), "demono expects a function")
            .to_compile_error();
    };

    transform_fn(item_fn)
}

fn transform_fn(item_fn: ItemFn) -> proc_macro2::TokenStream {
    let mut bound_map: HashMap<Ident, syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>> =
        HashMap::new();

    for param in &item_fn.sig.generics.params {
        if let GenericParam::Type(type_param) = param
            && !type_param.bounds.is_empty()
        {
            bound_map.insert(type_param.ident.clone(), type_param.bounds.clone());
        }
    }

    if let Some(where_clause) = &item_fn.sig.generics.where_clause {
        for predicate in &where_clause.predicates {
            if let WherePredicate::Type(type_predicate) = predicate
                && let Type::Path(TypePath { qself: None, path }) = &type_predicate.bounded_ty
                && path.segments.len() == 1
            {
                bound_map.insert(path.segments[0].ident.clone(), type_predicate.bounds.clone());
            }
        }
    }

    let mut converted = HashSet::new();
    for input in &item_fn.sig.inputs {
        if let FnArg::Typed(pat) = input
            && let Some(ident) = referenced_generic_ident(&pat.ty)
            && bound_map.contains_key(&ident)
        {
            converted.insert(ident);
        }
    }

    if converted.is_empty() {
        return syn::Error::new(
            item_fn.sig.ident.span(),
            "demono found no supported generic reference parameters",
        )
        .to_compile_error();
    }

    let mut inner_generics = item_fn.sig.generics.clone();
    inner_generics.params = inner_generics
        .params
        .into_iter()
        .filter(|param| match param {
            GenericParam::Type(type_param) => !converted.contains(&type_param.ident),
            _ => true,
        })
        .collect();

    if let Some(where_clause) = &item_fn.sig.generics.where_clause {
        let mut inner_where = where_clause.clone();
        inner_where.predicates = inner_where
            .predicates
            .into_iter()
            .filter(|predicate| {
                if let WherePredicate::Type(type_predicate) = predicate
                    && let Type::Path(TypePath { qself: None, path }) = &type_predicate.bounded_ty
                    && path.segments.len() == 1
                {
                    return !converted.contains(&path.segments[0].ident);
                }
                true
            })
            .collect();
        inner_generics.where_clause = Some(inner_where);
    }

    let mut inner_inputs = item_fn.sig.inputs.clone();
    for input in &mut inner_inputs {
        if let FnArg::Typed(pat) = input
            && let Some((ident, is_mut)) = referenced_generic_ident_with_mut(&pat.ty)
            && converted.contains(&ident)
            && let Some(bounds) = bound_map.get(&ident)
        {
            let dyn_bounds = bounds.clone();
            if dyn_bounds.is_empty() {
                continue;
            }
            let trait_object = Type::TraitObject(TypeTraitObject {
                dyn_token: Some(Default::default()),
                bounds: dyn_bounds,
            });
            let ref_ty = Type::Reference(TypeReference {
                and_token: Default::default(),
                lifetime: None,
                mutability: if is_mut { Some(Default::default()) } else { None },
                elem: Box::new(trait_object),
            });
            *pat.ty = ref_ty;
        }
    }

    let mut call_args = Vec::new();
    for input in &item_fn.sig.inputs {
        match input {
            FnArg::Receiver(_) => call_args.push(quote!(self)),
            FnArg::Typed(pat) => match pat.pat.as_ref() {
                Pat::Ident(id) => {
                    let ident = &id.ident;
                    call_args.push(quote!(#ident));
                }
                _ => {
                    return syn::Error::new(
                        pat.pat.span(),
                        "demono requires identifier patterns for parameters",
                    )
                    .to_compile_error();
                }
            },
        }
    }

    let inner_ident = format_ident!("__demono_inner_{}", item_fn.sig.ident);
    let inner_sig = Signature {
        ident: inner_ident.clone(),
        generics: inner_generics,
        inputs: inner_inputs,
        ..item_fn.sig.clone()
    };

    let vis = item_fn.vis;
    let attrs = item_fn.attrs;
    let sig = item_fn.sig;
    let block = item_fn.block;

    quote! {
        #(#attrs)*
        #vis #sig {
            #[inline(never)]
            #inner_sig {
                #block
            }
            #inner_ident(#(#call_args),*)
        }
    }
}

fn referenced_generic_ident(ty: &Type) -> Option<Ident> {
    referenced_generic_ident_with_mut(ty).map(|(ident, _)| ident)
}

fn referenced_generic_ident_with_mut(ty: &Type) -> Option<(Ident, bool)> {
    let Type::Reference(TypeReference { elem, mutability, .. }) = ty else {
        return None;
    };
    let Type::Path(TypePath { qself: None, path }) = elem.as_ref() else {
        return None;
    };
    if path.segments.len() != 1 {
        return None;
    }
    Some((path.segments[0].ident.clone(), mutability.is_some()))
}
