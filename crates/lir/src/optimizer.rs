use std::collections::HashMap;

use super::{CompilableInstruction, Label};

pub fn opt_asm(input: Vec<CompilableInstruction>) -> Vec<CompilableInstruction> {
    if input.is_empty() {
        return vec![];
    }
    let _in_count = input.len();
    let mut out = Vec::new();
    let mut label_map: HashMap<Label, Label> = HashMap::new();
    let mut in_jump = false;
    for el in input {
        if let CompilableInstruction::Jump(b) = &el {
            in_jump = true;
            loop {
                match out.pop() {
                    Some(CompilableInstruction::Label(a)) => {
                        label_map.insert(a, b.clone());
                    }
                    Some(e) => {
                        out.push(e);
                        break;
                    }
                    _ => break,
                }
            }
            out.push(el);
            continue;
        }
        if in_jump
            && matches!(
                &el,
                CompilableInstruction::Label(_) | &CompilableInstruction::If0(..)
            )
        {
            in_jump = false;
        }
        if in_jump {
            continue;
        }
        out.push(el);
    }
    remap(&mut out, &label_map);
    out
}

fn remap(asm: &mut [CompilableInstruction], amap: &HashMap<Label, Label>) {
    asm.iter_mut().for_each(|i| {
        match i {
            CompilableInstruction::Jump(a)
            | CompilableInstruction::Label(a)
            | CompilableInstruction::If0(.., a) => {
                *a = update(a, amap);
            }
            // `Match` carries a 16-slot jump table of optional
            // labels. Without updating those slots here, the
            // peephole pass' "Label A then Jump B → Jump B"
            // rewrite leaves the Match pointing at labels the
            // text emitter never declares — exactly the shape
            // that crashes `cythan_compiler` with "Try to init
            // your label at an index: 'lH…" on programs with
            // if-zero-shaped matches whose then-branch bodies
            // reduce to a single jump.
            CompilableInstruction::Match(_, slots) => {
                for slot in slots.iter_mut().flatten() {
                    *slot = update(slot, amap);
                }
            }
            _ => {}
        }
    });
}

fn update(a: &Label, amap: &HashMap<Label, Label>) -> Label {
    match amap.get(a) {
        Some(a) => update(a, amap),
        None => a.clone(),
    }
}
