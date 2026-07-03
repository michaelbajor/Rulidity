use std::collections::HashMap;

use quote::{format_ident, quote};

use crate::abi::signature_string;
use crate::model::{Ctx, FieldKind, Lower, StorageField};

pub(crate) fn lower_block<'a>(
    block: &syn::Block,
    output: &syn::ReturnType,
    ctx: &'a Ctx<'a>,
    param_offsets: HashMap<String, u32>,
) -> proc_macro2::TokenStream {
    let has_return = matches!(output, syn::ReturnType::Type(..));

    let mut state = Lower {
        ctx,
        locals: HashMap::new(),
        param_offsets,
        next_local: 0x80, // above 0x00-0x40 scratchpad
    };

    let body = lower_stmts(&block.stmts, &mut state, has_return);

    // fn with no return must halt, else it falls into the next body
    let halt = if !has_return {
        quote! { asm.add_op(::rulidity::asm::Op::Stop); }
    } else {
        quote! {}
    };

    quote! { #body #halt }
}

/// Lower a slice of statements. The shared `Lower` state threads locals across
/// nested blocks (`if` bodies) so they allocate fresh slots and stay visible.
/// Never emits the trailing STOP, that's `lower_block`'s job.
fn lower_stmts(
    stmts: &[syn::Stmt],
    state: &mut Lower,
    has_return: bool,
) -> proc_macro2::TokenStream {
    let n = stmts.len();
    let mut parts: Vec<proc_macro2::TokenStream> = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        let is_last = i + 1 == n;
        let part = match stmt {
            syn::Stmt::Expr(expr, semi) => {
                let is_return = is_last && has_return && semi.is_none();
                lower_expr_stmt(expr, is_return, state)
            }
            syn::Stmt::Local(local) => lower_local(local, state),
            other => {
                syn::Error::new_spanned(other, "rulidity: unsupported statement").to_compile_error()
            }
        };
        parts.push(part);
    }

    quote! { #(#parts)* }
}

/// Dispatch a single expression statement to the right lowering. A trailing
/// expression with no semicolon in a fn that returns is the return value.
fn lower_expr_stmt(
    expr: &syn::Expr,
    is_return: bool,
    state: &mut Lower,
) -> proc_macro2::TokenStream {
    match expr {
        syn::Expr::Assign(assign) => lower_assign(assign, state),
        syn::Expr::MethodCall(mc) if mc.method == "insert" => lower_mapping_insert(mc, state),
        syn::Expr::MethodCall(mc) if mc.method == "set" => lower_array_set(mc, state),
        syn::Expr::MethodCall(mc) if mc.method == "push" => lower_array_push(mc, state),
        syn::Expr::Call(call) if is_path(&call.func, "require") => lower_require(call, state),
        syn::Expr::Call(call) if is_path(&call.func, "emit") => lower_emit(call, state),
        syn::Expr::If(if_expr) => lower_if(if_expr, state),
        _ if is_return => {
            let e = lower_expression(expr, state);
            quote! {
                #e
                asm.return_word();
            }
        }
        _ => syn::Error::new_spanned(expr, "rulidity: unsupported statement").to_compile_error(),
    }
}

/// self.field = rhs;
fn lower_assign(assign: &syn::ExprAssign, state: &mut Lower) -> proc_macro2::TokenStream {
    match &*assign.left {
        syn::Expr::Field(f) if is_self(&f.base) => {
            let slot = member_slot(&f.member, state.ctx.storage);
            let rhs = lower_expression(&assign.right, state);
            quote! {
                #rhs
                asm.store_slot(::rulidity::U256::from(#slot));
            }
        }
        _ => syn::Error::new_spanned(&assign.left, "rulidity: unsupported assignment target")
            .to_compile_error(),
    }
}

/// self.map.insert(key, value);
fn lower_mapping_insert(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let base = self_field_slot(&mc.receiver, state.ctx.storage);
    let args: Vec<&syn::Expr> = mc.args.iter().collect();
    if args.len() != 2 {
        return syn::Error::new_spanned(mc, "rulidity: insert takes (key, value)")
            .to_compile_error();
    }
    let value = lower_expression(args[1], state); // value first (stays underneath)
    let key = lower_expression(args[0], state); // then key (mapping_slot eats it)
    quote! {
        #value
        #key
        asm.mapping_slot(::rulidity::U256::from(#base));
        asm.sstore();
    }
}

/// self.array.set(idx, val);
fn lower_array_set(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let sf = *self_field(&mc.receiver, state.ctx.storage);
    let base = sf.slot as u64;
    if sf.kind != FieldKind::Array {
        return syn::Error::new_spanned(mc, "rulidity: .set() is only valid on an Array field")
            .to_compile_error();
    }
    let args: Vec<&syn::Expr> = mc.args.iter().collect();
    if args.len() != 2 {
        return syn::Error::new_spanned(mc, "rulidity: set takes (index, value)")
            .to_compile_error();
    }
    let value = lower_expression(args[1], state);
    let index = lower_expression(args[0], state);
    quote! {
        #value
        #index
        asm.array_elem_slot(::rulidity::U256::from(#base));
        asm.sstore();
    }
}

/// self.array.push(val);
fn lower_array_push(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let sf = *self_field(&mc.receiver, state.ctx.storage);
    let base = sf.slot as u64;
    if sf.kind != FieldKind::Array {
        return syn::Error::new_spanned(mc, "rulidity: .push() is only valid on an Array field")
            .to_compile_error();
    }
    let args: Vec<&syn::Expr> = mc.args.iter().collect();
    if args.len() != 1 {
        return syn::Error::new_spanned(mc, "rulidity: push takes (value)").to_compile_error();
    }
    let value = lower_expression(args[0], state);
    quote! {
        #value
        asm.load_slot(::rulidity::U256::from(#base));
        asm.array_elem_slot(::rulidity::U256::from(#base));
        asm.sstore();
        asm.load_slot(::rulidity::U256::from(#base));
        asm.push_word(::rulidity::U256::from(1u64));
        asm.add_op(::rulidity::asm::Op::Add);
        asm.store_slot(::rulidity::U256::from(#base));
    }
}

/// require(cond);  reverts when cond is false
fn lower_require(call: &syn::ExprCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let cond = lower_expression(&call.args[0], state);
    quote! {
        #cond
        {
            let lab = asm.fresh_label();
            asm.add_op(::rulidity::asm::Op::JumpI(lab));
            asm.revert_empty();
            asm.add_op(::rulidity::asm::Op::JumpDest(lab));
        }
    }
}

/// emit(Event { field: expr, .. });  stage 1: all fields to data, LOG1
fn lower_emit(call: &syn::ExprCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let expr_struct = match &call.args[0] {
        syn::Expr::Struct(s) => s,
        _ => return syn::Error::new_spanned(call, "Invalid emit argument").to_compile_error(),
    };
    let struct_name = expr_struct.path.segments.last().unwrap().ident.to_string();
    let def = match state.ctx.events.get(&struct_name) {
        Some(d) => d,
        None => return syn::Error::new_spanned(call, "Unknown event").to_compile_error(),
    };

    // reorder the literal's fields back to declaration order, lower each value
    let mut values: Vec<proc_macro2::TokenStream> = Vec::new();
    for f in &def.fields {
        let fv = expr_struct.fields.iter().find(|fv| match &fv.member {
            syn::Member::Named(id) => *id == f.name,
            syn::Member::Unnamed(_) => false,
        });
        match fv {
            Some(fv) => values.push(lower_expression(&fv.expr, state)),
            None => values.push(
                syn::Error::new_spanned(
                    expr_struct,
                    format!("rulidity: missing event field `{}`", f.name),
                )
                .to_compile_error(),
            ),
        }
    }

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

    quote! {
        #(#values)*
        #(#stores)*
        asm.push_topic(#sig);
        asm.push_word(::rulidity::U256::from(#data_len));
        asm.push_word(::rulidity::U256::from(0u64));
        asm.add_op(::rulidity::asm::Op::Log(1));
    }
}

/// if cond { .. } and if cond { .. } else { .. }
fn lower_if(if_expr: &syn::ExprIf, state: &mut Lower) -> proc_macro2::TokenStream {
    let cond = lower_expression(&if_expr.cond, state);
    let then_body = lower_stmts(&if_expr.then_branch.stmts, state, false);

    match &if_expr.else_branch {
        Some((_, else_expr)) => {
            let else_stmts = match &**else_expr {
                syn::Expr::Block(b) => &b.block.stmts,
                _ => {
                    return syn::Error::new_spanned(
                        else_expr,
                        "rulidity: else if not supported yet",
                    )
                    .to_compile_error();
                }
            };

            let else_body = lower_stmts(else_stmts, state, false);
            quote! {
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
            }
        }
        None => quote! {
            #cond
            {
                let end = asm.fresh_label();
                asm.add_op(::rulidity::asm::Op::IsZero);
                asm.add_op(::rulidity::asm::Op::JumpI(end));
                #then_body
                asm.add_op(::rulidity::asm::Op::JumpDest(end));
            }
        },
    }
}

/// let x = expr;  stores the value in a fresh local slot
fn lower_local(local: &syn::Local, state: &mut Lower) -> proc_macro2::TokenStream {
    let name = ident_of_pat(local.pat.clone());
    let init = match &local.init {
        Some(i) => i.expr.clone(),
        None => {
            return syn::Error::new_spanned(local, "Unsupported local expression")
                .to_compile_error();
        }
    };

    let value = lower_expression(&init, state);
    let offset = state.alloc_local();
    state.locals.insert(name.to_string(), offset);

    quote! {
        #value
        asm.push_word(::rulidity::U256::from(#offset));
        asm.mstore();
    }
}

fn lower_expression(expr: &syn::Expr, state: &Lower) -> proc_macro2::TokenStream {
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
            let slot = member_slot(&field.member, state.ctx.storage);
            quote! { asm.load_slot(::rulidity::U256::from(#slot)); }
        }
        // self.map.get(key) or self.array.get(idx)
        syn::Expr::MethodCall(mc) if mc.method == "get" => lower_get(mc, state),
        // self.array.len()
        syn::Expr::MethodCall(mc) if mc.method == "len" => lower_len(mc, state),
        // arithmetic and comparison operators
        syn::Expr::Binary(bin) => lower_binary(bin, state),
        // (expr)
        syn::Expr::Paren(p) => lower_expression(&p.expr, state),
        // msg_sender()  or  U256::from(<int>)
        syn::Expr::Call(call) => {
            if let syn::Expr::Path(p) = &*call.func
                && p.path.is_ident("msg_sender")
            {
                return quote! { asm.msg_sender(); };
            }
            lower_from_call(call)
        }
        syn::Expr::Path(p) if p.path.get_ident().is_some() => lower_ident(p, state),
        _ => syn::Error::new_spanned(expr, "rulidity: unsupported expression").to_compile_error(),
    }
}

/// self.map.get(key) or self.array.get(idx)
fn lower_get(mc: &syn::ExprMethodCall, state: &Lower) -> proc_macro2::TokenStream {
    let sf = *self_field(&mc.receiver, state.ctx.storage);
    let base = sf.slot as u64;
    let args: Vec<&syn::Expr> = mc.args.iter().collect();
    if args.len() != 1 {
        return syn::Error::new_spanned(mc, "rulidity: get takes one argument").to_compile_error();
    }
    let arg = lower_expression(args[0], state);
    match sf.kind {
        FieldKind::Mapping => quote! {
            #arg
            asm.mapping_slot(::rulidity::U256::from(#base));
            asm.sload();
        },
        FieldKind::Array => quote! {
            #arg
            asm.array_elem_slot(::rulidity::U256::from(#base));
            asm.sload();
        },
        FieldKind::Scalar => syn::Error::new_spanned(
            mc,
            "rulidity: .get() is only valid on a Mapping or Array field",
        )
        .to_compile_error(),
    }
}

/// self.array.len()
fn lower_len(mc: &syn::ExprMethodCall, state: &Lower) -> proc_macro2::TokenStream {
    let sf = *self_field(&mc.receiver, state.ctx.storage);
    let base = sf.slot as u64;
    if sf.kind != FieldKind::Array {
        return syn::Error::new_spanned(mc, "rulidity: .len() is only valid on an Array field")
            .to_compile_error();
    }
    quote! {
        asm.load_slot(::rulidity::U256::from(#base));
    }
}

/// arithmetic and comparison operators. Comparisons use the MIRRORED opcode to
/// avoid a SWAP (after `#l #r`, top-of-stack is the right operand).
fn lower_binary(bin: &syn::ExprBinary, state: &Lower) -> proc_macro2::TokenStream {
    let l = lower_expression(&bin.left, state);
    let r = lower_expression(&bin.right, state);
    match &bin.op {
        syn::BinOp::Add(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Add); },
        syn::BinOp::Mul(_) => quote! { #l #r asm.add_op(::rulidity::asm::Op::Mul); },
        // SUB is not commutative. Making r go before l makes the sub do left - right
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
        _ => syn::Error::new_spanned(bin, "rulidity: unsupported operator").to_compile_error(),
    }
}

/// a bare identifier: a local (MLOAD) shadows a param (CALLDATALOAD)
fn lower_ident(p: &syn::ExprPath, state: &Lower) -> proc_macro2::TokenStream {
    let id = p.path.get_ident().unwrap();
    let name = id.to_string();

    if let Some(offset) = state.locals.get(&name) {
        let offset = *offset;
        return quote! {
            asm.push_word(::rulidity::U256::from(#offset));
            asm.mload();
        };
    }

    if let Some(offset) = state.param_offsets.get(&name) {
        let offset = *offset;
        return quote! {
            asm.push_word(::rulidity::U256::from(#offset));
            asm.add_op(::rulidity::asm::Op::CallDataLoad);
        };
    }

    syn::Error::new_spanned(p, format!("unknown identifier {name}")).to_compile_error()
}

fn is_path(e: &syn::Expr, name: &str) -> bool {
    matches!(e, syn::Expr::Path(p) if p.path.is_ident(name))
}

fn is_self(e: &syn::Expr) -> bool {
    is_path(e, "self")
}

fn member_slot(m: &syn::Member, storage: &HashMap<syn::Ident, StorageField>) -> u64 {
    match m {
        syn::Member::Named(ident) => storage.get(ident).expect("unknown storage field").slot as u64,
        syn::Member::Unnamed(_) => panic!("tuple fields not supported"),
    }
}

fn self_field<'a>(
    receiver: &syn::Expr,
    storage: &'a HashMap<syn::Ident, StorageField>,
) -> &'a StorageField {
    match receiver {
        syn::Expr::Field(f) if is_self(&f.base) => match &f.member {
            syn::Member::Named(id) => storage.get(id).expect("unknown storage field"),
            syn::Member::Unnamed(_) => panic!("tuple fields not supported"),
        },
        _ => panic!("expected self.<field>"),
    }
}

fn self_field_slot(left: &syn::Expr, storage: &HashMap<syn::Ident, StorageField>) -> u64 {
    match left {
        syn::Expr::Field(f) if is_self(&f.base) => member_slot(&f.member, storage),
        _ => panic!("expected self.<field>"),
    }
}

fn ident_of_pat(pat: syn::Pat) -> syn::Ident {
    match pat {
        syn::Pat::Ident(pat_ident) => pat_ident.ident,
        syn::Pat::Type(pat_type) => ident_of_pat(*pat_type.pat),
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
