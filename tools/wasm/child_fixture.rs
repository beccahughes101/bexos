fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 4, "child-fixture CHILD OPTIONS OUTPUT");
    let child = std::fs::read(&args[1]).unwrap();
    let options = std::fs::read(&args[2]).unwrap();
    assert!(child.len() < 4096 && options.len() < 4096);
    let source = format!(
        r#"(module
 (import "bexos:wasm/sandbox@1.0.0" "spawn" (func $spawn (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
 (import "bexos:wasm/sandbox@1.0.0" "invoke" (func $invoke (param i32 i32 i32 i32) (result i32)))
 (import "bexos:wasm/sandbox@1.0.0" "pause" (func $pause (param i32)))
 (import "bexos:wasm/sandbox@1.0.0" "resume" (func $resume (param i32)))
 (import "bexos:wasm/sandbox@1.0.0" "terminate" (func $terminate (param i32)))
 (import "bexos:wasm/sandbox@1.0.0" "status" (func $status (param i32) (result i32)))
 (memory (export "memory") 1)
 (data (i32.const 0) "{}") (data (i32.const 4096) "{}") (data (i32.const 8192) "double")
 (func (export "_start") (local $child i32)
  i32.const 0 i32.const {} i32.const 4096 i32.const {} i32.const 0 i32.const 0 i32.const 0 i32.const 0 call $spawn local.set $child
  local.get $child i32.const 8192 i32.const 6 i32.const 21 call $invoke i32.const 42 i32.ne if unreachable end
  local.get $child call $pause
  local.get $child call $status i32.const 1 i32.ne if unreachable end
  local.get $child call $resume
  local.get $child call $status i32.const 0 i32.ne if unreachable end
  local.get $child call $terminate
  local.get $child call $status i32.const 2 i32.ne if unreachable end))"#,
        escaped(&child),
        escaped(&options),
        child.len(),
        options.len()
    );
    std::fs::write(&args[3], wat::parse_str(source).unwrap()).unwrap();
}
