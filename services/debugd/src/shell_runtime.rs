//! Live provider lookup, terminal bridging, and transplant state.
use super::*;
use alloc::{string::String, vec, vec::Vec};
use app_opener_fidl as opener;
use bexos_debug_wire::*;
use bexos_migration::Error;
use bexos_migration::codec::{Decoder, Encoder};
use bexos_tty::{Frontend, Signal, TerminalMode, WindowSize};
use opener::{FidlDecode as _, FidlEncode as _};
use shell_fidl::{FidlDecode as _, FidlEncode as _};
#[derive(Default)]
pub struct ShellRuntime {
    pub next_id: u64,
    pub session: Option<Session>,
}
pub struct Session {
    pub id: u64,
    pub uid: u64,
    pub provider: String,
    pub terminal: Frontend,
    pub deadline: u64,
    pub eof: bool,
}
fn fail(message: &str) -> ShellResponse {
    ShellResponse {
        status: -1,
        message: message.into(),
        ..Default::default()
    }
}
fn now() -> u64 {
    bexos_userspace::syscall::ticks()
}
fn lease() -> u64 {
    bexos_userspace::syscall::frequency().saturating_mul(30)
}
impl ShellRuntime {
    pub fn expire(&mut self) -> bool {
        if self.session.as_ref().is_some_and(|s| now() >= s.deadline) {
            self.close();
            true
        } else {
            false
        }
    }
    pub fn renew(&mut self) {
        if let Some(s) = &mut self.session {
            s.deadline = now().saturating_add(lease());
        }
    }
    pub fn close(&mut self) {
        if let Some(s) = self.session.take() {
            s.terminal.close();
        }
    }
    pub async fn handle(
        &mut self,
        method: u32,
        mut q: ShellRequest,
        apps: &mut LiveAppManager,
        users: &mut LiveUserManager,
    ) -> ShellResponse {
        if method == METHOD_SHELL_OPEN {
            if self.session.is_some() {
                return fail("a shell is already active on this debug transport");
            }
            let user = match bexos_debugd::shell_auth::authenticate(users, &mut q).await {
                Ok(u) => u,
                Err(e) => {
                    return ShellResponse {
                        status: e.status,
                        message: e.message,
                        ..Default::default()
                    };
                }
            };
            let LiveAppManager::Proxy(apps) = apps else {
                return fail("appd shell provider lookup unavailable");
            };
            let (terminal, provider) = match open_provider(apps, user.uid, &user.name) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            if let Err(e) = terminal.resize(WindowSize {
                rows: if q.rows == 0 { 24 } else { q.rows },
                cols: if q.cols == 0 { 80 } else { q.cols },
                pixel_width: q.pixel_width,
                pixel_height: q.pixel_height,
            }) {
                terminal.close();
                return fail(&alloc::format!("initialize terminal dimensions: {e:?}"));
            }
            self.next_id = match self.next_id.checked_add(1) {
                Some(id) => id,
                None => {
                    terminal.close();
                    return fail("shell session IDs exhausted");
                }
            };
            let response = ShellResponse {
                session_id: self.next_id,
                uid: user.uid,
                provider: provider.clone(),
                ..Default::default()
            };
            self.session = Some(Session {
                id: self.next_id,
                uid: user.uid,
                provider,
                terminal,
                deadline: now().saturating_add(lease()),
                eof: false,
            });
            return response;
        }
        let Some(s) = &mut self.session else {
            return fail("no active shell session");
        };
        if q.session_id != s.id {
            return fail("unknown shell session");
        };
        s.deadline = now().saturating_add(lease());
        let mut response = ShellResponse {
            session_id: s.id,
            uid: s.uid,
            provider: s.provider.clone(),
            ..Default::default()
        };
        let result: Result<(), Status> = (|| {
            match method {
                METHOD_SHELL_EXCHANGE => {
                    if s.eof && !q.input.is_empty() {
                        return Err(Status::ErrInvalidArgs);
                    }
                    let n = s.terminal.write(&q.input)?;
                    response.consumed = n as u32;
                    if q.eof && n == q.input.len() && !s.eof {
                        s.terminal.eof()?;
                        s.eof = true;
                    }
                    response.stdout = Frontend::read(s.terminal.stdout, SHELL_CHUNK as u32)?;
                    response.stderr = Frontend::read(s.terminal.stderr, SHELL_CHUNK as u32)?;
                    let (exited, code) = s.terminal.status()?;
                    response.exited = exited && s.terminal.drained()?;
                    response.exit_code = code;
                }
                METHOD_SHELL_RESIZE => {
                    if q.rows == 0 || q.cols == 0 {
                        return Err(Status::ErrInvalidArgs);
                    }
                    s.terminal.resize(WindowSize {
                        rows: q.rows,
                        cols: q.cols,
                        pixel_width: q.pixel_width,
                        pixel_height: q.pixel_height,
                    })?;
                }
                METHOD_SHELL_MODE => {
                    if q.mode & !7 != 0 {
                        return Err(Status::ErrInvalidArgs);
                    }
                    s.terminal.mode(TerminalMode(q.mode))?;
                }
                METHOD_SHELL_SIGNAL => s
                    .terminal
                    .signal(Signal::decode_value(q.signal).map_err(|_| Status::ErrInvalidArgs)?)?,
                METHOD_SHELL_CLOSE => {}
                _ => return Err(Status::ErrInvalidArgs),
            }
            Ok(())
        })();
        if let Err(e) = result {
            // An untagged FIDL reply arriving after a timeout cannot be reused safely.
            if let Some(s) = self.session.take() {
                s.terminal.close_handles();
            }
            return fail(&alloc::format!(
                "terminal session closed after operation failed: {e:?}"
            ));
        }
        if method == METHOD_SHELL_CLOSE || response.exited {
            self.close();
        }
        response
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.next_id);
        w.word(self.session.is_some() as u64);
        if let Some(s) = &self.session {
            w.word(s.id);
            w.word(s.uid);
            w.text(&s.provider);
            for h in s.terminal.handles() {
                w.word(h);
            }
            w.word(s.eof as u64);
        }
    }
    pub fn adopt(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        self.next_id = r.word()?;
        self.session = if r.flag()? {
            let id = r.word()?;
            let uid = r.word()?;
            let provider = r.text(128)?.into();
            let terminal = Frontend {
                control: r.word()?,
                stdin: r.word()?,
                stdout: r.word()?,
                stderr: r.word()?,
            };
            if id == 0 || id > self.next_id || terminal.handles().contains(&0) {
                return Err(Error::InvalidData);
            }
            Some(Session {
                id,
                uid,
                provider,
                terminal,
                eof: r.flag()?,
                deadline: now().saturating_add(lease()),
            })
        } else {
            None
        };
        Ok(())
    }
    pub fn handles(&self) -> Vec<u64> {
        self.session
            .as_ref()
            .map_or_else(Vec::new, |s| s.terminal.handles().to_vec())
    }
}
// Temporary handle ownership is explicit: each failure closes every retained end.
fn open_provider(
    apps: &mut LifecycleAppManager,
    uid: u64,
    name: &str,
) -> Result<(Frontend, String), String> {
    let (opener_client, opener_server) =
        Channel::pair().map_err(|e| alloc::format!("create opener channel: {e:?}"))?;
    let mut retained = vec![opener_client.0, opener_server.0];
    let result = (|| {
        let q = lifecycle::AppLifecycleControlBindDebugOpenerRequest {
            uid,
            opener: lifecycle::HandleRef {
                raw: opener_server.0,
            },
        };
        let mut bytes = vec![0; 4096];
        let result = apps.call_raw_with_timeout(15, &q, 360_000);
        if result.is_ok() || apps.awaiting_response {
            retained.retain(|h| *h != opener_server.0);
        }
        let (response, handles) = result.map_err(|_| "bind scoped opener timed out or failed")?;
        let r = lifecycle::AppLifecycleControlBindDebugOpenerResponse::decode(&response, &handles)
            .map_err(|_| "decode opener binding")?;
        if r.status == lifecycle::AppLifecycleStatus::NotFound {
            return Err(
                "no shell provider installed; install an app providing bexos.shell.ShellProvider"
                    .into(),
            );
        }
        if r.status != lifecycle::AppLifecycleStatus::Ok {
            return Err("appd denied shell identity".into());
        }
        let (provider_client, provider_server) =
            Channel::pair().map_err(|_| "create provider channel")?;
        retained.extend([provider_client.0, provider_server.0]);
        let q = opener::OpenerGetPreferredInterfaceRequest {
            interface_name: "bexos.shell.ShellProvider",
            server_channel: opener::HandleRef {
                raw: provider_server.0,
            },
        };
        let mut hs = [opener::HandleRef { raw: 0 }; 4];
        let n = q
            .encode(&mut bytes, &mut hs)
            .map_err(|_| "encode provider lookup")?;
        let r = owned_call(
            opener_client,
            4,
            &bytes[..n.bytes],
            provider_server.0,
            &mut retained,
            360,
        )
        .map_err(|_| "shell provider lookup timed out")?;
        let r = opener::OpenerGetPreferredInterfaceResponse::decode(&r.bytes, &[])
            .map_err(|_| "decode provider lookup")?;
        if r.status == opener::OpenerStatus::NotFound {
            return Err(
                "no shell provider installed; install an app providing bexos.shell.ShellProvider"
                    .into(),
            );
        }
        if r.status != opener::OpenerStatus::Ok {
            return Err("shell provider launch denied or failed".into());
        }
        let provider = r.selected_package.to_string();
        let (session_client, session_server) =
            Channel::pair().map_err(|_| "create terminal channel")?;
        retained.extend([session_client.0, session_server.0]);
        let env = [
            alloc::format!("USER={name}"),
            "HOME=/data".into(),
            "TERM=xterm-256color".into(),
            "PATH=/pkg/bin:/system/bin".into(),
        ];
        let refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();
        let q = shell_fidl::ShellProviderCreateSessionRequest {
            session: shell_fidl::HandleRef {
                raw: session_server.0,
            },
            environment: shell_fidl::WireStringVector::from_slice(&refs),
        };
        let mut hs = [shell_fidl::HandleRef { raw: 0 }; 4];
        let n = q
            .encode(&mut bytes, &mut hs)
            .map_err(|_| "encode terminal creation")?;
        let r = owned_call(
            provider_client,
            1,
            &bytes[..n.bytes],
            session_server.0,
            &mut retained,
            60,
        )
        .map_err(|_| "shell session creation timed out")?;
        let r = shell_fidl::ShellProviderCreateSessionResponse::decode(&r.bytes, &[])
            .map_err(|_| "decode terminal creation")?;
        if r.status != Status::Ok {
            return Err("shell provider rejected session".into());
        }
        let terminal = Frontend::take(session_client.0)
            .map_err(|e| alloc::format!("take terminal streams: {e:?}"))?;
        retained.retain(|h| *h != session_client.0);
        Ok((terminal, provider))
    })();
    for h in retained {
        let _ = Memory::close(h);
    }
    result
}

fn owned_call(
    channel: Channel,
    ordinal: u64,
    bytes: &[u8],
    handle: u64,
    retained: &mut Vec<u64>,
    timeout_seconds: u64,
) -> Result<bexos_userspace::Message, Status> {
    let mut message = ordinal.to_le_bytes().to_vec();
    message.extend_from_slice(bytes);
    channel.send(&message, &[handle])?;
    // BexOS channels MOVE capabilities; never close their old numeric identities.
    retained.retain(|h| *h != handle);
    channel.recv_with_timeout(timeout_seconds)
}
