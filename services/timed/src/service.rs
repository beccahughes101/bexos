use alloc::string::ToString;
use alloc::vec::Vec;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, KernelTransport, Rpc, Startup, log};
use kernel_fidl::{
    ClockGetTimeRequest, ClockPublicClient, ClockType, HandleRef as KernelHandleRef,
    Status as KernelStatus, SystemPrivilegedAdjustClockRequest, SystemPrivilegedSetTimeClient,
};
use net_fidl::{
    FidlDecode as NetDecode, FidlEncode as NetEncode, HandleRef, IpAddress, Ipv4Address,
    NetstackCreateUdpSocketRequest, NetstackResolveHostRequest, NetstackResolveHostResponse,
    SocketAddress, SocketOptions, Status as NetStatus, UdpSocketBindRequest, UdpSocketBindResponse,
    UdpSocketRecvFromRequest, UdpSocketRecvFromResponse, UdpSocketSendToRequest,
    UdpSocketSendToResponse,
};
use time_fidl::{
    FidlDecode, FidlEncode, RtcHardwarePublicClient, RtcHardwareReadUtcRequest,
    RtcHardwareWriteUtcRequest, Status, SyncState, TimeManagerForceSyncRequest,
    TimeManagerForceSyncResponse, TimeManagerGetTimeQualityRequest,
    TimeManagerGetTimeQualityResponse, TimeManagerSetManualTimeRequest,
    TimeManagerSetManualTimeResponse, TimeManagerSetTimeServersRequest,
    TimeManagerSetTimeServersResponse, TimeManagerWatchTimeQualityRequest,
    TimeManagerWatchTimeQualityResponse, TimeQuality, TimeQualityWatcherOnTimeQualityRequest,
    TimeQualityWatcherPublicClient,
};

use crate::config::TimedConfig;
use crate::migration::Runtime;
use crate::{nts, sntp, state};
use bexos_net::secure::{NetstackConnector, RootConfigCache};

const NTP_PORT: u16 = 123;
const UTC_2020_NS: i64 = 1_577_836_800_000_000_000;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("timed startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    let config = TimedConfig::from_startup(&startup);
    let data = startup
        .namespace
        .iter()
        .find(|entry| entry.path == "/data")
        .map(|entry| Channel(entry.directory))
        .unwrap_or(Channel(0));
    Startup::ready(control).unwrap();
    log("timed: ready\n");
    serve(Runtime::new(
        control,
        startup.migration,
        TimedService::new(config, data, startup.service_grants),
    ))
    .await
}

pub struct TimedService {
    pub(crate) config: TimedConfig,
    pub(crate) data: Channel,
    pub(crate) quality: TimeQuality,
    pub(crate) netstack: Option<Channel>,
    pub(crate) tls_trust: Option<Channel>,
    pub(crate) rtc: Option<Channel>,
    pub(crate) nts_state: nts::NtsCookieState,
    pub(crate) has_clock: bool,
    pub(crate) has_set_time: bool,
    pub(crate) clients: Vec<BoundServiceEndpoint>,
    pub(crate) quality_watchers: Vec<Channel>,
    pub(crate) next_sync_ms: u64,
    pub(crate) pending_slew_ns: i64,
    pub(crate) slew_started_monotonic_ns: u64,
    pub(crate) slew_rate_ppm: i32,
}

