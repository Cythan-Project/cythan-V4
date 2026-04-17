//! Human-readable text dump of HIR. Used by the `build-hir` toolchain
//! subcommand to write a `.hir` file that developers can inspect.
//!
//! Format is intentionally verbose and line-oriented — diffs highlight
//! single-op changes, and the nested indentation mirrors block structure.

use std::collections::HashMap;
use std::fmt::Write;

use either::Either;

use crate::ir::{ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};

/// Dump an entire program (map of `FnSig → HirFunction`) to a string.
/// Functions are sorted by `(type_name, method_name, trait_name)` so
/// output is deterministic across runs.
pub fn dump_program(fns: &HashMap<typer::FnSig, HirFunction>) -> String {
    let mut keys: Vec<&typer::FnSig> = fns.keys().collect();
    keys.sort_by(|a, b| {
        (a.type_name.as_str(), a.method_name.as_str(), a.trait_name.as_deref())
            .cmp(&(b.type_name.as_str(), b.method_name.as_str(), b.trait_name.as_deref()))
    });
    let mut out = String::new();
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        let func = &fns[*k];
        dump_fn(k, func, &mut out);
    }
    out.push('\n');
    out
}

/// Dump one function.
pub fn dump_fn(sig: &typer::FnSig, func: &HirFunction, out: &mut String) {
    let header = match &sig.trait_name {
        None => format!("fn {}::{}", sig.type_name, sig.method_name),
        Some(t) => format!("fn <{} as {}>::{}", sig.type_name, t, sig.method_name),
    };
    let _ = writeln!(out, "{} [{} slots]", header, func.slot_count);

    // Signature layout: inputs, outputs. One slot per line.
    for slot in &func.sig.slots {
        let mutbit = if slot.mutable { "mut " } else { "" };
        let _ = writeln!(
            out,
            "  slot {} = {}: {}{} [{} cell{}]",
            slot.offset,
            slot.name,
            mutbit,
            slot.type_name,
            slot.size,
            if slot.size == 1 { "" } else { "s" },
        );
    }

    out.push_str("body:\n");
    dump_block(&func.body, 1, out);
}

fn indent(n: usize, out: &mut String) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

fn dump_block(block: &HirBlock, depth: usize, out: &mut String) {
    for op in &block.ops {
        dump_op(op, depth, out);
    }
    if let Some(rs) = block.result_slot {
        indent(depth, out);
        let _ = writeln!(out, "=> {}", rs);
    }
}

fn dump_op(op: &HirOp, depth: usize, out: &mut String) {
    indent(depth, out);
    match op {
        HirOp::Set(s, v) => {
            let _ = writeln!(out, "{} := {}", s, v);
        }
        HirOp::Copy(dst, src) => {
            let _ = writeln!(out, "{} := {}", dst, src);
        }
        HirOp::Inc(s) => {
            let _ = writeln!(out, "{}++", s);
        }
        HirOp::Dec(s) => {
            let _ = writeln!(out, "{}--", s);
        }
        HirOp::If0(cond, then_b, else_b) => {
            let _ = writeln!(out, "if {} == 0 {{", cond);
            dump_block(then_b, depth + 1, out);
            indent(depth, out);
            if else_b.ops.is_empty() && else_b.result_slot.is_none() {
                let _ = writeln!(out, "}}");
            } else {
                let _ = writeln!(out, "}} else {{");
                dump_block(else_b, depth + 1, out);
                indent(depth, out);
                let _ = writeln!(out, "}}");
            }
        }
        HirOp::Loop(body) => {
            let _ = writeln!(out, "loop {{");
            dump_block(body, depth + 1, out);
            indent(depth, out);
            let _ = writeln!(out, "}}");
        }
        HirOp::Break => {
            let _ = writeln!(out, "break");
        }
        HirOp::Continue => {
            let _ = writeln!(out, "continue");
        }
        HirOp::Stop => {
            let _ = writeln!(out, "stop");
        }
        HirOp::ReadRegister(s, reg) => {
            let _ = writeln!(out, "{} := read_register({})", s, reg);
        }
        HirOp::WriteRegister(reg, src) => match src {
            Either::Left(lit) => {
                let _ = writeln!(out, "write_register({}) := {}", reg, lit);
            }
            Either::Right(slot) => {
                let _ = writeln!(out, "write_register({}) := {}", reg, slot);
            }
        },
        HirOp::Block(b) => {
            let _ = writeln!(out, "block {{");
            dump_block(b, depth + 1, out);
            indent(depth, out);
            let _ = writeln!(out, "}}");
        }
        HirOp::Skip => {
            let _ = writeln!(out, "skip");
        }
        HirOp::Match(discr, arms) => {
            let _ = writeln!(out, "match {} {{", discr);
            for (body, vals) in arms {
                indent(depth + 1, out);
                let vs: Vec<String> = vals.iter().map(|v| v.to_string()).collect();
                let _ = writeln!(out, "{} => {{", vs.join(" | "));
                dump_block(body, depth + 2, out);
                indent(depth + 1, out);
                let _ = writeln!(out, "}}");
            }
            indent(depth, out);
            let _ = writeln!(out, "}}");
        }
        HirOp::Call { target, args, ret } => {
            let _ = writeln!(
                out,
                "call {} args=[{}] ret=[{}]",
                fn_ref_text(target),
                slot_list(args),
                slot_list(ret),
            );
        }
    }
}

fn fn_ref_text(r: &FnRef) -> String {
    let mut s = match &r.trait_name {
        None => format!("{}::{}", r.type_name, r.method_name),
        Some(t) => format!("<{} as {}>::{}", r.type_name, t, r.method_name),
    };
    if !r.template_args.is_empty() {
        s.push('<');
        for (i, arg) in r.template_args.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&template_arg_text(arg));
        }
        s.push('>');
    }
    s
}

fn template_arg_text(arg: &ConcreteTemplateArg) -> String {
    match arg {
        ConcreteTemplateArg::Type(t) => concrete_type_text(t),
        ConcreteTemplateArg::Value(v) => v.to_string(),
    }
}

fn concrete_type_text(t: &ConcreteType) -> String {
    if t.args.is_empty() {
        t.name.clone()
    } else {
        let mut s = t.name.clone();
        s.push('<');
        for (i, a) in t.args.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&template_arg_text(a));
        }
        s.push('>');
        s
    }
}

fn slot_list(slots: &[SlotId]) -> String {
    slots
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
