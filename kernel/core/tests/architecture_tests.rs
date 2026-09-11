use bexos_kernel_core::{
    mmu,
    runtime::{
        Context,
        snapshot::{read_context, write_context},
    },
    transplant::codec::{Reader, Writer},
};

#[test]
fn normalized_context_operations_preserve_all_other_state() {
    assert_eq!(core::mem::align_of::<Context>(), 16);
    assert_eq!(core::mem::size_of::<Context>(), 816);
    let mut context = Context::zero();
    let mut saved = saved_context(&context);
    for index in 0..64 {
        saved[272 + index * 8..280 + index * 8]
            .copy_from_slice(&(index as u64 * 0x12345678).to_le_bytes());
    }
    context = read_context(&mut Reader::new(&saved)).unwrap();
    context.stack_pointer = 0x80001000;
    context.instruction_pointer = 0x80002000;
    let vector_state = saved_context(&context)[272..784].to_vec();
    context.set_initial_argument(19);
    context.set_thread_pointer(0x90001000);
    context.syscall_words_mut()[2] = 41;
    assert_eq!(context.syscall_words()[0], 19);
    assert_eq!(context.syscall_words()[2], 41);
    assert_eq!(context.thread_pointer(), 0x90001000);
    assert_eq!(&saved_context(&context)[272..784], &vector_state);
    assert_eq!(context.stack_pointer, 0x80001000);
    assert_eq!(context.instruction_pointer, 0x80002000);
    let mut bytes = [0; 1024];
    let mut writer = Writer::new(&mut bytes);
    write_context(&mut writer, &context).unwrap();
    let len = writer.len();
    let decoded = read_context(&mut Reader::new(&bytes[..len])).unwrap();
    assert_eq!(&saved_context(&decoded)[272..784], &vector_state);
    assert_eq!(decoded.syscall_words(), context.syscall_words());
    assert_eq!(decoded.thread_pointer(), context.thread_pointer());
    bytes[len - 8..len].copy_from_slice(&(3 - Context::ARCHITECTURE).to_le_bytes());
    assert!(read_context(&mut Reader::new(&bytes[..len])).is_err());
}

#[test]
fn page_encoding_distinguishes_readonly_data_and_executable_kernel_pages() {
    let code = mmu::kernel_page_descriptor(
        0x12345000,
        mmu::MemoryAttr::Normal,
        mmu::Access::KernelReadOnly,
        true,
    );
    let data = mmu::kernel_page_descriptor(
        0x12345000,
        mmu::MemoryAttr::Normal,
        mmu::Access::KernelReadWrite,
        false,
    );
    assert_eq!(code & mmu::TABLE_ADDR_MASK, 0x12345000);
    assert_eq!(data & mmu::TABLE_ADDR_MASK, 0x12345000);
    if Context::ARCHITECTURE == 2 {
        assert_eq!(code & ((1 << 63) | 6), 0);
        assert_eq!(data & ((1 << 63) | 6), (1 << 63) | 2);
        let user = mmu::page_descriptor(
            0x12345000,
            mmu::MemoryAttr::Normal,
            mmu::Access::KernelUserReadOnly,
        );
        assert_eq!(user & 7, 5);
        assert_ne!(user & (1 << 63), 0);
        let initial = Context::zero();
        let saved = saved_context(&initial);
        assert_eq!(
            u64::from_le_bytes(saved[272..280].try_into().unwrap()),
            0x37f
        );
        assert_eq!(
            u64::from_le_bytes(saved[296..304].try_into().unwrap()),
            0x1f80
        );
    } else {
        assert_ne!(code, data);
        assert_eq!(code & (1 << 53), 0);
        assert_ne!(data & (1 << 53), 0);
    }
}

fn saved_context(context: &Context) -> [u8; 816] {
    let mut bytes = [0; 816];
    write_context(&mut Writer::new(&mut bytes), context).unwrap();
    bytes
}
