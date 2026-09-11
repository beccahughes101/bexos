use bexos_userspace::{Channel, Memory};

/// Own the endpoints until startup transfers the server and the managed-service
/// registry takes ownership of the client. Failed launches close both sides.
pub(super) struct LaunchMigration {
    client: Option<Channel>,
    server: Option<Channel>,
}

impl LaunchMigration {
    pub fn new(enabled: bool) -> Result<Self, kernel_fidl::Status> {
        let (client, server) = if enabled {
            let (client, server) = Channel::pair()?;
            (Some(client), Some(server))
        } else {
            (None, None)
        };
        Ok(Self { client, server })
    }
    pub fn server(&self) -> Option<Channel> {
        self.server
    }
    pub fn sent(&mut self) {
        self.server = None;
    }
    pub fn commit(&mut self) -> Option<Channel> {
        self.client.take()
    }
}

impl Drop for LaunchMigration {
    fn drop(&mut self) {
        for endpoint in [self.client, self.server].into_iter().flatten() {
            let _ = Memory::close(endpoint.0);
        }
    }
}
