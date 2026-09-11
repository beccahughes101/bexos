use bexos_dioxus_guest::{bexos::wasm::kernel, rpc};
use shell_session_fidl as w;
use w::{FidlDecode, FidlEncode};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub uid: u64,
    pub state: u8,
    pub diagnostic: String,
    pub users: Vec<(u64, String)>,
    pub apps: Vec<(String, String)>,
}
pub struct Client(pub u32);
impl Client {
    pub fn connect() -> Result<Self, String> {
        use app_service_directory_fidl::*;
        let directory = rpc::find("ServiceDirectory")?;
        let (a, b) = kernel::channel_pair().map_err(|_| "Session channel unavailable")?;
        let mut bytes = [0; 64];
        let mut hs = [HandleRef { raw: 0 }; 1];
        let e = ServiceDirectoryBindShellSessionRequest {
            endpoint: HandleRef { raw: b as u64 },
        }
        .encode(&mut bytes, &mut hs)
        .map_err(|_| "Session encoding failed")?;
        let m = rpc::exchange(directory, 3, &bytes[..e.bytes], vec![b])?;
        let r = ServiceDirectoryBindShellSessionResponse::decode(&m.data, &[])
            .map_err(|_| "Invalid session response")?;
        if r.status != ServiceDirectoryStatus::Ok {
            rpc::close(a);
            return Err("Session access denied".into());
        }
        Ok(Self(a))
    }
    fn call(&self, ordinal: u64, request: &impl FidlEncode) -> Result<rpc::Message, String> {
        let mut bytes = vec![0; 1024];
        let e = match request.encode(&mut bytes, &mut []) {
            Ok(e) => e,
            Err(_) => {
                rpc::clear_sensitive(&mut bytes);
                return Err("Session encoding failed".into());
            }
        };
        // Authentication plus cold shell startup can involve durable storage and compilation.
        let result =
            rpc::exchange_timeout(self.0, ordinal, &bytes[..e.bytes], vec![], 900_000_000_000);
        rpc::clear_sensitive(&mut bytes);
        result
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let m = self.call(1, &w::SessionManagerGetStateRequest {})?;
        let r = w::SessionManagerGetStateResponse::decode(&m.data, &[])
            .map_err(|_| "Invalid session state")?;
        if r.status != w::Status::Ok {
            return Err("Session unavailable".into());
        }
        let mut s = Snapshot {
            uid: r.uid,
            state: r.state as u8,
            diagnostic: r.diagnostic.into(),
            ..Default::default()
        };
        for i in 0..r.users.len() {
            let u = r.users.get(i).map_err(|_| "Invalid user")?;
            s.users.push((u.uid, u.name.into()));
        }
        for i in 0..r.apps.len() {
            let a = r.apps.get(i).map_err(|_| "Invalid app")?;
            s.apps.push((a.package_id.into(), a.name.into()));
        }
        Ok(s)
    }
    pub fn login(&self, uid: u64, password: &str) -> Result<(), String> {
        self.status(2, &w::SessionManagerLoginRequest { uid, password })
    }
    pub fn action(&self, ordinal: u64) -> Result<(), String> {
        self.status(ordinal, &w::SessionManagerLockCurrentSessionRequest {})
    }
    pub fn close_app(&self, package_id: &str) -> Result<(), String> {
        self.status(6, &w::SessionManagerCloseAppRequest { package_id })
    }
    fn status(&self, ordinal: u64, q: &impl FidlEncode) -> Result<(), String> {
        let m = self.call(ordinal, q)?;
        let r =
            w::SessionManagerLoginResponse::decode(&m.data, &[]).map_err(|_| "Invalid response")?;
        if r.status == w::Status::Ok {
            Ok(())
        } else {
            Err(format!("{:?}", r.status))
        }
    }
    pub fn open_app(&self, package_name: &str) -> Result<(), String> {
        use app_opener_fidl::*;
        let mut bytes = vec![0; 4096];
        let q = OpenerOpenAppRequest {
            package_name,
            target: OpenTarget {
                default_process: true,
                specific_process: "",
            },
            arguments: WireStringVector::from_slice(&[]),
        };
        let e = q
            .encode(&mut bytes, &mut [])
            .map_err(|_| "App encoding failed")?;
        let m = rpc::exchange_timeout(
            rpc::find("Opener")?,
            3,
            &bytes[..e.bytes],
            vec![],
            360_000_000_000,
        )?;
        let r = OpenerOpenAppResponse::decode(&m.data, &[]).map_err(|_| "Invalid app response")?;
        if r.status == OpenerStatus::Ok && r.result == OpenResult::Success {
            Ok(())
        } else {
            Err("Unable to open app".into())
        }
    }
}

