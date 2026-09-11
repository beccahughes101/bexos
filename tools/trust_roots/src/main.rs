mod tool;

fn main() {
    if let Err(error) = tool::run(std::env::args().skip(1)) {
        eprintln!("trust_roots: {error}");
        std::process::exit(1);
    }
}