impl TimedService {
    pub fn new(
        config: TimedConfig,
        data: Channel,
        grants: Vec<bexos_userspace::ServiceGrant>,
    ) -> Self {
        log("timed: loading retained clock state\n");
        let first_sync_ms = now_ms().saturating_add(u64::from(config.poll_interval_ms));
        let mut service = Self {
            config,
            data,
            quality: if data.0 != 0 {
                state::load(data)
            } else {
                state::default_quality()
            },
            netstack: None,
            tls_trust: None,
            rtc: None,
            nts_state: nts::NtsCookieState::default(),
            has_clock: false,
            has_set_time: false,
            clients: Vec::new(),
            quality_watchers: Vec::new(),
            next_sync_ms: first_sync_ms,
            pending_slew_ns: 0,
            slew_started_monotonic_ns: 0,
            slew_rate_ppm: 0,
        };
        for grant in grants {
            match grant.service.as_str() {
                "bexos.net.Netstack" => service.netstack = Some(Channel(grant.endpoint)),
                "bexos.security.trust.TlsTrustManager" => {
                    service.tls_trust = Some(Channel(grant.endpoint))
                }
                "bexos.time.RtcHardware" => service.rtc = Some(Channel(grant.endpoint)),
                "bexos.kernel.Clock" => service.has_clock = true,
                "bexos.kernel.SystemPrivileged" => service.has_set_time = true,
                _ => {}
            }
        }
        log("timed: initializing RTC clock\n");
        service.bootstrap_from_rtc();
        log("timed: clock initialization complete\n");
        service
    }

    pub fn empty_for_migration() -> Self {
        Self {
            config: TimedConfig::default(),
            data: Channel(0),
            quality: state::default_quality(),
            netstack: None,
            tls_trust: None,
            rtc: None,
            nts_state: nts::NtsCookieState::default(),
            has_clock: false,
            has_set_time: false,
            clients: Vec::new(),
            quality_watchers: Vec::new(),
            next_sync_ms: 0,
            pending_slew_ns: 0,
            slew_started_monotonic_ns: 0,
            slew_rate_ppm: 0,
        }
    }

    pub fn quality(&self) -> TimeQuality {
        self.quality
    }

    pub fn clients(&self) -> &[BoundServiceEndpoint] {
        &self.clients
    }

    pub fn quality_watchers(&self) -> &[Channel] {
        &self.quality_watchers
    }

    pub fn next_sync_ms(&self) -> u64 {
        self.next_sync_ms
    }

    pub fn nts_state(&self) -> &nts::NtsCookieState {
        &self.nts_state
    }

    pub fn set_migration_test_state(
        &mut self,
        quality: TimeQuality,
        nts_state: nts::NtsCookieState,
        clients: Vec<BoundServiceEndpoint>,
        quality_watchers: Vec<Channel>,
        next_sync_ms: u64,
    ) {
        self.quality = quality;
        self.nts_state = nts_state;
        self.clients = clients;
        self.quality_watchers = quality_watchers;
        self.next_sync_ms = next_sync_ms;
    }

    pub async fn force_sync(&mut self) -> Status {
        if !self.config.network_sync_enabled {
            return Status::ErrUnsupported;
        }
        let Some(monotonic_ns) = self.monotonic_ns() else {
            self.record_failure(Status::ErrInvalidArgs);
            return Status::ErrInvalidArgs;
        };
        let result = if self.config.use_nts {
            self.query_nts(monotonic_ns).await
        } else {
            self.query_sntp(monotonic_ns).await
        };
        match result {
            Ok(quality) => {
                let status = self.apply_network_quality(monotonic_ns, quality);
                if status != Status::Ok {
                    self.record_failure(Status::ErrInvalidArgs);
                    return status;
                }
                let _ = state::store(self.data, &self.quality);
                self.write_rtc_best_effort(self.quality.last_synced_timestamp_ns as i64);
                Status::Ok
            }
            Err(status) => {
                self.record_failure(status);
                status
            }
        }
    }

    pub fn set_manual_time(&mut self, utc_timestamp_ns: i64) -> Status {
        let Some(monotonic_ns) = self.monotonic_ns() else {
            self.record_failure(Status::ErrInvalidArgs);
            return Status::ErrInvalidArgs;
        };
        let offset = utc_timestamp_ns - monotonic_ns as i64;
        let delta = offset - self.current_kernel_offset(monotonic_ns);
        let status = self.adjust_realtime(delta, 0);
        if status != Status::Ok {
            self.record_failure(status);
            return status;
        }
        self.clear_slew();
        self.quality = TimeQuality {
            source: time_fidl::ClockSource::ManualUser,
            state: SyncState::Manual,
            stratum: 0,
            root_dispersion_ns: 0,
            last_synced_timestamp_ns: utc_timestamp_ns as u64,
            utc_offset_ns: offset,
            last_error: Status::Ok,
        };
        let _ = state::store(self.data, &self.quality);
        self.write_rtc_best_effort(utc_timestamp_ns);
        Status::Ok
    }

