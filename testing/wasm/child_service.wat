(module
 (memory (export "memory") 1)
 (func (export "next") (param i32) (result i32)
  i32.const 0 i32.const 0 i32.load i32.const 1 i32.add i32.store
  i32.const 0 i32.load)
 (func (export "bexos-service-version") (result i32) i32.const 1)
 (func (export "bexos-service-dispatch") (param i32))
 (func (export "bexos-checkpoint") (result i64) i64.const 4)
 (func (export "bexos-restore-allocate") (param i32) (result i32) i32.const 0)
 (func (export "bexos-restore") (param i32 i32) (result i32)
  local.get 1 i32.const 4 i32.ne)
 (func (export "bexos-activate"))
 (func (export "bexos-abort")))
