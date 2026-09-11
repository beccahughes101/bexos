mod process;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let name = args.first().map(String::as_str).unwrap_or("utility");
    let name = name.rsplit('/').next().unwrap_or(name);
    if matches!(name, "ps" | "kill") {
        let result = process::Manager::new().and_then(|mut manager| {
            bexos_cli_utilities::process::run(
                name,
                &args[1..],
                &mut manager,
                &mut std::io::stdout().lock(),
            )
        });
        match result {
            Ok(status) => std::process::exit(status),
            Err(e) => {
                eprintln!("{name}: {e}");
                std::process::exit(1);
            }
        }
    }
    let status = bexos_cli_utilities::run(
        name,
        &args[1..],
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(status);
}