    pub fn set_time_servers(&mut self, server: &str, use_nts: bool) -> Status {
        if server.is_empty() || server.len() > 128 {
            return Status::ErrInvalidArgs;
        }
        self.config.primary_server = server.to_string();
        self.config.use_nts = use_nts;
        Status::Ok
    }

    fn record_failure(&mut self, status: Status) {
        if self.quality.state != SyncState::Failed || self.quality.last_error != status {
            self.quality.state = SyncState::Failed;
            self.quality.last_error = status;
            let _ = state::store(self.data, &self.quality);
        }
    }

    fn monotonic_ns(&self) -> Option<u64> {
        if !self.has_clock {
            return None;
        }
        let mut client = ClockPublicClient::new(KernelTransport(8));
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 32];
        let mut request_handles = [KernelHandleRef { raw: 0 }; 1];
        let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
        client
            .get_time(
                &ClockGetTimeRequest {
                    clock_type: ClockType::Monotonic,
                },
                &mut request_bytes,
                &mut request_handles,
                &mut response_bytes,
                &mut response_handles,
            )
            .ok()
            .filter(|response| response.status == KernelStatus::Ok)
            .map(|response| response.nanos)
    }

    fn bootstrap_from_rtc(&mut self) {
        // A retained quality record does not establish this boot's kernel clock.
        // Revalidate the hardware source before reporting synchronized time.
        self.quality = state::default_quality();
        if self.rtc.is_none() {
            log("timed: RTC endpoint unavailable\n");
            return;
        }
        let Some(monotonic_ns) = self.monotonic_ns() else {
            log("timed: monotonic clock unavailable for RTC bootstrap\n");
            return;
        };
        let Some(utc_timestamp_ns) = self.read_rtc() else {
            log("timed: RTC read failed validation\n");
            return;
        };
        let offset = utc_timestamp_ns - monotonic_ns as i64;
        if self.adjust_realtime(offset, 0) != Status::Ok {
            log("timed: RTC clock adjustment rejected\n");
            return;
        }
        self.clear_slew();
        self.quality = TimeQuality {
            source: time_fidl::ClockSource::RtcHardware,
            state: SyncState::Synced,
            stratum: 0,
            root_dispersion_ns: 0,
            last_synced_timestamp_ns: utc_timestamp_ns as u64,
            utc_offset_ns: offset,
            last_error: Status::Ok,
        };
        let _ = state::store(self.data, &self.quality);
        log("timed: validated RTC initialized realtime\n");
    }

    fn apply_network_quality(&mut self, monotonic_ns: u64, quality: TimeQuality) -> Status {
        self.materialize_slew(monotonic_ns);
        let delta = quality.utc_offset_ns - self.current_kernel_offset(monotonic_ns);
        let should_step = self.quality.state != SyncState::Synced
            || delta.unsigned_abs() > self.config.slew_step_threshold_ns as u64;
        let slew_rate_ppm = if should_step {
            0
        } else if delta < 0 {
            -self.config.slew_limit_ppm
        } else if delta > 0 {
            self.config.slew_limit_ppm
        } else {
            0
        };
        let status = self.adjust_realtime(delta, slew_rate_ppm);
        if status == Status::Ok {
            if slew_rate_ppm == 0 {
                self.clear_slew();
            } else {
                self.pending_slew_ns = delta;
                self.slew_started_monotonic_ns = monotonic_ns;
                self.slew_rate_ppm = slew_rate_ppm;
            }
            self.quality = quality;
        }
        status
    }

    fn materialize_slew(&mut self, monotonic_ns: u64) {
        if self.pending_slew_ns == 0 || self.slew_rate_ppm == 0 {
            self.clear_slew();
            return;
        }
        let elapsed = monotonic_ns.saturating_sub(self.slew_started_monotonic_ns);
        let accrued = (u128::from(elapsed) * u128::from(self.slew_rate_ppm.unsigned_abs())
            / 1_000_000u128)
            .min(u128::from(self.pending_slew_ns.unsigned_abs())) as i64;
        let signed = if self.pending_slew_ns < 0 {
            -accrued
        } else {
            accrued
        };
        self.pending_slew_ns = self.pending_slew_ns.saturating_sub(signed);
        self.slew_started_monotonic_ns = monotonic_ns;
        if self.pending_slew_ns == 0 {
            self.slew_rate_ppm = 0;
        }
    }

    fn current_kernel_offset(&self, monotonic_ns: u64) -> i64 {
        let mut clone = Self {
            config: self.config.clone(),
            data: self.data,
            quality: self.quality,
            netstack: self.netstack,
            tls_trust: self.tls_trust,
            rtc: self.rtc,
            nts_state: nts::NtsCookieState::default(),
            has_clock: self.has_clock,
            has_set_time: self.has_set_time,
            clients: Vec::new(),
            quality_watchers: Vec::new(),
            next_sync_ms: self.next_sync_ms,
            pending_slew_ns: self.pending_slew_ns,
            slew_started_monotonic_ns: self.slew_started_monotonic_ns,
            slew_rate_ppm: self.slew_rate_ppm,
        };
        clone.materialize_slew(monotonic_ns);
        self.quality.utc_offset_ns - clone.pending_slew_ns
    }

    fn clear_slew(&mut self) {
        self.pending_slew_ns = 0;
        self.slew_started_monotonic_ns = 0;
        self.slew_rate_ppm = 0;
    }

    fn adjust_realtime(&self, offset_delta_ns: i64, slew_rate_ppm: i32) -> Status {
        if !self.has_set_time {
            return Status::ErrInvalidArgs;
        }
        let mut client = SystemPrivilegedSetTimeClient::new(KernelTransport(4));
        let mut request_bytes = [0; 32];
        let mut response_bytes = [0; 16];
        let mut request_handles = [KernelHandleRef { raw: 0 }; 1];
        let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
        match client.adjust_clock(
            &SystemPrivilegedAdjustClockRequest {
                clock_type: ClockType::Realtime,
                offset_delta_ns,
                slew_rate_ppm,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        ) {
            Ok(response) if response.status == KernelStatus::Ok => Status::Ok,
            _ => Status::ErrInvalidArgs,
        }
    }

    fn read_rtc(&self) -> Option<i64> {
        let rtc = self.rtc?;
        let mut client = RtcHardwarePublicClient::new(Rpc(rtc));
        let mut request_bytes = [0; 16];
        let mut response_bytes = [0; 32];
        let mut request_handles = [time_fidl::HandleRef { raw: 0 }; 1];
        let mut response_handles = [time_fidl::HandleRef { raw: 0 }; 1];
        let response = client
            .read_utc(
                &RtcHardwareReadUtcRequest {},
                &mut request_bytes,
                &mut request_handles,
                &mut response_bytes,
                &mut response_handles,
            )
            .ok()?;
        (response.status == Status::Ok && response.utc_timestamp_ns >= UTC_2020_NS)
            .then_some(response.utc_timestamp_ns)
    }

    fn write_rtc_best_effort(&self, utc_timestamp_ns: i64) {
        let Some(rtc) = self.rtc else {
            return;
        };
        let mut client = RtcHardwarePublicClient::new(Rpc(rtc));
        let mut request_bytes = [0; 32];
        let mut response_bytes = [0; 16];
        let mut request_handles = [time_fidl::HandleRef { raw: 0 }; 1];
        let mut response_handles = [time_fidl::HandleRef { raw: 0 }; 1];
        let _ = client.write_utc(
            &RtcHardwareWriteUtcRequest { utc_timestamp_ns },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        );
    }

    async fn query_sntp(&mut self, monotonic_ns: u64) -> Result<TimeQuality, Status> {
        let address = self.resolve_server().await?;
        let udp = self.open_udp_socket().await?;
        let bind = UdpSocketBindRequest {
            local_addr: SocketAddress {
                addr: unspecified_for(address),
                port: 0,
            },
        };
        call_udp_bind(udp.0, &bind).await?;
        let mut request = [0; 48];
        let request_len = sntp::encode_request(&mut request)?;
        call_udp_send_to(
            udp.0,
            &request[..request_len],
            SocketAddress {
                addr: address,
                port: NTP_PORT,
            },
        )
        .await?;
        let response = call_udp_recv_from(udp.0).await?;
        let sample = sntp::decode_response(&response)?;
        Ok(sntp::quality_from_sample(sample, monotonic_ns))
    }

    async fn query_nts(&mut self, monotonic_ns: u64) -> Result<TimeQuality, Status> {
        if self.tls_trust.is_none() {
            return Err(Status::ErrNetworkUnreachable);
        }
        if !self.nts_state.configured() {
            self.bootstrap_nts_ke()?;
        }
        let ntp_server = if self.nts_state.server.is_empty() {
            self.config.primary_server.as_str()
        } else {
            core::str::from_utf8(&self.nts_state.server).map_err(|_| Status::ErrInvalidArgs)?
        };
        let address = self.resolve_name(ntp_server).await?;
        let port = if self.nts_state.port == 0 {
            NTP_PORT
        } else {
            self.nts_state.port
        };
        let udp = self.open_udp_socket().await?;
        let bind = UdpSocketBindRequest {
            local_addr: SocketAddress {
                addr: unspecified_for(address),
                port: 0,
            },
        };
        call_udp_bind(udp.0, &bind).await?;
        let mut request = [0; 2048];
        let (request_len, unique) = nts::encode_request(&mut self.nts_state, &mut request)?;
        call_udp_send_to(
            udp.0,
            &request[..request_len],
            SocketAddress {
                addr: address,
                port,
            },
        )
        .await?;
        let response = call_udp_recv_from(udp.0).await?;
        let sample = nts::decode_response(&response, &mut self.nts_state, &unique)?;
        Ok(nts::quality_from_sample(sample, monotonic_ns))
    }

    fn bootstrap_nts_ke(&mut self) -> Result<(), Status> {
        let (Some(netstack), Some(tls_trust)) = (self.netstack, self.tls_trust) else {
            return Err(Status::ErrNetworkUnreachable);
        };
        let mut request = Vec::new();
        nts::encode_ke_request(&mut request);
        let mut connector = NetstackConnector::new(netstack);
        let mut roots = RootConfigCache::new(tls_trust);
        let stream = bexos_net::secure::TlsConnector::connect(
            &mut connector,
            &self.config.primary_server,
            nts::NTS_KE_PORT,
        )
        .map_err(|_| Status::ErrNetworkUnreachable)?;
        let (response, c2s, s2c) = bexos_net::secure::tls_exchange_with_exporters(
            stream,
            &mut roots,
            &self.config.primary_server,
            &[nts::NTS_ALPN],
            &request,
            8192,
        )
        .map_err(|_| Status::ErrNetworkUnreachable)?;
        self.nts_state = nts::adopt_ke_response(&response, &self.config.primary_server, c2s, s2c)?;
        Ok(())
    }

    async fn resolve_server(&mut self) -> Result<IpAddress, Status> {
        self.resolve_name(&self.config.primary_server).await
    }

    async fn resolve_name(&self, hostname: &str) -> Result<IpAddress, Status> {
        let Some(netstack) = self.netstack else {
            return Err(Status::ErrNetworkUnreachable);
        };
        let mut client = net_fidl::NetstackPublicClient::new(Rpc(netstack));
        let mut request_bytes = [0; 512];
        let mut response_bytes = [0; 512];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response: NetstackResolveHostResponse = client
            .resolve_host(
                &NetstackResolveHostRequest { hostname },
                &mut request_bytes,
                &mut request_handles,
                &mut response_bytes,
                &mut response_handles,
            )
            .map_err(|_| Status::ErrNetworkUnreachable)?;
        if response.status != NetStatus::Ok {
            return Err(Status::ErrNetworkUnreachable);
        }
        response.addresses.get(0).map_err(|_| Status::ErrNotFound)
    }

    async fn open_udp_socket(&self) -> Result<UdpChannel, Status> {
        let Some(netstack) = self.netstack else {
            return Err(Status::ErrNetworkUnreachable);
        };
        let (client_end, server_end) = Channel::pair().map_err(|_| Status::ErrInvalidArgs)?;
        let client_end = UdpChannel(client_end);
        let mut client = net_fidl::NetstackPublicClient::new(Rpc(netstack));
        let mut request_bytes = [0; 64];
        let mut response_bytes = [0; 16];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let mut response_handles = [HandleRef { raw: 0 }; 1];
        let response = client
            .create_udp_socket(
                &NetstackCreateUdpSocketRequest {
                    options: SocketOptions {
                        non_blocking: Some(true),
                        keep_alive_ms: None,
                        rx_buffer_size: Some(8192),
                        tx_buffer_size: Some(8192),
                    },
                    socket: HandleRef { raw: server_end.0 },
                },
                &mut request_bytes,
                &mut request_handles,
                &mut response_bytes,
                &mut response_handles,
            )
            .map_err(|_| Status::ErrNetworkUnreachable)?;
        if response.status == NetStatus::Ok {
            Ok(client_end)
        } else {
            Err(Status::ErrNetworkUnreachable)
        }
    }
}

