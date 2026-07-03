use std::collections::HashMap;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ImplItem, Item, ItemMod, parse_macro_input};

mod abi;
mod lower;
mod model;

use abi::{
    abi_param_json, field_kind_of, is_mut_receiver, param_offset, params_of, signature_string,
};
use lower::lower_block;
use model::{Ctx, EventDef, EventField, InternalFn, StorageField};

use crate::lower::lower_constructor;

/// #[contract] mod my_contract { ... }
#[proc_macro_attribute]
pub fn contract(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let module: ItemMod = parse_macro_input!(item as ItemMod);
    let items = module.content.unwrap().1;

    // collect metadata; the items themselves are re-emitted as real Rust below
    let mut storage_fields = Vec::new();
    let mut external_functions = Vec::new();

    let mut events: HashMap<String, EventDef> = HashMap::new();

    let mut constructor: Option<(
        syn::Block,
        syn::punctuated::Punctuated<syn::FnArg, syn::token::Comma>,
    )> = None;

    let mut internal_functions: HashMap<String, InternalFn> = HashMap::new();

    for item in &items {
        match item {
            Item::Struct(s) if has_attr(&s.attrs, "storage") => {
                for field in &s.fields {
                    storage_fields.push((field.ident.clone(), field.ty.clone()));
                }
            }
            Item::Struct(s) if has_attr(&s.attrs, "event") => {
                let event_fields: Vec<EventField> = s
                    .fields
                    .iter()
                    .map(|f| EventField {
                        name: f.ident.clone().unwrap().to_string(),
                        ty: f.ty.clone(),
                        indexed: has_attr(&f.attrs, "indexed"),
                    })
                    .collect();

                events.insert(
                    s.ident.to_string(),
                    EventDef {
                        name: s.ident.to_string(),
                        fields: event_fields,
                    },
                );
            }
            Item::Impl(imp) => {
                for impl_item in &imp.items {
                    if let ImplItem::Fn(m) = impl_item {
                        if has_attr(&m.attrs, "external") {
                            external_functions.push((
                                m.sig.ident.clone(),
                                m.sig.output.clone(),
                                m.block.clone(),
                                m.sig.inputs.clone(),
                            ));
                        } else if has_attr(&m.attrs, "constructor") {
                            if constructor.is_some() {
                                panic!("Only one constructor is allowed");
                            }
                            constructor = Some((m.block.clone(), m.sig.inputs.clone()));
                        } else {
                            // those are internal functions
                            internal_functions.insert(
                                m.sig.ident.to_string(),
                                InternalFn {
                                    params: params_of(m.sig.inputs.clone()),
                                    output: m.sig.output.clone(),
                                    block: m.block.clone(),
                                },
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // assign storage slots by declaration order
    let mut storage: HashMap<syn::Ident, StorageField> = HashMap::new();
    for (slot_id, (ident, ty)) in storage_fields.iter().enumerate() {
        storage.insert(
            ident.clone().unwrap(),
            StorageField {
                slot: slot_id,
                kind: field_kind_of(ty),
            },
        );
    }

    let ctx = Ctx {
        storage: &storage,
        events: &events,
        internal_functions: &internal_functions,
    };

    let constructor_build = match &constructor {
        Some((block, inputs)) => {
            let params = params_of(inputs.clone());
            let body = lower_constructor(block, &params, &ctx);

            quote! {
                fn build_constructor(asm: &mut ::rulidity::asm::Asm) {
                    #body
                }
            }
        }
        None => quote! {},
    };

    let ctor_arg = if constructor.is_some() {
        quote! { ::std::option::Option::Some(build_constructor as fn(&mut ::rulidity::asm::Asm)) }
    } else {
        quote! { ::std::option::Option::None }
    };

    // one `build_*` fn per external fn, lowered from its body
    let build_fns: Vec<proc_macro2::TokenStream> = external_functions
        .iter()
        .map(|(name, output, block, inputs)| {
            let build_ident = format_ident!("build_{}", name);
            let offsets = param_offset(params_of(inputs.clone()));
            let body = lower_block(block, output, &ctx, offsets);
            quote! {
                fn #build_ident(asm: &mut ::rulidity::asm::Asm) {
                    #body
                }
            }
        })
        .collect();

    // registration entries for the dispatcher
    let function_inits: Vec<proc_macro2::TokenStream> = external_functions
        .iter()
        .map(|(name, _output, _block, inputs)| {
            let build_ident = format_ident!("build_{}", name);
            let sig = signature_string(name.to_string(), params_of(inputs.clone()));
            quote! {
                ::rulidity::contract::Function::new(builder.selector(#sig), #build_ident)
            }
        })
        .collect();

    let mut abi_entries: Vec<String> = external_functions
        .iter()
        .map(|(name, output, _block, inputs)| {
            let inputs_json = params_of(inputs.clone())
                .iter()
                .map(|(id, ty)| abi_param_json(&id.to_string(), ty, None))
                .collect::<Vec<_>>()
                .join(",");

            let outputs_json = match output {
                syn::ReturnType::Default => String::new(),
                syn::ReturnType::Type(_, ty) => abi_param_json("", ty, None),
            };
            let mutability = if is_mut_receiver(inputs) {
                "nonpayable"
            } else {
                "view"
            };

            format!(
                r#"{{"type":"function","name":"{name}","inputs":[{inputs_json}],"outputs":[{outputs_json}],"stateMutability":"{mutability}"}}"#
            )
        })
        .collect();
    let mut evs: Vec<&EventDef> = events.values().collect();
    evs.sort_by(|first, second| first.name.cmp(&second.name));
    for def in evs {
        let inputs_json = def
            .fields
            .iter()
            .map(|f| abi_param_json(&f.name, &f.ty, Some(f.indexed)))
            .collect::<Vec<_>>()
            .join(",");
        abi_entries.push(format!(
            r#"{{"type":"event","name":"{}","inputs":[{inputs_json}],"anonymous":false}}"#,
            def.name
        ));
    }
    if let Some((_, inputs)) = &constructor {
        let inputs_json = params_of(inputs.clone())
            .iter()
            .map(|(id, ty)| abi_param_json(&id.to_string(), ty, None))
            .collect::<Vec<_>>()
            .join(",");
        abi_entries.push(format!(
            r#"{{"type":"constructor","inputs":[{inputs_json}],"stateMutability":"nonpayable"}}"#
        ));
    }
    let abi_str = format!("[{}]", abi_entries.join(","));

    // re-emit the user's items as real Rust, minus the helper attributes
    let real_items: Vec<proc_macro2::TokenStream> = items.iter().map(strip_helper_attrs).collect();

    let mod_name = &module.ident;

    let expanded = quote! {
        #[allow(dead_code, non_snake_case)]
        mod #mod_name {
            #(#real_items)*

            pub fn deploy_code() -> ::std::vec::Vec<u8> {
                let builder = ::rulidity::contract::Builder;
                let funcs = ::std::vec![#(#function_inits),*];
                builder.assemble_contract(funcs, #ctor_arg)
            }

            pub fn abi_json() -> &'static str { #abi_str }

            #constructor_build

            #(#build_fns)*
        }
    };

    // eprintln!("{}", expanded);
    expanded.into()
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

/// Clone an item and remove the #[storage] / #[external] helper attributes,
/// so it re-emits as ordinary (type-checkable) Rust.
fn strip_helper_attrs(item: &Item) -> proc_macro2::TokenStream {
    let mut item = item.clone();
    match &mut item {
        Item::Struct(s) => {
            s.attrs
                .retain(|a| !a.path().is_ident("storage") && !a.path().is_ident("event"));
            // #[indexed] lives on the event's fields, strip that too, for now
            for field in &mut s.fields {
                field.attrs.retain(|a| !a.path().is_ident("indexed"));
            }
        }
        Item::Impl(imp) => {
            for impl_item in &mut imp.items {
                if let ImplItem::Fn(f) = impl_item {
                    f.attrs.retain(|a| {
                        !a.path().is_ident("external") && !a.path().is_ident("constructor")
                    });
                }
            }
        }
        _ => {}
    }
    quote! { #item }
}
