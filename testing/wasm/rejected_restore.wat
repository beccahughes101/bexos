(module
 (import "bexos:kernel/ipc@1.0.0" "channel-read" (func $read (param i32 i32 i32 i32 i32) (result i64)))
 (import "bexos:kernel/ipc@1.0.0" "channel-write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1)
 ;; Application checkpoint: latest client resource ID and request counter.
 (func (export "bexos-service-version") (result i32) i32.const 1)
 (func (export "bexos-service-dispatch") (param $client i32)
  local.get $client
  if i32.const 0 local.get $client i32.store end
  i32.const 0 i32.load i32.eqz if return end
  i32.const 0 i32.load i32.const 64 i32.const 32 i32.const 128 i32.const 0 call $read
  i64.const 0 i64.lt_s if return end
  i32.const 4 i32.const 4 i32.load i32.const 1 i32.add i32.store
  i32.const 0 i32.load i32.const 4 i32.const 4 i32.const 128 i32.const 0 call $write drop)
 (func (export "bexos-checkpoint") (result i64) i64.const 8)
 (func (export "bexos-restore-allocate") (param i32) (result i32) i32.const 0)
 (func (export "bexos-restore") (param i32 i32) (result i32) i32.const 1)
 (func (export "bexos-activate"))
 (func (export "bexos-abort")))