struct UdpChannel(Channel);
impl Drop for UdpChannel {
    fn drop(&mut self) {
        let _ = bexos_userspace::Memory::close(self.0.0);
    }
}

fn unspecified_for(address: IpAddress) -> IpAddress {
    match address {
        IpAddress::Ipv4(_) => IpAddress::Ipv4(Ipv4Address {
            octets: [0, 0, 0, 0],
        }),
        IpAddress::Ipv6(_) => IpAddress::Ipv6(net_fidl::Ipv6Address { octets: [0; 16] }),
    }
}

async fn serve(mut runtime: Runtime) -> ! {
    let control = runtime.control;
    let mut source = bexos_userspace::live_migration::Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        if runtime.service.config.network_sync_enabled
            && !source.active()
            && runtime.service.next_sync_ms == 0
        {
            let status = runtime.service.force_sync().await;
            let quality = runtime.service.quality();
            notify_time_watchers(&mut runtime.service.quality_watchers, quality);
            let delay = if status == Status::Ok {
                runtime.service.config.poll_interval_ms
            } else {
                runtime.service.config.initial_retry_ms
            };
            runtime.service.next_sync_ms = now_ms().saturating_add(u64::from(delay));
            source.changed(0);
        } else if runtime.service.config.network_sync_enabled
            && runtime.service.next_sync_ms != 0
            && now_ms() >= runtime.service.next_sync_ms
        {
            runtime.service.next_sync_ms = 0;
            source.changed(0);
        }
        if let Ok(message) = control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("TimeManager") {
                        runtime.service.clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(0);
                    }
                } else if metadata_protocol(metadata) == Some("TimeManager") {
                    let _ = bexos_userspace::Memory::close(endpoint);
                }
            }
        }
        if poll_clients(&mut runtime.service).await {
            source.changed(0);
        }
        sleep_until_next_sync(runtime.service.next_sync_ms).await;
    }
}

