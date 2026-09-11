(module
 (import "bexos:kernel/ipc@1.0.0" "channel-read" (func $read (param i32 i32 i32 i32 i32) (result i64)))
 (import "bexos:kernel/ipc@1.0.0" "channel-write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
 (import "bexos:wasm/sandbox@1.0.0" "spawn" (func $spawn (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
 (import "bexos:wasm/sandbox@1.0.0" "invoke" (func $invoke (param i32 i32 i32 i32) (result i32)))
 (import "bexos:wasm/sandbox@1.0.0" "pause" (func $pause (param i32)))
 (import "bexos:wasm/sandbox@1.0.0" "resume" (func $resume (param i32)))
 (import "bexos:wasm/sandbox@1.0.0" "status" (func $status (param i32) (result i32)))
 (memory (export "memory") 1)
 (data (i32.const 4096) "{{CHILD}}")
 (data (i32.const 8192) "{{OPTIONS}}")
 (data (i32.const 12288) "next")
 ;; Application checkpoint: latest client resource ID, request counter, and live child ID.
 (func (export "bexos-service-version") (result i32) i32.const 1)
 (func (export "bexos-service-dispatch") (param $client i32) (local $next i32)
  i32.const 8 i32.load i32.eqz
  if
   i32.const 8
   i32.const 4096 i32.const {{CHILD_LEN}} i32.const 8192 i32.const {{OPTIONS_LEN}}
   i32.const 0 i32.const 0 i32.const 0 i32.const 0 call $spawn i32.store
   i32.const 8 i32.load call $pause
  end
  local.get $client
  if i32.const 0 local.get $client i32.store end
  i32.const 0 i32.load i32.eqz if return end
  i32.const 0 i32.load i32.const 64 i32.const 32 i32.const 128 i32.const 0 call $read
  i64.const 0 i64.lt_s if return end
  ;; The child remains paused between requests, including each migration safe point.
  i32.const 8 i32.load call $status i32.const 1 i32.ne if unreachable end
  i32.const 8 i32.load call $resume
  i32.const 8 i32.load i32.const 12288 i32.const 4 i32.const 0 call $invoke local.set $next
  i32.const 8 i32.load call $pause
  local.get $next i32.const 4 i32.load i32.const 1 i32.add i32.ne if unreachable end
  i32.const 4 local.get $next i32.store
  i32.const 0 i32.load i32.const 4 i32.const 4 i32.const 128 i32.const 0 call $write drop)
 (func (export "bexos-checkpoint") (result i64) i64.const 12)
 (func (export "bexos-restore-allocate") (param i32) (result i32) i32.const 0)
 (func (export "bexos-restore") (param i32 i32) (result i32) i32.const 0)
 (func (export "bexos-activate"))
 (func (export "bexos-abort")))
