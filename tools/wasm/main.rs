fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3, "usage: wat INPUT OUTPUT");
    let bytes = wat::parse_file(&args[1]).expect("valid WAT fixture");
    std::fs::write(&args[2], bytes).expect("write WASM fixture");
}
