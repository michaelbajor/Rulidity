use std::collections::HashMap;

#[derive(Debug, Clone)]
#[allow(dead_code)] // temporary until indexed fields are implemented properly
pub(crate) struct EventField {
    pub(crate) name: String,
    pub(crate) ty: syn::Type,
    pub(crate) indexed: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EventDef {
    pub(crate) name: String,
    pub(crate) fields: Vec<EventField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    Scalar,
    Mapping,
    Array,
}

#[derive(Debug, Clone)]
pub(crate) struct InternalFn {
    pub(crate) params: Vec<(syn::Ident, syn::Type)>,
    pub(crate) output: syn::ReturnType,
    pub(crate) block: syn::Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StorageField {
    pub(crate) slot: usize,
    pub(crate) kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceMethod {
    pub(crate) sig: String, // "transfer(address,uint256)"
    pub(crate) params: Vec<(syn::Ident, syn::Type)>,
    pub(crate) output: syn::ReturnType,
    pub(crate) mutable: bool, // &self -> STATICCAL, &mut self -> CALL
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterfaceDef {
    pub(crate) methods: std::collections::HashMap<String, InterfaceMethod>,
}

/// Everything that stays constant while lowering a whole contract.
pub(crate) struct Ctx<'a> {
    pub(crate) storage: &'a HashMap<syn::Ident, StorageField>,
    pub(crate) events: &'a HashMap<String, EventDef>,
    pub(crate) internal_functions: &'a HashMap<String, InternalFn>,
    // declared type of each storage field, so mapping/array chains can be
    // peeled a level at a time (e.g. Mapping<K, Mapping<K2, V>>)
    pub(crate) field_types: &'a HashMap<syn::Ident, syn::Type>,
    pub(crate) interfaces: &'a HashMap<String, InterfaceDef>,
}

/// Per function body state. `locals` and `param_offsets` describe the current
/// scope, `next_local` hands out fresh memory slots as we descend.
pub(crate) struct Lower<'a> {
    pub(crate) ctx: &'a Ctx<'a>,
    pub(crate) locals: HashMap<String, u32>,
    pub(crate) param_offsets: HashMap<String, u32>,
    pub(crate) next_local: u32,
    pub(crate) call_stack: Vec<String>, // recursion guard
    pub(crate) ret_string: bool,
}

impl Lower<'_> {
    /// Reserve a 32 byte memory slot, above the 0x00-0x40 scratchpad.
    pub(crate) fn alloc_local(&mut self) -> u32 {
        let offset = self.next_local;
        self.next_local += 0x20;
        offset
    }

    // reserve a continous calldata buffer
    // 1 word for the selector + 1 word per arg
    pub(crate) fn alloc_calldata(&mut self, nargs: usize) -> u32 {
        let base = self.next_local;
        self.next_local += 0x20 * (1 + nargs as u32);
        base
    }
}
