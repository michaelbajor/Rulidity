use std::collections::HashMap;

use quote::{format_ident, quote};

use crate::abi::signature_string;
use crate::model::{Ctx, FieldKind, Lower, StorageField};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    Return,
    Leave,
    Void,
}

fn returns(output: &syn::ReturnType) -> bool {
    matches!(output, syn::ReturnType::Type(..))
}

fn returns_string(output: &syn::ReturnType) -> bool {
    if let syn::ReturnType::Type(_, ty) = output
        && let syn::Type::Path(tp) = &**ty
        && let Some(seg) = tp.path.segments.last()
    {
        return seg.ident == "ShortString";
    }

    false
}

fn is_short_string_ty(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "ShortString"))
}

pub(crate) fn lower_block<'a>(
    block: &syn::Block,
    output: &syn::ReturnType,
    ctx: &'a Ctx<'a>,
    params: Vec<(syn::Ident, syn::Type)>,
) -> proc_macro2::TokenStream {
    let has_return = matches!(output, syn::ReturnType::Type(..));

    let mut state = Lower {
        ctx,
        locals: HashMap::new(),
        param_offsets: HashMap::new(),
        next_local: 0x80, // above 0x00-0x40 scratchpad
        call_stack: Vec::new(),
        ret_string: returns_string(output),
    };

    // each param owns one head word at 4 + 32 * i
    // word aprams read directly from calldata (offset in param_offset)
    // string params decode the offset/len/data at entry into a local variable
    let mut prologue: Vec<proc_macro2::TokenStream> = Vec::new();
    for (i, (name, ty)) in params.iter().enumerate() {
        let head = 4 + 0x20 * i as u32;
        if is_short_string_ty(ty) {
            let slot = state.alloc_local();
            state.locals.insert(name.to_string(), slot);
            prologue.push(quote! { asm.decode_short_string_param(#head, #slot); });
        } else {
            state.param_offsets.insert(name.to_string(), head);
        }
    }

    let tail = if has_return { Tail::Return } else { Tail::Void };
    let body = lower_stmts(&block.stmts, &mut state, tail);

    // fn with no return must halt, else it falls into the next body
    let halt = if !has_return {
        quote! { asm.add_op(::rulidity::asm::Op::Stop); }
    } else {
        quote! {}
    };

    quote! { #(#prologue)* #body #halt }
}

/// Lower a constructor body. Args are appended after a creation code at deployment time.
/// Each arg is CODECOPY'd from the code tail into a local slot and then used as local
/// Runs in place at offset 0 and can't halt.
pub(crate) fn lower_constructor<'a>(
    block: &syn::Block,
    params: &[(syn::Ident, syn::Type)],
    ctx: &'a Ctx<'a>,
) -> proc_macro2::TokenStream {
    let mut state = Lower {
        ctx,
        locals: HashMap::new(),
        param_offsets: HashMap::new(),
        next_local: 0x80,
        call_stack: Vec::new(),
        ret_string: false,
    };

    let n = params.len() as u64;
    let s = params
        .iter()
        .filter(|(_, ty)| is_short_string_ty(ty))
        .count() as u64;
    let total_len = 32 * n + 64 * s;

    let mut k = 0u64;
    let mut prologue: Vec<proc_macro2::TokenStream> = Vec::new();

    for (i, (name, ty)) in params.iter().enumerate() {
        let slot = state.alloc_local();
        state.locals.insert(name.to_string(), slot);

        if is_short_string_ty(ty) {
            // tail is the last 64 * s bytes, the Kth string's [len][data] live there
            let len_back = (64 * (s - k)) as u32;
            let data_back = len_back - 32;
            prologue.push(
                quote! {asm.decode_short_string_constructor_arg(#len_back, #data_back, #slot);},
            );
            k += 1;
        } else {
            // arg i lives at args_start + 32 * i = CODESIZE - (total_len - 32 * i)
            let back = total_len - 32 * i as u64;
            prologue.push(quote! {
                asm.push_word(::rulidity::U256::from(32u64)); // length
                asm.push_word(::rulidity::U256::from(#back)); // 32 * (n - 1)
                asm.code_size(); // CODESIZE
                asm.sub();  // CODESIZE - 32 * (n - 1)
                asm.push_word(::rulidity::U256::from(#slot)); // dest (local slot)
                asm.code_copy();
            });
        }
    }

    let body = lower_stmts(&block.stmts, &mut state, Tail::Void);

    quote! {
        #(#prologue)*
        #body
    }
}

/// Lower a slice of statements. The shared `Lower` state threads locals across
/// nested blocks (`if` bodies) so they allocate fresh slots and stay visible.
/// Never emits the trailing STOP, that's `lower_block`'s job.
fn lower_stmts(stmts: &[syn::Stmt], state: &mut Lower, tail: Tail) -> proc_macro2::TokenStream {
    let n = stmts.len();
    let mut parts: Vec<proc_macro2::TokenStream> = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        let is_last = i + 1 == n;
        let part = match stmt {
            syn::Stmt::Expr(expr, semi) => {
                let stmt_tail = if is_last && semi.is_none() {
                    tail
                } else {
                    Tail::Void
                };
                lower_expr_stmt(expr, stmt_tail, state)
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
fn lower_expr_stmt(expr: &syn::Expr, tail: Tail, state: &mut Lower) -> proc_macro2::TokenStream {
    match expr {
        syn::Expr::Assign(assign) => return lower_assign(assign, state),
        syn::Expr::MethodCall(mc) if mc.method == "insert" => {
            return lower_mapping_insert(mc, state);
        }
        syn::Expr::MethodCall(mc) if mc.method == "set" => return lower_array_set(mc, state),
        syn::Expr::MethodCall(mc) if mc.method == "push" => return lower_array_push(mc, state),
        syn::Expr::Call(call) if is_path(&call.func, "require") => {
            return lower_require(call, state);
        }
        syn::Expr::Call(call) if is_path(&call.func, "emit") => return lower_emit(call, state),
        syn::Expr::If(if_expr) => return lower_if(if_expr, state),
        syn::Expr::MethodCall(mc)
            if matches!(tail, Tail::Void)
                && is_self(&mc.receiver)
                && state
                    .ctx
                    .internal_functions
                    .contains_key(&mc.method.to_string()) =>
        {
            return lower_internal_call(mc, false, state);
        }
        _ => {}
    }

    // otherwise it's a value expression in tail position
    match tail {
        Tail::Return => {
            let e = lower_expression(expr, state);
            let encode = if state.ret_string {
                quote! {asm.return_short_string();}
            } else {
                quote! {asm.return_word();}
            };
            quote! {
                #e
                #encode
            }
        }
        Tail::Leave => lower_expression(expr, state),
        Tail::Void => {
            syn::Error::new_spanned(expr, "Rulidity: unsupported statement").to_compile_error()
        }
    }
}

fn lower_internal_call(
    mc: &syn::ExprMethodCall,
    want_value: bool,
    state: &mut Lower,
) -> proc_macro2::TokenStream {
    let name = mc.method.to_string();

    if state.call_stack.contains(&name) {
        return syn::Error::new_spanned(mc, "Rulidity: recursive internal calls are not supported")
            .to_compile_error();
    }

    let (params, block, output) = {
        let f = state
            .ctx
            .internal_functions
            .get(&name)
            .expect("Shouldn't fail, guarded by caller");
        (f.params.clone(), f.block.clone(), f.output.clone())
    };

    if mc.args.len() != params.len() {
        return syn::Error::new_spanned(mc, "Rulidity: wrong number of arguments")
            .to_compile_error();
    }

    // Eval each arg in caller's scope and store it in a fresh local var
    let mut binds = Vec::new();
    let mut param_local: HashMap<String, u32> = HashMap::new();

    for ((pname, _), arg) in params.iter().zip(mc.args.iter()) {
        let v = lower_expression(arg, state);
        let slot = state.alloc_local();
        param_local.insert(pname.to_string(), slot);
        binds.push(quote! {
            #v
            asm.push_word(::rulidity::U256::from(#slot));
            asm.mstore();
        });
    }

    // lowering the body in fresh scope (params as locals and no calldata)
    let saved_locals = std::mem::replace(&mut state.locals, param_local);
    let saved_offsets = std::mem::take(&mut state.param_offsets);
    state.call_stack.push(name.clone());

    let body_tail = if returns(&output) {
        Tail::Leave
    } else {
        Tail::Void
    };

    let body = lower_stmts(&block.stmts, state, body_tail);

    state.call_stack.pop();
    state.locals = saved_locals;
    state.param_offsets = saved_offsets;

    let cleanup = if !want_value && returns(&output) {
        quote! { asm.pop(); }
    } else {
        quote! {}
    };

    quote! {
        #(#binds)*
        #body
        #cleanup
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

/// self.map.insert(key, value); including nested maps, e.g.
/// self.allowances.get(owner).insert(spender, amount);
fn lower_mapping_insert(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let args: Vec<&syn::Expr> = mc.args.iter().collect();
    if args.len() != 2 {
        return syn::Error::new_spanned(mc, "rulidity: insert takes (key, value)")
            .to_compile_error();
    }
    // the receiver is the mapping being written to; resolve its base slot
    let (base_code, base_ty) = match lower_place(&mc.receiver, state) {
        Ok(place) => place,
        Err(e) => return e,
    };
    if !matches!(ty_kind(&base_ty), TyKind::Mapping(_)) {
        return syn::Error::new_spanned(mc, "rulidity: .insert() is only valid on a Mapping")
            .to_compile_error();
    }
    let value = lower_expression(args[1], state); // value first (stays underneath)
    let key = lower_expression(args[0], state);
    quote! {
        #value
        #base_code
        #key
        asm.mapping_slot_from_stack();
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
    let then_body = lower_stmts(&if_expr.then_branch.stmts, state, Tail::Void);

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

            let else_body = lower_stmts(else_stmts, state, Tail::Void);
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

fn lower_expression(expr: &syn::Expr, state: &mut Lower) -> proc_macro2::TokenStream {
    match expr {
        // integer literal
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        }) => {
            let v: u64 = int.base10_parse().unwrap();
            quote! { asm.push_word(::rulidity::U256::from(#v)); }
        }
        // bool literal
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Bool(b),
            ..
        }) => {
            let v: u64 = if b.value { 1 } else { 0 };
            quote! { asm.push_word(::rulidity::U256::from(#v)); }
        }
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => {
            let bytes = pack_short_string(&s.value());
            let bs = bytes.iter();
            quote! {asm.push_word(::rulidity::U256::from_be_slice(&[#(#bs),*]));}
        }
        // self.field loads that field's storage slot
        syn::Expr::Field(field) if is_self(&field.base) => {
            let slot = member_slot(&field.member, state.ctx.storage);
            quote! { asm.load_slot(::rulidity::U256::from(#slot)); }
        }
        // self.func() -> internal function, needs to be before matching for maps and arrays so that self.get() does not enter those arms
        syn::Expr::MethodCall(mc)
            if is_self(&mc.receiver)
                && state
                    .ctx
                    .internal_functions
                    .contains_key(&mc.method.to_string()) =>
        {
            lower_internal_call(mc, true, state)
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

/// self.map.get(key) / self.array.get(idx), including nested chains like
/// self.allowances.get(owner).get(spender). Reads the resolved slot.
fn lower_get(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
    let place = syn::Expr::MethodCall(mc.clone());
    match lower_place(&place, state) {
        Ok((slot_code, value_ty)) => match ty_kind(&value_ty) {
            TyKind::Scalar => quote! {
                #slot_code
                asm.sload();
            },
            _ => syn::Error::new_spanned(
                mc,
                "rulidity: .get() must resolve to a scalar value to read",
            )
            .to_compile_error(),
        },
        Err(e) => e,
    }
}

fn pack_short_string(s: &str) -> [u8; 32] {
    assert!(
        s.len() <= 31,
        "Short-string literals are max 31 bytes, got {}",
        s.len()
    );

    let mut w = [0; 32];
    w[..s.len()].copy_from_slice(s.as_bytes());
    w[31] = (s.len() as u8) * 2;

    w
}

/// What a storage value at a resolved slot is, and its inner type for peeling.
enum TyKind {
    Scalar,
    Mapping(syn::Type), // value type V of Mapping<K, V>
    Array(syn::Type),   // element type T of Array<T>
}

/// Classify a declared field / peeled value type by its outer path segment.
fn ty_kind(ty: &syn::Type) -> TyKind {
    if let syn::Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
    {
        match seg.ident.to_string().as_str() {
            "Mapping" => {
                if let Some(v) = nth_generic_arg(seg, 1) {
                    return TyKind::Mapping(v);
                }
            }
            "Array" => {
                if let Some(t) = nth_generic_arg(seg, 0) {
                    return TyKind::Array(t);
                }
            }
            _ => {}
        }
    }
    TyKind::Scalar
}

/// The n-th angle-bracketed type argument of a path segment, e.g. arg 1 of
/// `Mapping<Address, U256>` is `U256`.
fn nth_generic_arg(seg: &syn::PathSegment, n: usize) -> Option<syn::Type> {
    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
        ab.args
            .iter()
            .filter_map(|a| match a {
                syn::GenericArgument::Type(t) => Some(t.clone()),
                _ => None,
            })
            .nth(n)
    } else {
        None
    }
}

/// Emit code that leaves a storage slot on the stack, and return the value type
/// living at that slot (so a further `.get` knows whether it is another level).
/// Handles `self.field` and any chain of `.get(key)` on top of it.
fn lower_place(
    expr: &syn::Expr,
    state: &mut Lower,
) -> Result<(proc_macro2::TokenStream, syn::Type), proc_macro2::TokenStream> {
    match expr {
        // self.field -> the field's base slot, typed by its declaration
        syn::Expr::Field(f) if is_self(&f.base) => {
            let ident = match &f.member {
                syn::Member::Named(id) => id,
                syn::Member::Unnamed(_) => {
                    return Err(
                        syn::Error::new_spanned(f, "tuple fields not supported").to_compile_error()
                    );
                }
            };
            let slot = match state.ctx.storage.get(ident) {
                Some(sf) => sf.slot as u64,
                None => {
                    return Err(
                        syn::Error::new_spanned(f, "unknown storage field").to_compile_error()
                    );
                }
            };
            let ty = match state.ctx.field_types.get(ident) {
                Some(t) => t.clone(),
                None => {
                    return Err(
                        syn::Error::new_spanned(f, "unknown storage field").to_compile_error()
                    );
                }
            };
            Ok((quote! { asm.push_word(::rulidity::U256::from(#slot)); }, ty))
        }
        // receiver.get(key) -> combine the receiver's slot with the key
        syn::Expr::MethodCall(mc) if mc.method == "get" => {
            if mc.args.len() != 1 {
                return Err(
                    syn::Error::new_spanned(mc, "rulidity: get takes one argument")
                        .to_compile_error(),
                );
            }
            let (base_code, base_ty) = lower_place(&mc.receiver, state)?;
            let key = lower_expression(&mc.args[0], state);
            match ty_kind(&base_ty) {
                TyKind::Mapping(v) => Ok((
                    quote! {
                        #base_code
                        #key
                        asm.mapping_slot_from_stack();
                    },
                    v,
                )),
                TyKind::Array(t) => Ok((
                    quote! {
                        #base_code
                        #key
                        asm.array_elem_slot_from_stack();
                    },
                    t,
                )),
                TyKind::Scalar => Err(syn::Error::new_spanned(
                    mc,
                    "rulidity: .get() is only valid on a Mapping or Array",
                )
                .to_compile_error()),
            }
        }
        _ => Err(
            syn::Error::new_spanned(expr, "rulidity: unsupported storage access")
                .to_compile_error(),
        ),
    }
}

/// self.array.len()
fn lower_len(mc: &syn::ExprMethodCall, state: &mut Lower) -> proc_macro2::TokenStream {
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
fn lower_binary(bin: &syn::ExprBinary, state: &mut Lower) -> proc_macro2::TokenStream {
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
fn lower_ident(p: &syn::ExprPath, state: &mut Lower) -> proc_macro2::TokenStream {
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