async fn poll_clients(service: &mut TimedService) -> bool {
    let mut changed = false;
    let clients = core::mem::take(&mut service.clients);
    for client in clients {
        let channel = client.channel;
        match channel.try_recv() {
            Ok(message) => {
                changed = true;
                let (ordinal, req) = envelope(&message.bytes);
                let handles = handle_refs(&message.handles);
                if !client.allows(ordinal) {
                    service.clients.push(client);
                    continue;
                }
                match ordinal {
                    1 => {
                        let response =
                            if TimeManagerGetTimeQualityRequest::decode(req, &handles).is_ok() {
                                TimeManagerGetTimeQualityResponse {
                                    status: Status::Ok,
                                    quality: service.quality(),
                                }
                            } else {
                                TimeManagerGetTimeQualityResponse {
                                    status: Status::ErrInvalidArgs,
                                    quality: service.quality(),
                                }
                            };
                        reply(channel, &response);
                    }
                    2 => {
                        let status = if TimeManagerForceSyncRequest::decode(req, &handles).is_ok() {
                            service.force_sync().await
                        } else {
                            Status::ErrInvalidArgs
                        };
                        reply(
                            channel,
                            &TimeManagerForceSyncResponse {
                                status,
                                quality: service.quality(),
                            },
                        );
                    }
                    3 => {
                        let status = match TimeManagerSetManualTimeRequest::decode(req, &handles) {
                            Ok(request) => service.set_manual_time(request.utc_timestamp_ns),
                            Err(_) => Status::ErrInvalidArgs,
                        };
                        let quality = service.quality();
                        notify_time_watchers(&mut service.quality_watchers, quality);
                        reply(
                            channel,
                            &TimeManagerSetManualTimeResponse {
                                status,
                                quality: service.quality(),
                            },
                        );
                    }
                    4 => {
                        let status = match TimeManagerSetTimeServersRequest::decode(req, &handles) {
                            Ok(request) => {
                                let server = request.servers.get(0).unwrap_or("");
                                service.set_time_servers(server, request.use_nts)
                            }
                            Err(_) => Status::ErrInvalidArgs,
                        };
                        reply(channel, &TimeManagerSetTimeServersResponse { status });
                    }
                    5 => {
                        let response =
                            match TimeManagerWatchTimeQualityRequest::decode(req, &handles) {
                                Ok(request) if request.watcher.raw != 0 => {
                                    service.quality_watchers.push(Channel(request.watcher.raw));
                                    let quality = service.quality();
                                    notify_time_watchers(&mut service.quality_watchers, quality);
                                    TimeManagerWatchTimeQualityResponse { status: Status::Ok }
                                }
                                _ => TimeManagerWatchTimeQualityResponse {
                                    status: Status::ErrInvalidArgs,
                                },
                            };
                        reply(channel, &response);
                    }
                    _ => {}
                }
                service.clients.push(client);
            }
            Err(KernelStatus::ErrPeerClosed) => {}
            Err(_) => service.clients.push(client),
        }
    }
    changed
}

