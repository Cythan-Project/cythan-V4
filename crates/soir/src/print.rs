//! Textual pretty-printer for soir graphs.
//!
//! A scheduled text dump (post-GCM) will land with M3. This module
//! is the M0-era "raw graph" dump: one line per live node, listing
//! its kind and input edges. The output is meant for eyeballing and
//! `//! expect!` golden tests — no attempt is made to lay out
//! control flow visually. For that, export `dot_graph` (stubbed
//! below) once a debugger needs it.

use crate::ir::{Graph, NodeId, NodeKind};
use std::fmt::Write;

/// Textual dump of every live node in arena order. The first line
/// is the function's flat signature (`in=N out=M`) so a reader can
/// tell how many `Param(i)` / return values to expect.
///
/// Format per node:
/// ```text
///   n3: Match ctrl=n2 scrut=n7 arms=[[0], [1..=15]]
/// ```
pub fn dump_graph(g: &Graph) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "graph: in={} out={} ({} live / {} arena)",
        g.sig().input_count,
        g.sig().output_count,
        g.live_len(),
        g.arena_len(),
    )
    .unwrap();
    for (id, node) in g.iter() {
        write!(out, "  {}: ", id).unwrap();
        format_node(&mut out, &node.kind);
        if !node.users.is_empty() {
            write!(out, "   -> users={}", format_ids(&node.users)).unwrap();
        }
        writeln!(out).unwrap();
    }
    out
}

fn format_node(out: &mut String, kind: &NodeKind) {
    match kind {
        NodeKind::Start => out.push_str("Start"),
        NodeKind::Return { ctrl, eff, values } => {
            write!(
                out,
                "Return ctrl={} eff={} values={}",
                ctrl,
                eff,
                format_ids(values)
            )
            .unwrap();
        }
        NodeKind::Stop { ctrl, eff } => {
            write!(out, "Stop ctrl={} eff={}", ctrl, eff).unwrap();
        }
        NodeKind::Region { preds } => {
            write!(out, "Region preds={}", format_ids(preds)).unwrap();
        }
        NodeKind::Loop { entry, back } => {
            write!(out, "Loop entry={} back=", entry).unwrap();
            match back {
                Some(b) => write!(out, "{}", b).unwrap(),
                None => out.push_str("<open>"),
            }
        }
        NodeKind::Block { ctrl } => {
            write!(out, "Block ctrl={}", ctrl).unwrap();
        }
        NodeKind::BlockExit { preds, block } => {
            write!(out, "BlockExit block={} preds={}", block, format_ids(preds)).unwrap();
        }
        NodeKind::LoopExit { preds, loop_node } => {
            write!(
                out,
                "LoopExit loop={} preds={}",
                loop_node,
                format_ids(preds)
            )
            .unwrap();
        }
        NodeKind::Phi { region, values } => {
            write!(out, "Phi region={} values=", region).unwrap();
            format_optional_ids(out, values);
        }
        NodeKind::EffPhi { region, effs } => {
            write!(out, "EffPhi region={} effs=", region).unwrap();
            format_optional_ids(out, effs);
        }
        NodeKind::If { ctrl, cond } => {
            write!(out, "If ctrl={} cond={}", ctrl, cond).unwrap();
        }
        NodeKind::Match {
            ctrl,
            scrut,
            arm_values,
        } => {
            write!(out, "Match ctrl={} scrut={} arms=[", ctrl, scrut).unwrap();
            for (i, vals) in arm_values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_set(vals));
            }
            out.push(']');
        }
        NodeKind::Proj { of, kind } => {
            write!(out, "Proj of={} kind={}", of, kind).unwrap();
        }
        NodeKind::Const(v) => {
            write!(out, "Const {}", v).unwrap();
        }
        NodeKind::Inc(a) => write!(out, "Inc {}", a).unwrap(),
        NodeKind::Dec(a) => write!(out, "Dec {}", a).unwrap(),
        NodeKind::Add(a, b) => write!(out, "Add {} {}", a, b).unwrap(),
        NodeKind::Sub(a, b) => write!(out, "Sub {} {}", a, b).unwrap(),
        NodeKind::Eq(a, b) => write!(out, "Eq {} {}", a, b).unwrap(),
        NodeKind::ReadReg { ctrl, eff, reg } => {
            write!(out, "ReadReg ctrl={} eff={} reg={}", ctrl, eff, reg).unwrap();
        }
        NodeKind::WriteReg {
            ctrl,
            eff,
            reg,
            val,
        } => {
            write!(
                out,
                "WriteReg ctrl={} eff={} reg={} val={}",
                ctrl, eff, reg, val
            )
            .unwrap();
        }
        NodeKind::Call {
            ctrl,
            eff,
            target,
            args,
            ret_count,
        } => {
            write!(
                out,
                "Call ctrl={} eff={} target={}::{} args={} rets={}",
                ctrl,
                eff,
                target.type_name,
                target.method_name,
                format_ids(args),
                ret_count
            )
            .unwrap();
        }
        NodeKind::Dead => out.push_str("<dead>"),
    }
}

fn format_ids(ids: &[NodeId]) -> String {
    let mut s = String::from("[");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        write!(s, "{}", id).unwrap();
    }
    s.push(']');
    s
}

fn format_optional_ids(out: &mut String, ids: &[Option<NodeId>]) {
    out.push('[');
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match id {
            Some(v) => write!(out, "{}", v).unwrap(),
            None => out.push('_'),
        }
    }
    out.push(']');
}

/// Compact `{0, 1, 2, 3}` or `{0..=3}` style for value sets
/// (scrutinee arms). The ranged form drops in when the set is a
/// contiguous u4 interval covering 3+ values.
fn format_value_set(vals: &[u8]) -> String {
    if vals.is_empty() {
        return "{}".to_string();
    }
    let mut sorted = vals.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    // Try contiguous range detection.
    if sorted.len() >= 3 {
        let is_contig = sorted
            .windows(2)
            .all(|w| w[1].saturating_sub(w[0]) == 1);
        if is_contig {
            return format!("[{}..={}]", sorted[0], sorted[sorted.len() - 1]);
        }
    }
    let mut s = String::from("[");
    for (i, v) in sorted.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        write!(s, "{}", v).unwrap();
    }
    s.push(']');
    s
}
