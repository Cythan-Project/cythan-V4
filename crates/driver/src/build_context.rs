use std::path::Path;
use std::process::exit;

use errors::{report, Error, Span, SpannedObject};
use mir::{Mir, MirCodeBlock};

use crate::STACK_SIZE;
use cythan_frontend::{
    compiler::{
        class_loader::ClassLoader,
        state::{code_manager::CodeManager, local_state::LocalState},
    },
    natives::load_natives,
    parser::ty::Type,
};

/// Compile a source file to MIR.
///
/// `file_path` is the path to the main `.ct` file (e.g. `std/Morpion.ct` or `./my_game.ct`).
/// `std_dir` is the standard library directory. All `.ct` files in it are loaded automatically.
/// The main class name is derived from the file stem (e.g. `Morpion` from `std/Morpion.ct`).
pub fn compile(file_path: &Path, std_dir: &Path, optimize: bool) -> MirCodeBlock {
    let file_path = file_path.to_owned();
    let std_dir = std_dir.to_owned();
    let child = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || generate_mir(&file_path, &std_dir))
        .unwrap();
    let k = child.join().unwrap();
    let count = k.instr_count();
    let k = if optimize { k.optimize_code_new() } else { k };
    let ncount = k.instr_count();
    eprintln!(
        "Optimized from {} to {} ({:.02}%)",
        count,
        ncount,
        if count > 0 {
            (count - ncount) as f64 / count as f64 * 100.
        } else {
            0.0
        }
    );
    k
}

fn generate_mir_(file_path: &Path, std_dir: &Path) -> Result<MirCodeBlock, Error> {
    let mut cl = ClassLoader::new();

    // Load standard library
    if std_dir.is_dir() {
        for file in std::fs::read_dir(std_dir).unwrap() {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|e| e == "ct") {
                cl.load_string(
                    &std::fs::read_to_string(&path).unwrap(),
                    path.to_str().unwrap(),
                )?;
            }
        }
    }

    // Load the main file (if not already in std)
    let file_path_canonical = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.to_owned());
    let already_loaded = std_dir.is_dir()
        && std::fs::read_dir(std_dir).unwrap().any(|f| {
            f.ok()
                .and_then(|f| f.path().canonicalize().ok())
                .is_some_and(|p| p == file_path_canonical)
        });

    if !already_loaded {
        cl.load_string(
            &std::fs::read_to_string(file_path)
                .unwrap_or_else(|e| panic!("Cannot read {}: {}", file_path.display(), e)),
            file_path.to_str().unwrap(),
        )?;
    }

    load_natives(&mut cl);

    let class_name = file_path
        .file_stem()
        .expect("File has no stem")
        .to_str()
        .unwrap();

    let rs = cl
        .view(&Type::simple(class_name, Span::default()))?
        .method_view(&SpannedObject(Span::default(), "main".to_owned()), &None)?
        .execute(&mut LocalState::new(), &mut CodeManager::new(cl), vec![])?;
    let mut mir = rs.mir;
    mir.add_mir(Mir::Stop);
    Ok(mir)
}

fn generate_mir(file_path: &Path, std_dir: &Path) -> MirCodeBlock {
    match generate_mir_(file_path, std_dir) {
        Ok(e) => e,
        Err(e) => {
            report(e);
            exit(1);
        }
    }
}
