use crate::config::DhcpLease;
use net_fidl::Status;

const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99];
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_MESSAGE_TYPE: u8 = 53;
const DHCP_ACK: u8 = 5;

pub fn parse_ack(packet: &[u8]) -> Result<DhcpLease, Status> {
    if packet.len() < 240 || packet[236..240] != DHCP_MAGIC {
        return Err(Status::ErrInvalidArgs);
    }
    let mut message_type = 0;
    let mut prefix_len = 24;
    let mut gateway = None;
    let mut dns = None;
    let mut offset = 240;
    while offset < packet.len() {
        let code = packet[offset];
        offset += 1;
        if code == 255 {
            break;
        }
        if code == 0 {
            continue;
        }
        let Some(&len) = packet.get(offset) else {
            return Err(Status::ErrInvalidArgs);
        };
        offset += 1;
        let end = offset
            .checked_add(len as usize)
            .ok_or(Status::ErrInvalidArgs)?;
        if end > packet.len() {
            return Err(Status::ErrInvalidArgs);
        }
        let value = &packet[offset..end];
        match code {
            OPT_MESSAGE_TYPE if len == 1 => message_type = value[0],
            OPT_SUBNET_MASK if len == 4 => prefix_len = mask_prefix(value)?,
            OPT_ROUTER if len >= 4 => gateway = Some([value[0], value[1], value[2], value[3]]),
            OPT_DNS if len >= 4 => dns = Some([value[0], value[1], value[2], value[3]]),
            _ => {}
        }
        offset = end;
    }
    if message_type != DHCP_ACK {
        return Err(Status::ErrShouldWait);
    }
    Ok(DhcpLease {
        ipv4: [packet[16], packet[17], packet[18], packet[19]],
        prefix_len,
        gateway,
        dns,
    })
}

fn mask_prefix(mask: &[u8]) -> Result<u8, Status> {
    let mut prefix = 0;
    let mut seen_zero = false;
    for byte in mask {
        for bit in (0..8).rev() {
            let set = (byte & (1 << bit)) != 0;
            if set && seen_zero {
                return Err(Status::ErrInvalidArgs);
            }
            if set {
                prefix += 1;
            } else {
                seen_zero = true;
            }
        }
    }
    Ok(prefix)
}
