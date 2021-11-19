use std::time::Instant;

use crate::{compile, run, TestContext};

#[test]
fn morpion() {
    let (opt, ctx) = time("run_optimized", || {
        run(
            &{
                let a = time("compile_optimized", || compile("Counter".to_owned()));
                time("optimize", || a.optimize())
            },
            TestContext::new("1234567"),
        )
    });
    let prt = ctx.lock().unwrap().print.clone();
    if !prt.contains("OXO\nXOX\nO") || !prt.contains("O won!") {
        panic!("Test failed for optimized invalid output");
    }
    let (normal, ctx) = time("run_unoptimized", || {
        run(
            &time("compile_unoptimized", || compile("Counter".to_owned())),
            TestContext::new("1234567"),
        )
    });
    let prt = ctx.lock().unwrap().print.clone();
    if !prt.contains("OXO\nXOX\nO") || !prt.contains("O won!") {
        panic!("Test failed for unoptimized invalid output");
    }
    println!(
        "opt: {}ops | uopt {}ops ({}% improvement)",
        get_format(opt),
        get_format(normal),
        normal * 100 / opt
    );
}

fn time<T>(legend: &str, f: impl FnOnce() -> T) -> T {
    let instant = Instant::now();
    let t = f();
    println!("{} done in {:?}", legend, instant.elapsed());
    t
}

fn get_format(n: usize) -> String {
    if n > 1_000_000 {
        format!("{}M", (n / 100_000) as f64 / 10.0)
    } else if n > 1_000 {
        format!("{}K", (n / 100) as f64 / 10.0)
    } else {
        n.to_string()
    }
}
