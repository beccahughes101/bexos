(component
 (core module $m
  (memory (export "memory") 1)
  (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 1024)
  (func (export "dispatch") (param i32)
   i32.const 0 i32.const 0 i32.load local.get 0 i32.add i32.store)
  (func (export "checkpoint") (result i32)
   i32.const 64 i32.const 0 i32.store
   i32.const 68 i32.const 4 i32.store i32.const 64)
  (func (export "restore") (param i32 i32) (result i32)
   local.get 1 i32.const 4 i32.ne if i32.const 1 return end
   i32.const 0 local.get 0 i32.load i32.store i32.const 0)
  (func (export "activate")) (func (export "abort")))
 (core instance $i (instantiate $m))
 (func $dispatch (param "resource-id" u32) (canon lift (core func $i "dispatch")))
 (func $checkpoint (result (list u8)) (canon lift (core func $i "checkpoint") (memory $i "memory")))
 (func $restore (param "checkpoint" (list u8)) (result (result))
  (canon lift (core func $i "restore") (memory $i "memory") (realloc (func $i "realloc"))))
 (func $activate (canon lift (core func $i "activate")))
 (func $abort (canon lift (core func $i "abort")))
 (instance $lifecycle (export "dispatch" (func $dispatch)) (export "checkpoint" (func $checkpoint))
  (export "restore" (func $restore)) (export "activate" (func $activate)) (export "abort" (func $abort)))
 (export "bexos:wasm/lifecycle@1.0.0" (instance $lifecycle)))
