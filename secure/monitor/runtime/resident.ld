SECTIONS {
/* Shared fixed hardware layout, outside replaceable monitor banks. */
  .resident.text 0x18000000 : { KEEP(*(.resident.text)) }
  ASSERT(SIZEOF(.resident.text) <= 4096, "resident interrupt code overflow")
  .resident.gdt 0x18001000 : { KEEP(*(.resident.gdt)) }
  ASSERT(SIZEOF(.resident.gdt) <= 4096, "resident GDT overflow")
  .resident.idt 0x18002000 : { KEEP(*(.resident.idt)) }
  ASSERT(SIZEOF(.resident.idt) <= 4096, "resident IDT overflow")
  .resident.clock 0x18003000 : {
    KEEP(*(.resident.ticks))
    KEEP(*(.resident.clock))
  }
  ASSERT(SIZEOF(.resident.clock) <= 4096, "resident clock state overflow")
  .resident.root 0x18004000 : {
    KEEP(*(.resident.root.pml4))
    KEEP(*(.resident.root.pdpt))
    KEEP(*(.resident.root.pd))
    KEEP(*(.resident.root.gdt))
  }
  ASSERT(SIZEOF(.resident.root) <= 0xc000, "resident root tables overflow")
  .resident.iopm 0x18010000 : { KEEP(*(.resident.iopm)) }
  ASSERT(SIZEOF(.resident.iopm) <= 0x3000, "resident IOPM overflow")
  .resident.msrpm 0x18013000 : { KEEP(*(.resident.msrpm)) }
  ASSERT(SIZEOF(.resident.msrpm) <= 0x3000, "resident MSRPM overflow")
  .resident.tss 0x18016000 : { KEEP(*(.resident.tss)) }
  ASSERT(SIZEOF(.resident.tss) <= 0x1000, "resident TSS overflow")
  .resident.hsave 0x18018000 : { KEEP(*(.resident.hsave)) }
  ASSERT(SIZEOF(.resident.hsave) <= 4096, "resident HSAVE overflow")
  .resident.host 0x18019000 : { KEEP(*(.resident.host)) }
  ASSERT(SIZEOF(.resident.host) <= 4096, "resident host VMCB overflow")
  .resident.trusty_cpu 0x1801a000 : { KEEP(*(.resident.trusty_cpu)) }
  ASSERT(SIZEOF(.resident.trusty_cpu) <= 4096, "resident Trusty VMCB overflow")
  .resident.normal_cpus 0x1801b000 : { KEEP(*(.resident.normal_cpus)) }
  ASSERT(SIZEOF(.resident.normal_cpus) <= 0x4000, "resident BexOS VMCBs overflow")
  .resident.trusty_npt 0x18020000 : { KEEP(*(.resident.trusty_npt)) }
  ASSERT(SIZEOF(.resident.trusty_npt) <= 0x20000, "resident Trusty NPT overflow")
  .resident.normal_npt 0x18040000 : { KEEP(*(.resident.normal_npt)) }
  ASSERT(SIZEOF(.resident.normal_npt) <= 0x20000, "resident BexOS NPT overflow")
  .resident.dma 0x18060000 : { KEEP(*(.resident.dma)) }
  ASSERT(SIZEOF(.resident.dma) <= 0x10000, "resident DMA tables overflow")
  .resident.pci_policy 0x18070000 : { KEEP(*(.resident.pci_policy)) }
  ASSERT(SIZEOF(.resident.pci_policy) <= 0x1000, "resident PCI policy overflow")
  .resident.trial_uart 0x18071000 : { KEEP(*(.resident.trial_uart)) }
  ASSERT(SIZEOF(.resident.trial_uart) <= 0x3000, "resident trial UART overflow")
  .resident.approvals 0x18074000 : { KEEP(*(.resident.approvals)) }
  ASSERT(SIZEOF(.resident.approvals) <= 0x1000, "resident boot approvals overflow")
  .resident.recovery 0x18075000 : { KEEP(*(.resident.recovery)) }
  ASSERT(SIZEOF(.resident.recovery) <= 0x1000, "resident recovery ticket overflow")
  .resident.transfer_control 0x18076000 : { KEEP(*(.resident.transfer_control)) }
  ASSERT(SIZEOF(.resident.transfer_control) <= 0x1000, "resident transfer control overflow")
  .resident.client 0x18077000 : { KEEP(*(.resident.client)) }
  ASSERT(SIZEOF(.resident.client) <= 0x3000, "resident client record overflow")
  .resident.exceptions 0x1807a000 (NOLOAD) : { KEEP(*(.resident.exceptions)) }
  ASSERT(SIZEOF(.resident.exceptions) <= 0x6000, "resident exception stacks overflow")
  .resident.transfer 0x18080000 : { KEEP(*(.resident.transfer)) }
  ASSERT(SIZEOF(.resident.transfer) <= 0x20000, "resident transfer record overflow")
  .resident.normal_owner 0x180a0000 : { KEEP(*(.resident.normal_owner)) }
  ASSERT(SIZEOF(.resident.normal_owner) <= 0x2000, "resident normal owner overflow")
  .resident.secure_platform 0x180a2000 : { KEEP(*(.resident.secure_platform)) }
  ASSERT(SIZEOF(.resident.secure_platform) <= 0x1000, "resident secure platform overflow")
  .resident.secure_registers 0x180a3000 : { KEEP(*(.resident.secure_registers)) }
  ASSERT(SIZEOF(.resident.secure_registers) <= 0x1000, "resident secure registers overflow")
  .resident.stack 0x18100000 (NOLOAD) : { KEEP(*(.resident.stack)) }
  ASSERT(SIZEOF(.resident.stack) <= 0x100000, "resident transition stack overflow")
}
