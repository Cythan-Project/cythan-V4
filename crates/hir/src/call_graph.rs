//! Phase 6 Step 6.1 — call graph construction & cycle detection.
//!
//! Given an entry `FnSig` and a map of compiled functions, traverses every
//! `Call` op to build a directed graph of call edges and fail on cycles
//! (recursion is not supported by the target VM).

use std::collections::{HashMap, HashSet};

use crate::{HirBlock, HirFunction, HirOp};

pub type FnSigKey = typer::FnSig;

#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    /// Edges: caller → set of callees.
    pub edges: HashMap<FnSigKey, HashSet<FnSigKey>>,
    /// Order in which nodes were first discovered from the entry point.
    pub reachable: Vec<FnSigKey>,
}

/// Build a call graph starting from `entry`. Returns an error if any
/// reachable function is unknown (missing from `functions`) or if the graph
/// contains a cycle (recursive calls).
pub fn build_call_graph(
    functions: &HashMap<FnSigKey, HirFunction>,
    entry: &FnSigKey,
) -> Result<CallGraph, String> {
    if !functions.contains_key(entry) {
        return Err(format!(
            "entry `{}::{}` not found",
            entry.type_name, entry.method_name
        ));
    }

    let mut graph = CallGraph::default();
    let mut stack: Vec<FnSigKey> = vec![entry.clone()];
    let mut seen: HashSet<FnSigKey> = HashSet::new();
    seen.insert(entry.clone());
    graph.reachable.push(entry.clone());

    while let Some(f) = stack.pop() {
        let hir = functions.get(&f).ok_or_else(|| {
            format!("missing function `{}::{}`", f.type_name, f.method_name)
        })?;
        let mut callees = HashSet::new();
        collect_callees(&hir.body, &mut callees);
        for c in &callees {
            if seen.insert(c.clone()) {
                graph.reachable.push(c.clone());
                stack.push(c.clone());
            }
        }
        graph.edges.insert(f, callees);
    }

    detect_cycle(&graph, entry)?;
    Ok(graph)
}

fn collect_callees(block: &HirBlock, out: &mut HashSet<FnSigKey>) {
    for op in &block.ops {
        match op {
            HirOp::Call { target, .. } => {
                let key = match &target.trait_name {
                    Some(t) => FnSigKey::new_trait(&target.type_name, &target.method_name, t),
                    None => FnSigKey::new(&target.type_name, &target.method_name),
                };
                out.insert(key);
            }
            HirOp::If0(_, a, b) => {
                collect_callees(a, out);
                collect_callees(b, out);
            }
            HirOp::Loop(b) | HirOp::Block(b) => collect_callees(b, out),
            HirOp::Match(_, arms) => {
                for (arm, _) in arms {
                    collect_callees(arm, out);
                }
            }
            _ => {}
        }
    }
}

/// 3-color DFS: White (unvisited), Gray (in-progress), Black (done).
fn detect_cycle(graph: &CallGraph, entry: &FnSigKey) -> Result<(), String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<FnSigKey, Color> = graph
        .edges
        .keys()
        .map(|k| (k.clone(), Color::White))
        .collect();

    fn visit(
        node: &FnSigKey,
        graph: &CallGraph,
        color: &mut HashMap<FnSigKey, Color>,
        path: &mut Vec<FnSigKey>,
    ) -> Result<(), String> {
        color.insert(node.clone(), Color::Gray);
        path.push(node.clone());
        if let Some(callees) = graph.edges.get(node) {
            for c in callees {
                match color.get(c).copied().unwrap_or(Color::White) {
                    Color::Gray => {
                        // Cycle found. Build a readable path.
                        let start = path.iter().position(|n| n == c).unwrap_or(0);
                        let cycle = path[start..]
                            .iter()
                            .chain(std::iter::once(c))
                            .map(|f| format!("{}::{}", f.type_name, f.method_name))
                            .collect::<Vec<_>>()
                            .join(" → ");
                        return Err(format!("recursion detected: {}", cycle));
                    }
                    Color::White => visit(c, graph, color, path)?,
                    Color::Black => {}
                }
            }
        }
        path.pop();
        color.insert(node.clone(), Color::Black);
        Ok(())
    }

    let mut path = Vec::new();
    visit(entry, graph, &mut color, &mut path)
}
