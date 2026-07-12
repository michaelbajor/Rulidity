use syn::{FnArg, Pat, token::Comma};

use crate::model::FieldKind;

pub(crate) fn params_of(
    inputs: syn::punctuated::Punctuated<FnArg, Comma>,
) -> Vec<(syn::Ident, syn::Type)> {
    let mut params = Vec::new();
    for arg in inputs {
        match arg {
            FnArg::Receiver(_) => {} // skip, that's self, &self and &mut self
            FnArg::Typed(pat_type) => {
                let this_type = *pat_type.ty;
                let name = match *pat_type.pat {
                    Pat::Ident(pat_ident) => pat_ident.ident,
                    _ => panic!("invalid arg identifier"),
                };

                params.push((name, this_type));
            }
        }
    }

    params
}

/// converts rust types into solidity types
pub(crate) fn abi_type_of(ty: syn::Type) -> String {
    let type_str = match ty {
        syn::Type::Path(type_path) => {
            let path = type_path.path;
            path.segments.last().unwrap().clone().ident
        }
        _ => panic!("Unsupported type"),
    };

    let segment_str = type_str.to_string();
    let ret = match segment_str.as_str() {
        "U256" => "uint256",
        "Address" => "address",
        "bool" => "bool",
        "ShortString" => "string",
        _ => panic!("Unsupported parameter type {segment_str}"), // @todo add more Solidity types
    };

    ret.to_owned()
}

/// calculates the signature of whatever is given
/// used to get the function signatures, so fn balance_of(&self, who: Address) -> U256 becomes balance_of(Address)
/// and event signatures, so struct Event { who: Address, amount: U256 } becomes Event(Address,uint256)
pub(crate) fn signature_string(name: String, params: Vec<(syn::Ident, syn::Type)>) -> String {
    let types: Vec<String> = params
        .iter()
        .map(|(_, ty)| abi_type_of(ty.clone()))
        .collect();
    let args_str = types.join(",");
    let mut func_signature = name;
    func_signature.push('(');
    func_signature.push_str(&args_str);
    func_signature.push(')');

    func_signature
}

pub(crate) fn field_kind_of(ty: &syn::Type) -> FieldKind {
    let type_str = match ty {
        syn::Type::Path(type_path) => &type_path.path.segments.last().unwrap().ident.to_string(),
        _ => panic!("This type cannot be converted to FieldKind"),
    };

    match type_str.as_str() {
        "Mapping" => FieldKind::Mapping,
        "Array" => FieldKind::Array,
        _ => FieldKind::Scalar,
    }
}

pub(crate) fn abi_param_json(name: &str, ty: &syn::Type, indexed: Option<bool>) -> String {
    let t = abi_type_of(ty.clone());
    match indexed {
        Some(idx) => {
            format!(r#"{{"name":"{name}","type":"{t}","indexed":{idx},"internalType":"{t}"}}"#)
        }
        None => format!(r#"{{"name":"{name}","type":"{t}","internalType":"{t}"}}"#),
    }
}

pub(crate) fn is_mut_receiver(inputs: &syn::punctuated::Punctuated<FnArg, Comma>) -> bool {
    matches!(inputs.first(), Some(FnArg::Receiver(r)) if r.mutability.is_some())
}