impl Client {
    fn resource_call(&self, ordinal: u64, q: &impl FidlEncode) -> Result<(), String> {
        let mut bytes = [0; 128];
        let mut hs = [w::HandleRef { raw: 0 }; 1];
        let e = q
            .encode(&mut bytes, &mut hs)
            .map_err(|_| "Invalid shell resource")?;
        let m = rpc::exchange(
            self.0,
            ordinal,
            &bytes[..e.bytes],
            hs[..e.handles].iter().map(|h| h.raw as u32).collect(),
        )?;
        let r = w::SessionManagerLoginResponse::decode(&m.data, &[])
            .map_err(|_| "Invalid shell response")?;
        if r.status == w::Status::Ok {
            Ok(())
        } else {
            Err(format!("{:?}", r.status))
        }
    }
    pub fn listen(&self) -> Result<Listener, String> {
        let (a, b) = kernel::channel_pair().map_err(|_| "No shell notification channel")?;
        if let Err(e) = self.resource_call(
            7,
            &w::SessionManagerRegisterUserShellRequest {
                endpoint: w::HandleRef { raw: b as u64 },
            },
        ) {
            rpc::close(a);
            return Err(e);
        }
        Ok(Listener {
            channel: a,
            display_token: 0,
        })
    }
    pub fn attach_display(&self, token: u32) -> Result<(), String> {
        self.resource_call(
            8,
            &w::SessionManagerAttachUserDisplayRequest {
                display_id: 1,
                view_token: w::HandleRef {
                    raw: rpc::clone_resource(token)? as u64,
                },
            },
        )
    }
}
/// Transferable endpoint and attached display reference; neither conveys SysUI authority.
#[derive(Default)]
pub struct Listener {
    pub channel: u32,
    pub display_token: u32,
}
impl Listener {
    pub fn poll(&mut self) -> Result<Option<u8>, String> {
        let mut state = None;
        loop {
            let m = match kernel::channel_read_checked(self.channel, 256, 1) {
                Ok(m) => m,
                Err(kernel::StreamError::WouldBlock) => return Ok(state),
                Err(_) => return Err("Shell notification channel closed".into()),
            };
            let hs = m
                .resources
                .iter()
                .map(|h| w::HandleRef { raw: *h as u64 })
                .collect::<Vec<_>>();
            if m.data.len() < 8 {
                for h in m.resources {
                    rpc::close(h)
                }
                continue;
            }
            match u64::from_le_bytes(m.data[..8].try_into().unwrap()) {
                1 => {
                    let q = w::UserShellAttachDisplayViewRequest::decode(&m.data[8..], &hs)
                        .map_err(|_| "Invalid display attachment")?;
                    if q.display_id != 1 {
                        for h in m.resources {
                            rpc::close(h)
                        }
                        continue;
                    }
                    if self.display_token != 0 {
                        rpc::close(self.display_token)
                    }
                    self.display_token = q.view_token.raw as u32;
                    let mut bytes = [0; 32];
                    let e = w::UserShellAttachDisplayViewResponse {
                        status: w::Status::Ok,
                    }
                    .encode(&mut bytes, &mut [])
                    .map_err(|_| "Invalid display reply")?;
                    kernel::channel_write(
                        self.channel,
                        &rpc::Message {
                            data: bytes[..e.bytes].to_vec(),
                            resources: vec![],
                        },
                    )
                    .map_err(|_| "Display reply failed")?;
                }
                2 => {
                    let q = w::UserShellSetSessionStateRequest::decode(&m.data[8..], &[])
                        .map_err(|_| "Invalid session notification")?;
                    state = Some(q.state as u8);
                    for h in m.resources {
                        rpc::close(h)
                    }
                }
                _ => {
                    for h in m.resources {
                        rpc::close(h)
                    }
                }
            }
        }
    }
}
