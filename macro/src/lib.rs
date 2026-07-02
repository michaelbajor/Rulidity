use std::collections::HashMap;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItem, Item, ItemMod, Pat, parse_macro_input, token::Comma};

#[derive(Debug, Clone)]
#[allow(dead_code)] // temporary until indexed fields are implemented properly
struct EventField {
    name: String,
    ty: syn::Type,
    indexed: bool,
}

#[derive(Debug, Clone)]
struct EventDef {
    name: String,
    fields: Vec<EventField>,
}

/// `#[contract] mod my_contract { ... }`
#[proc_macro_attribute]
pub fn contract(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let module: ItemMod = parse_macro_input!(item as ItemMod);
    let items = module.content.unwrap().1;

    // collect metadata; the items themselves are re-emitted as real Rust below
    let mut storage_fields = Vec::new();
    let mut external_functions = Vec::new();

    let mut events: HashMap<String, EventDef> = HashMap::new();

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
                    if let ImplItem::Fn(m) = impl_item
                        && has_attr(&m.attrs, "external")
                    {
                        external_functions.push((
                            m.sig.ident.clone(),
                            m.sig.output.clone(),
                            m.block.clone(),
                            m.sig.inputs.clone(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // assign storage slots by declaration order
    let mut storage_slots = HashMap::new();
    for (slot_id, (ident, _ty)) in storage_fields.iter().enumerate() {
        storage_slots.insert(ident.clone().unwrap(), slot_id);
    }

    // one `build_*` fn per external fn, lowered from its body
    let build_fns: Vec<proc_macro2::TokenStream> = external_functions
        .iter()
        .map(|(name, output, block, inputs)| {
            let build_ident = format_ident!("build_{}", name);
            let offsets = param_offset(params_of(inputs.clone()));
            let body = lower_block(block, output, &storage_slots, &offsets, &events);
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

    // re-emit the user's items as real Rust, minus the helper attributes
    let real_items: Vec<proc_macro2::TokenStream> = items.iter().map(strip_helper_attrs).collect();

    let mod_name = &module.ident;

    let expanded = quote! {
        #[allow(dead_code)]
        mod #mod_name {
            #(#real_items)*

            pub fn deploy_code() -> ::std::vec::Vec<u8> {
                let builder = ::rulidity::contract::Builder;
                let funcs = ::std::vec![#(#function_inits),*];
                builder.assemble_contract(funcs)
            }

            #(#build_fns)*
        }
    };

    // eprintln!("{}", expanded);
    expanded.into()
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

/// Clone an item and remove the `#[storage]` / `#[external]` helper attributes,
/// so it re-emits as ordinary (type-checkable) Rust.
fn strip_helper_attrs(item: &Item) -> proc_macro2::TokenStream {
    let mut item = item.clone();
    match &mut item {
        Item::Struct(s) => {
            s.attrs
                .retain(|a| !a.path().is_ident("storage") && !a.path().is_ident("event"));
            // `#[indexed]` lives on the event's fields, strip those too
            for field in &mut s.fields {
                field.attrs.retain(|a| !a.path().is_ident("indexed"));
            }
        }
        Item::Impl(imp) => {
            for impl_item in &mut imp.items {
                if let ImplItem::Fn(f) = impl_item {
                    f.attrs.retain(|a| !a.path().is_ident("external"));
                }
            }
        }
        _ => {}
    }
    quote! { #item }
}

fn lower_block(
    block: &syn::Block,
    output: &syn::ReturnType,
    slots: &HashMap<syn::Ident, usize>,
    param_offsets: &HashMap<String, u32>,
    events: &HashMap<String, EventDef>,
) -> proc_macro2::TokenStream {
    let has_return = matches!(output, syn::ReturnType::Type(..));

    let mut locals: HashMap<String, u32> = HashMap::new();
    let mut next_local: u32 = 0x80; // above 0x00-0x40 scratchpad

    let body = lower_stmts(
        &block.stmts,
        slots,
        param_offsets,
        &mut locals,
        &mut next_local,
        events,
        has_return,
    );

    // fn with no return must halt, else it falls into the next body
    let halt = if !has_return {
        quote! { asm.add_op(::rulidity::asm::Op::Stop); }
    } else {
        quote! {}
    };

    quote! { #body #halt }
}

/// Lower a slice of statements, threading the shared locals state so nested
/// blocks (`if` bodies) allocate fresh memory slots and stay
/// visible. Never emits the trailing STOP, that's done in `lower_block`.
/// @todo this is a horribly long and convoluted function. It might be a good idea to spread the code into function
fn lower_stmts(
    stmts: &[syn::Stmt],
    slots: &HashMap<syn::Ident, usize>,
    param_offsets: &HashMap<String, u32>,
    locals: &mut HashMap<String, u32>,
    next_local: &mut u32,
    events: &HashMap<String, EventDef>,
    has_return: bool,
) -> proc_macro2::TokenStream {
    let n = stmts.len();
    let mut parts: Vec<proc_macro2::TokenStream> = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        let is_last = i + 1 == n;

        match stmt {
            syn::Stmt::Expr(expr, semi) => match expr {
                // self.field = rhs;
                syn::Expr::Assign(assign) => match &*assign.left {
                    syn::Expr::Field(f) if is_self(&f.base) => {
                        let slot = member_slot(&f.member, slots);
                        let rhs = lower_expression(&assign.right, slots, param_offsets, locals);
                        parts.push(quote! {
                            #rhs
                            asm.store_slot(::rulidity::U256::from(#slot));
                        });
                    }
                    _ => parts.push(
                        syn::Error::new_spanned(
                            &assign.left,
                            "rulidity: unsupported assignment target",
                        )
                        .to_compile_error(),
                    ),
                },
                // self.map.insert(key, value);
                syn::Expr::MethodCall(mc) if mc.method == "insert" => {
                    let base = self_field_slot(&mc.receiver, slots);
                    let args: Vec<&syn::Expr> = mc.args.iter().collect();
                    if args.len() != 2 {
                        parts.push(
                            syn::Error::new_spanned(mc, "rulidity: insert takes (key, value)")
                                .to_compile_error(),
                        );
                    } else {
                        let value = lower_expression(args[1], slots, param_offsets, locals); // value first (stays underneath)
                        let key = lower_expression(args[0], slots, param_offsets, locals); // then key (mapping_slot eats it)
                        parts.push(quote! {
                            #value
                            #key
                            asm.mapping_slot(::rulidity::U256::from(#base));
                            asm.sstore();
                        });
                    }
                }
                syn::Expr::Call(call) if is_path(&call.func, "require") => {
                    // require calls
                    let cond = lower_expression(&call.args[0], slots, param_offsets, locals);
                    parts.push(quote! {
                        #cond
                        {
                            let lab = asm.fresh_label();
                            asm.add_op(::rulidity::asm::Op::JumpI(lab));
                            asm.revert_empty();
                            asm.add_op(::rulidity::asm::Op::JumpDest(lab));
                        }
                    });
                }
                syn::Expr::Call(call) if is_path(&call.func, "emit") => {
                    // emit calls
                    let expr_struct = match &call.args[0] {
                        syn::Expr::Struct(s) => s,
                        _ => {
                            parts.push(
                                syn::Error::new_spanned(call, "Invalid emit argument")
                                    .to_compile_error(),
                            );
                            continue;
                        }
                    };
                    let struct_name = expr_struct.path.segments.last().unwrap().ident.to_string();
                    let def = match events.get(&struct_name) {
                        Some(d) => d,
                        None => {
                            parts.push(
                                syn::Error::new_spanned(call, "Unknown event").to_compile_error(),
                            );
                            continue;
                        }
                    };

                    // reorder the literal's fields back to declaration order, lower each value
                    let values: Vec<proc_macro2::TokenStream> = def
                        .fields
                        .iter()
                        .map(|f| {
                            let fv = expr_struct.fields.iter().find(|fv| match &fv.member {
                                syn::Member::Named(id) => *id == f.name,
                                syn::Member::Unnamed(_) => false,
                            });
                            match fv {
                                Some(fv) => {
                                    lower_expression(&fv.expr, slots, param_offsets, locals)
                                }
                                None => syn::Error::new_spanned(
                                    expr_struct,
                                    format!("rulidity: missing event field `{}`", f.name),
                                )
                                .to_compile_error(),
                            }
                        })
                        .collect();

                    // canonical signature over all fields, in declaration order
                    let params: Vec<(syn::Ident, syn::Type)> = def
                        .fields
                        .iter()
                        .map(|f| (format_ident!("{}", f.name), f.ty.clone()))
                        .collect();
                    let sig = signature_string(def.name.clone(), params);

                    let k = def.fields.len();
                    let data_len = (k as u64) * 32;
                    // top-of-stack is the LAST field, so store the highest offset first (j counts down).
                    let stores: Vec<proc_macro2::TokenStream> = (0..k)
                        .rev()
                        .map(|j| {
                            let off = (j as u64) * 32;
                            quote! {
                                asm.push_word(::rulidity::U256::from(#off));
                                asm.mstore();
                            }
                        })
                        .collect();

                    parts.push(quote! {
                        #(#values)*
                        #(#stores)*
                        asm.push_topic(#sig);
                        asm.push_word(::rulidity::U256::from(#data_len));
                        asm.push_word(::rulidity::U256::from(0u64));
                        asm.add_op(::rulidity::asm::Op::Log(1));
                    });
                }
                syn::Expr::If(if_expr) => {
                    let cond = lower_expression(&if_expr.cond, slots, param_offsets, locals);
                    let then_body = lower_stmts(
                        &if_expr.then_branch.stmts,
                        slots,
                        param_offsets,
                        locals,
                        next_local,
                        events,
                        false,
                    );

                    match &if_expr.else_branch {
                        Some((_, else_expr)) => {
                            let else_stmts = match &**else_expr {
                                syn::Expr::Block(b) => &b.block.stmts,
                                _ => {
                                    parts.push(
                                        syn::Error::new_spanned(
                                            else_expr,
                                            "rulidity: else if not supported yet",
                                        )
                                        .to_compile_error(),
                                    );
                                    continue;
                                }
                            };

                            let else_body = lower_stmts(
                                else_stmts,
                                slots,
                                param_offsets,
                                locals,
                                next_local,
                                events,
                                false,
                            );
                            parts.push(quote! {
                                #cond
                                {
                                    let else_l = asm.fresh_label();
                                    let end    = asm.fresh_label();
                                    asm.add_op(::rulidity::asm::Op::IsZero);
                                    asm.add_op(::rulidity::asm::Op::JumpI(else_l));
                                    #then_body
                                    asm.add_op(::rulidity::asm::Op::Jump(end));
                                    asm.add_op(::rulidity::asm::Op::JumpDest(else_l));
                                    #else_body
                                    asm.add_op(::rulidity::asm::Op::JumpDest(end));
                                }
                            });
                        }
                        None => {
                            parts.push(quote! {
                                #cond
                                {
                                    let end = asm.fresh_label();
                                    asm.add_op(::rulidity::asm::Op::IsZero);
                                    asm.add_op(::rulidity::asm::Op::JumpI(end));
                                    #then_body
                                    asm.add_op(::rulidity::asm::Op::JumpDest(end));
                                }
                            });
                        }
                    }
                }
                // trailing expression is the return value
                _ if is_last && has_return && semi.is_none() => {
                    let e = lower_expression(expr, slots, param_offsets, locals);
                    parts.push(quote! {
                        #e
                        asm.return_word();
                    });
                }
                _ => parts.push(
                    syn::Error::new_spanned(expr, "rulidity: unsupported statement")
                        .to_compile_error(),
                ),
            },
            // let x = expression
            syn::Stmt::Local(local) => {
                let name = ident_of_pat(local.pat.clone());
                let init = if let Some(i) = &local.init {
                    i.expr.clone()
                } else {
                    return syn::Error::new_spanned(local, "Unsupported local expression")
                        .to_compile_error();
                };

                let value = lower_expression(&init, slots, param_offsets, locals);
                let offset = *next_local;
                *next_local += 0x20;
                locals.insert(name.to_string(), offset);

                parts.push(quote! {
                    #value
                    asm.push_word(::rulidity::U256::from(#offset));
                    asm.mstore();
                });
            }
            other => parts.push(
                syn::Error::new_spanned(other, "rulidity: unsupported statement")
                    .to_compile_error(),
            ),
        }
    }

    quote! { #(#parts)* }
}

fn lower_expression(
    expr: &syn::Expr,
    slots: &HashMap<syn::Ident, usize>,
    param_offsets: &HashMap<String, u32>,
    locals: &HashMap<String, u32>,
) -> proc_macro2::TokenStream {
    match expr {
        // integer literal
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        }) => {
            let v: u64 = int.base10_parse().unwrap();
            quote! { asm.push_word(::rulidity::U256::from(#v)); }
        }
        // self.field loads that field's storage slot
        syn::Expr::Field(field) if is_self(&field.base) => {
            let slot = member_slot(&field.member, slots);
            quote! { asm.load_slot(::rulidity::U256::from(#slot)); }
        }
        // self.map.get(key)
        syn::Expr::MethodCall(mc) if mc.method == "get" => {
            let base = self_field_slot(&mc.receiver, slots);
            let args: Vec<&syn::Expr> = mc.args.iter().collect();
            if args.len() != 1 {
                return syn::Error::new_spanned(mc, "rulidity: get takes (key)").to_compile_error();
            }
            let key = lower_expression(args[0], slots, param_offsets, locals);
            quote! {
                #key
                asm.mapping_slot(::rulidity::U256::from(#base));
                asm.sload();
            }
        }
        // arithmetic and comparison operators
        syn::Expr::Binary(bin) => {
            let l = lower_expression(&bin.left, slots, param_offsets, locals);
            let r = lower_expression(&bin.right, slots, param_offsets, locals);
            match &bin.op {
                syn::BinOp::Add(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Add); },
                syn::BinOp::Mul(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Mul); },
                // SUB is not commutative. Making the r go before l makes the sub do left - right
                syn::BinOp::Sub(_) => quote! {
                    #r #l
                    asm.add_op(::rulidity::asm::Op::Sub);
                },
                syn::BinOp::Lt(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Gt); },
                syn::BinOp::Gt(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Lt); },
                syn::BinOp::Eq(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Eq); },
                syn::BinOp::Ne(_) => quote! {
                    #l #r
                    asm.add_op(::rulidity::asm::Op::Eq);
                    asm.add_op(::rulidity::asm::Op::IsZero);
                },
                syn::BinOp::Le(_) => quote! {
                    #l #r
                    asm.add_op(::rulidity::asm::Op::Lt);
                    asm.add_op(::rulidity::asm::Op::IsZero);
                },
                syn::BinOp::Ge(_) => quote! {
                    #l #r
                    asm.add_op(::rulidity::asm::Op::Gt);
                    asm.add_op(::rulidity::asm::Op::IsZero);
                },
                _ => syn::Error::new_spanned(expr, "rulidity: unsupported operator")
                    .to_compile_error(),
            }
        }
        // (expr)
        syn::Expr::Paren(p) => lower_expression(&p.expr, slots, param_offsets, locals),
        // msg_sender()  or  U256::from(<int>)
        syn::Expr::Call(call) => {
            if let syn::Expr::Path(p) = &*call.func
                && p.path.is_ident("msg_sender")
            {
                return quote! { asm.msg_sender(); };
            }
            lower_from_call(call)
        }
        syn::Expr::Path(p) if p.path.get_ident().is_some() => {
            let id = p.path.get_ident().unwrap();
            let name = id.to_string();

            if let Some(offset) = locals.get(&name) {
                return quote! {
                    asm.push_word(::rulidity::U256::from(#offset));
                    asm.mload();
                };
            }

            if let Some(offset) = param_offsets.get(&name) {
                let offset = *offset;
                return quote! {
                    asm.push_word(::rulidity::U256::from(#offset));
                    asm.add_op(::rulidity::asm::Op::CallDataLoad);
                };
            }

            syn::Error::new_spanned(expr, format!("unknown identifier {name}")).to_compile_error()
        }
        _ => syn::Error::new_spanned(expr, "rulidity: unsupported expression").to_compile_error(),
    }
}

fn is_path(e: &syn::Expr, name: &str) -> bool {
    matches!(e, syn::Expr::Path(p) if p.path.is_ident(name))
}

fn is_self(e: &syn::Expr) -> bool {
    is_path(e, "self")
}

fn member_slot(m: &syn::Member, slots: &HashMap<syn::Ident, usize>) -> u64 {
    match m {
        syn::Member::Named(ident) => *slots.get(ident).expect("unknown storage field") as u64,
        syn::Member::Unnamed(_) => panic!("tuple fields not supported"),
    }
}

fn self_field_slot(left: &syn::Expr, slots: &HashMap<syn::Ident, usize>) -> u64 {
    match left {
        syn::Expr::Field(f) if is_self(&f.base) => member_slot(&f.member, slots),
        _ => panic!("expected self.<field>"),
    }
}

fn ident_of_pat(pat: syn::Pat) -> syn::Ident {
    match pat {
        Pat::Ident(pat_ident) => pat_ident.ident,
        Pat::Type(pat_type) => ident_of_pat(*pat_type.pat),
        _ => panic!("Unsupported let pattern"),
    }
}

// handle U256::from(1) (or any ...::from(<int>))
fn lower_from_call(call: &syn::ExprCall) -> proc_macro2::TokenStream {
    if let syn::Expr::Path(p) = &*call.func {
        let is_from = p.path.segments.last().is_some_and(|s| s.ident == "from");
        if is_from
            && call.args.len() == 1
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) = &call.args[0]
        {
            let v: u64 = int.base10_parse().unwrap();
            return quote! { asm.push_word(::rulidity::U256::from(#v)); };
        }
    }
    syn::Error::new_spanned(call, "rulidity: unsupported call").to_compile_error()
}

fn params_of(inputs: syn::punctuated::Punctuated<FnArg, Comma>) -> Vec<(syn::Ident, syn::Type)> {
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
fn abi_type_of(ty: syn::Type) -> String {
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
        _ => panic!("Unsupported parameter type {segment_str}"), // @todo add more Solidity types
    };

    ret.to_owned()
}

/// calculates the signature of whatever is given
/// used to get the function signatures, so fn balance_of(&self, who: Address) -> U256 becomes balance_of(Address)
/// and event signatures, so struct Event { who: Address, amount: U256 } becomes Event(Address,uint256)
fn signature_string(name: String, params: Vec<(syn::Ident, syn::Type)>) -> String {
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

/// calculates calldata offset of function arguments. Currently only simple params are supported
/// so each param takes 32 bytes
fn param_offset(params: Vec<(syn::Ident, syn::Type)>) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for (i, (ident, _)) in params.iter().enumerate() {
        let ident_str = ident.to_string();
        map.insert(ident_str, 4 + 32 * i as u32);
    }

    map
}
