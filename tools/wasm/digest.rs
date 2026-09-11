use sha2::{Digest, Sha256};
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3);
    let bytes = std::fs::read(&args[1]).unwrap();
    std::fs::write(&args[2], Sha256::digest(bytes)).unwrap();
}
