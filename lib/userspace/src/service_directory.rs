use crate::{Channel, Memory, Rpc, ServiceGrant};
use app_service_directory_fidl as service_directory;
use service_directory::{FidlDecode, FidlEncode, HandleRef};

pub struct ServiceDirectoryClient {
    rpc: Rpc,
}

impl ServiceDirectoryClient {
    pub fn from_startup_grants(grants: &[ServiceGrant]) -> Option<Self> {
        grants
            .iter()
            .find(|grant| {
                grant.service == "ServiceDirectory"
                    && grant.protocol == "bexos.app.service_directory.ServiceDirectory"
                    && grant.capability == "Public"
            })
            .map(|grant| Self {
                rpc: Rpc(Channel(grant.endpoint)),
            })
    }

    pub fn new(channel: Channel) -> Self {
        Self { rpc: Rpc(channel) }
    }

    pub fn connect(
        &mut self,
        service: &str,
        capability: &str,
    ) -> Result<Channel, service_directory::ServiceDirectoryStatus> {
        let (client, server) =
            Channel::pair().map_err(|_| service_directory::ServiceDirectoryStatus::LaunchFailed)?;
        let request = service_directory::ServiceDirectoryConnectRequest {
            service,
            capability,
            endpoint: HandleRef { raw: server.0 },
        };
        let mut req_bytes = [0u8; 512];
        let mut req_handles = [HandleRef { raw: 0 }; 2];
        let encoded = match request.encode(&mut req_bytes, &mut req_handles) {
            Ok(encoded) => encoded,
            Err(_) => {
                let _ = Memory::close(client.0);
                let _ = Memory::close(server.0);
                return Err(service_directory::ServiceDirectoryStatus::InvalidArgs);
            }
        };
        // Separate send from receive so each failure closes exactly the handles
        // still owned here. The server endpoint is consumed by a successful send.
        if self
            .rpc
            .call_raw(2, &req_bytes[..encoded.bytes], &[server.0], false)
            .is_err()
        {
            let _ = Memory::close(client.0);
            let _ = Memory::close(server.0);
            return Err(service_directory::ServiceDirectoryStatus::LaunchFailed);
        }
        let status = match self.rpc.0.recv() {
            Ok(message) => {
                if !message.handles.is_empty() {
                    for handle in message.handles {
                        let _ = Memory::close(handle);
                    }
                    service_directory::ServiceDirectoryStatus::LaunchFailed
                } else {
                    service_directory::ServiceDirectoryConnectResponse::decode(&message.bytes, &[])
                        .map(|response| response.status)
                        .unwrap_or(service_directory::ServiceDirectoryStatus::LaunchFailed)
                }
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {
                service_directory::ServiceDirectoryStatus::Timeout
            }
            Err(_) => service_directory::ServiceDirectoryStatus::LaunchFailed,
        };
        if status == service_directory::ServiceDirectoryStatus::Ok {
            Ok(client)
        } else {
            let _ = Memory::close(client.0);
            Err(status)
        }
    }
}
