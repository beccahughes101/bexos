use crate::{Channel, Memory, Rpc, ServiceGrant};
use app_service_directory_fidl as service_directory;
use service_directory::HandleRef;

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
        let mut resp_bytes = [0u8; 128];
        let mut resp_handles = [HandleRef { raw: 0 }; 1];
        let response = service_directory::ServiceDirectoryPublicClient::new(&mut self.rpc)
            .connect(
                &request,
                &mut req_bytes,
                &mut req_handles,
                &mut resp_bytes,
                &mut resp_handles,
            )
            .map_err(|_| service_directory::ServiceDirectoryStatus::LaunchFailed)?;
        if response.status == service_directory::ServiceDirectoryStatus::Ok {
            Ok(client)
        } else {
            let _ = Memory::close(client.0);
            Err(response.status)
        }
    }
}
