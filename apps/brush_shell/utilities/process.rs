//! WASM utility process control uses an identity-bound appd launcher.
use app_opener_fidl::{self as protocol, FidlDecode};
use bexos_cli_utilities::process::{Process, ProcessManager};
use bexos_wasm_guest::command::{Resource, bind, receive, send};
use std::{
    future::Future,
    io,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
struct Wakeup;
impl Wake for Wakeup {
    fn wake(self: Arc<Self>) {}
}
// Every transport wait yields; Wasmtime's fuel slices give the OS opportunities
// to run appd. No native thread runtime or ambient WASI networking is required.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Wakeup));
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return result;
        }
    }
}
fn error(value: impl ToString) -> io::Error {
    io::Error::other(value.to_string())
}
pub struct Manager(Resource);
impl Manager {
    pub fn new() -> io::Result<Self> {
        block_on(bind()).map(Self).map_err(error)
    }
}
impl ProcessManager for Manager {
    fn list(&mut self) -> io::Result<Vec<Process>> {
        send(
            self.0.0,
            3,
            &protocol::CommandLauncherListProcessesRequest {},
        )
        .map_err(error)?;
        let message = block_on(receive(self.0.0)).map_err(error)?;
        let reply = protocol::CommandLauncherListProcessesResponse::decode(&message.data, &[])
            .map_err(|_| error("invalid process list"))?;
        if reply.status != protocol::OpenerStatus::Ok {
            return Err(error("appd could not list processes"));
        }
        let mut processes = Vec::new();
        for i in 0..reply.processes.len() {
            let p = reply
                .processes
                .get(i)
                .map_err(|_| error("invalid process record"))?;
            processes.push(Process {
                id: p.process_id,
                package: p.package_name.into(),
                name: p.process_name.into(),
                exited: p.exited,
                suspended: p.suspended,
                status: p.exit_code,
            });
        }
        Ok(processes)
    }
    fn signal(&mut self, id: u64, signal: u32) -> io::Result<()> {
        send(
            self.0.0,
            4,
            &protocol::CommandLauncherSignalProcessRequest {
                process_id: id,
                signal,
            },
        )
        .map_err(error)?;
        let message = block_on(receive(self.0.0)).map_err(error)?;
        let reply = protocol::CommandLauncherSignalProcessResponse::decode(&message.data, &[])
            .map_err(|_| error("invalid signal response"))?;
        if reply.status != protocol::OpenerStatus::Ok {
            return Err(error(format!(
                "process {id}: signal denied or process unavailable"
            )));
        }
        Ok(())
    }
}
