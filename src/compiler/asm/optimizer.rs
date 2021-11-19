use std::collections::HashMap;

use super::{CompilableInstruction, Label};

pub fn opt_asm(input: Vec<CompilableInstruction>) -> Vec<CompilableInstruction> {
    if input.is_empty() {
        return vec![];
    }
    std::fs::write(
        "target/before_opt.asm",
        input
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let in_count = input.len();
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
    std::fs::write(
        "target/after_opt.asm",
        out.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    /* println!(
        "Optimized from {} ASM instructions to {} ASM instructions",
        in_count,
        out.len()
    ); */
    out
}

fn remap(asm: &mut [CompilableInstruction], amap: &HashMap<Label, Label>) {
    asm.iter_mut().for_each(|i| {
        if let CompilableInstruction::Jump(a)
        | CompilableInstruction::Label(a)
        | CompilableInstruction::If0(.., a) = i
        {
            *a = update(a, amap);
        }
    });
}

fn update(a: &Label, amap: &HashMap<Label, Label>) -> Label {
    match amap.get(a) {
        Some(a) => update(a, amap),
        None => a.clone(),
    }
}
