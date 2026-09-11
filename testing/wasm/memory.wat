(module
 (memory (export "memory") 1025 2048)
 (func (export "_start")
  ;; Cross the native 64 MiB VMO boundary and then grow to the manifest limit.
  i32.const 67108864 i32.const 42 i32.store
  i32.const 1023 memory.grow i32.const 1025 i32.ne if unreachable end
  i32.const 134217724 i32.const 99 i32.store
  i32.const 67108864 i32.load i32.const 42 i32.ne if unreachable end
  i32.const 134217724 i32.load i32.const 99 i32.ne if unreachable end
  i32.const 1 memory.grow i32.const -1 i32.ne if unreachable end))