fn notify_time_watchers(watchers: &mut Vec<Channel>, quality: TimeQuality) {
    watchers.retain(|watcher| {
        let mut client = TimeQualityWatcherPublicClient::new(Rpc(*watcher));
        let mut request_bytes = [0; 128];
        let mut request_handles = [time_fidl::HandleRef { raw: 0 }; 1];
        client
            .on_time_quality(
                &TimeQualityWatcherOnTimeQualityRequest { quality },
                &mut request_bytes,
                &mut request_handles,
            )
            .is_ok()
    });
}

async fn call_udp_bind(udp: Channel, request: &UdpSocketBindRequest) -> Result<(), Status> {
    // Nested SocketAddress + IpAddress requires 68 bytes for IPv4, 80 for IPv6.
    let mut request_bytes = [0; 128];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let encoded = request
        .encode(&mut request_bytes, &mut request_handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    let message = bexos_userspace_async::call_raw(
        Rpc(udp),
        3,
        &request_bytes[..encoded.bytes],
        &raw_net_handles(&request_handles[..encoded.handles]),
        true,
    )
    .await
    .map_err(|_| Status::ErrNetworkUnreachable)?;
    let response_handles = handle_refs_net(&message.handles);
    let response = UdpSocketBindResponse::decode(&message.bytes, &response_handles)
        .map_err(|_| Status::ErrNetworkUnreachable)?;
    if response.status == NetStatus::Ok {
        Ok(())
    } else {
        Err(Status::ErrNetworkUnreachable)
    }
}

async fn call_udp_send_to(
    udp: Channel,
    data: &[u8],
    destination: SocketAddress,
) -> Result<(), Status> {
    let mut request_bytes = [0; 9000];
    let mut request_handles = [HandleRef { raw: 0 }; 1];
    let encoded = (UdpSocketSendToRequest { data, destination })
        .encode(&mut request_bytes, &mut request_handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    let message = bexos_userspace_async::call_raw(
        Rpc(udp),
        1,
        &request_bytes[..encoded.bytes],
        &raw_net_handles(&request_handles[..encoded.handles]),
        true,
    )
    .await
    .map_err(|_| Status::ErrNetworkUnreachable)?;
    let response_handles = handle_refs_net(&message.handles);
    let response = UdpSocketSendToResponse::decode(&message.bytes, &response_handles)
        .map_err(|_| Status::ErrNetworkUnreachable)?;
    if response.status == NetStatus::Ok {
        Ok(())
    } else {
        Err(Status::ErrNetworkUnreachable)
    }
}

async fn call_udp_recv_from(udp: Channel) -> Result<Vec<u8>, Status> {
    let deadline = now_ms().saturating_add(2_000);
    loop {
        let mut request_bytes = [0; 16];
        let encoded = (UdpSocketRecvFromRequest {})
            .encode(&mut request_bytes, &mut [])
            .map_err(|_| Status::ErrInvalidArgs)?;
        let message = bexos_userspace_async::call_raw(
            Rpc(udp),
            2,
            &request_bytes[..encoded.bytes],
            &[],
            true,
        )
        .await
        .map_err(|_| Status::ErrTimedOut)?;
        let response_handles = handle_refs_net(&message.handles);
        let response = UdpSocketRecvFromResponse::decode(&message.bytes, &response_handles)
            .map_err(|_| Status::ErrTimedOut)?;
        if response.status == NetStatus::Ok {
            return Ok(response.data.to_vec());
        }
        if response.status != NetStatus::ErrShouldWait || now_ms() >= deadline {
            return Err(Status::ErrTimedOut);
        }
        bexos_userspace_async::yield_once().await;
    }
}

async fn sleep_until_next_sync(next_sync_ms: u64) {
    let now = now_ms();
    if next_sync_ms > now.saturating_add(1) {
        let delay_ms = next_sync_ms.saturating_sub(now).min(10);
        let deadline = now.saturating_add(delay_ms);
        while now_ms() < deadline {
            bexos_userspace_async::yield_once().await;
        }
    } else {
        bexos_userspace_async::yield_once().await;
    }
}

fn raw_net_handles(handles: &[HandleRef]) -> Vec<u64> {
    handles.iter().map(|handle| handle.raw).collect()
}

fn handle_refs_net(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

fn now_ms() -> u64 {
    ((bexos_userspace::syscall::ticks() as u128 * 1000)
        / bexos_userspace::syscall::frequency() as u128) as u64
}

fn metadata_protocol(metadata: &str) -> Option<&str> {
    let mut fields = metadata.split('|');
    let _service = fields.next()?;
    fields.next()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let ordinal = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    (ordinal, &bytes[8..])
}

fn handle_refs(handles: &[u64]) -> Vec<time_fidl::HandleRef> {
    handles
        .iter()
        .map(|raw| time_fidl::HandleRef { raw: *raw })
        .collect()
}

fn reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = [0; 512];
    let mut handles = [time_fidl::HandleRef { raw: 0 }; 4];
    if let Ok(encoded) = q.encode(&mut bytes, &mut handles) {
        let raw_handles: Vec<u64> = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        let _ = channel.send(&bytes[..encoded.bytes], &raw_handles);
    }
}
