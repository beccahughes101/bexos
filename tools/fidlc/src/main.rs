use std::path::PathBuf;

fn main() {
    if let Err(err) = run() {
        eprintln!("fidlc: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut inputs = Vec::new();
    let mut deps = Vec::new();
    let mut args = std::env::args().skip(1);
    let output = loop {
        let Some(arg) = args.next() else {
            return Err(usage("missing input .fidl path"));
        };
        if arg == "--rust-out" {
            break args
                .next()
                .ok_or_else(|| usage("missing Rust output path"))?;
        }
        if arg == "--dep" {
            deps.push(resolve_input(
                args.next()
                    .ok_or_else(|| usage("missing --dep .fidl path"))?,
            ));
            continue;
        }
        if arg.starts_with("--") {
            return Err(usage(&format!(
                "unsupported backend flag `{arg}`; only --rust-out is implemented"
            )));
        }
        inputs.push(resolve_input(arg));
    };
    if inputs.is_empty() {
        return Err(usage("missing input .fidl path"));
    }
    if args.next().is_some() {
        return Err(usage("unexpected extra arguments"));
    }

    fidlc::compile_rust_inputs_with_deps(&inputs, &deps, &PathBuf::from(output))
}

fn usage(reason: &str) -> String {
    format!(
        "{reason}\nusage: fidlc <input.fidl> [more.fidl ...] [--dep dep.fidl ...] --rust-out <output.rs>"
    )
}

fn resolve_input(input: String) -> PathBuf {
    let path = PathBuf::from(&input);
    if path.is_absolute() || path.exists() {
        return path;
    }

    match std::env::var_os("BUILD_WORKSPACE_DIRECTORY") {
        Some(workspace) => PathBuf::from(workspace).join(path),
        None => path,
    }
}
