use super::*;
use crate::{
    host::Host,
    resources::{Kind, READ, WRITE},
    wasi::filesystem::FsResult,
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct FileHandle(Kind);
impl Handle for FileHandle {
    fn kind(&self) -> Kind {
        self.0
    }
    fn rights(&self) -> u32 {
        READ | WRITE
    }
    fn native(&self) -> u64 {
        1
    }
}
#[derive(Default)]
struct FileHost {
    opens: AtomicUsize,
    replies: Mutex<Vec<u32>>,
}
impl Host for FileHost {
    fn monotonic_ns(&self) -> u64 {
        0
    }
    fn log(&self, _: &[u8]) {}
    fn channel_write(
        &self,
        _: &dyn Handle,
        bytes: &[u8],
        handles: &[Entry],
    ) -> wasmtime::Result<()> {
        assert!(handles.is_empty());
        self.replies
            .lock()
            .unwrap()
            .push(u32::from_le_bytes(bytes.try_into().unwrap()));
        Ok(())
    }
    fn channel_read(
        &self,
        _: &dyn Handle,
        _: usize,
        _: usize,
    ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        Ok((1u64.to_le_bytes().to_vec(), vec![]))
    }
    fn open_file(
        &self,
        _: &dyn Handle,
        path: &str,
        _: crate::wasi::wasi::filesystem::types::OpenFlags,
        flags: crate::wasi::wasi::filesystem::types::DescriptorFlags,
    ) -> FsResult<Entry> {
        assert_eq!(path, "sequence.txt");
        assert!(!flags.contains(crate::wasi::wasi::filesystem::types::DescriptorFlags::WRITE));
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(Entry {
            name: path.into(),
            handle: Arc::new(FileHandle(Kind::File)),
        })
    }
    fn read_file(&self, _: &dyn Handle, offset: u64, count: usize) -> wasmtime::Result<Vec<u8>> {
        assert_eq!(count, 1);
        Ok(vec![b'0' + (offset % 10) as u8])
    }
}
#[test]
fn component_service_adopts_open_descriptor_and_stream_offset_without_reopening() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let host = Arc::new(FileHost::default());
    let mut ctx = context();
    ctx.host = host.clone();
    ctx.resources
        .insert(Entry {
            name: "/pkg".into(),
            handle: Arc::new(FileHandle(Kind::Directory)),
        })
        .unwrap();
    let client = ctx
        .resources
        .insert(Entry {
            name: "client".into(),
            handle: Arc::new(FileHandle(Kind::Channel)),
        })
        .unwrap();
    let bytes: Arc<[u8]> =
        wat::parse_str(include_str!("../../../../testing/wasm/file_service.wat"))
            .unwrap()
            .into();
    let mut source = run(crate::service_guest::ServiceGuest::instantiate(
        &engine,
        bytes.clone(),
        ctx,
    ))
    .unwrap();
    run(source.activate()).unwrap();
    assert_eq!(run(source.snapshot()).unwrap().wasi.len(), 3);
    run(source.dispatch(client)).unwrap();
    run(source.dispatch(0)).unwrap();
    let bulk = run(source.snapshot()).unwrap();
    assert_eq!(bulk.wasi.len(), 3);
    let prepared = Arc::new(vec![
        crate::prepared::PreparedService::compile(&engine, bytes, 1 << 20).unwrap(),
    ]);
    let mut candidate_context = context();
    candidate_context.host = host.clone();
    let mut candidate =
        run(bulk.prepare_candidate(engine, candidate_context, None, prepared)).unwrap();
    run(source.dispatch(0)).unwrap();
    let final_state = run(source.snapshot()).unwrap();
    run(final_state.install_checkpoint(&mut candidate, true)).unwrap();
    run(candidate.activate()).unwrap();
    run(candidate.dispatch(0)).unwrap();
    assert_eq!(*host.replies.lock().unwrap(), vec![1, 2, 3, 4]);
    assert_eq!(host.opens.load(Ordering::Relaxed), 1);
}
