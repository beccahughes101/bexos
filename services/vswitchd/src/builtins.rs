use sha2::{Digest, Sha256};

use crate::extensions::{Extension, FailurePolicy};
use crate::switch::SwitchError;
use bexos_network_extension_abi::Hook;

pub const FIREWALL_NAME: &str = "bexos.bundled.firewall";
pub const NAT_PRE_NAME: &str = "bexos.bundled.nat.pre-routing";
pub const NAT_POST_NAME: &str = "bexos.bundled.nat.post-routing";

const FIREWALL_WASM: &[u8] = include_bytes!(env!("BEXOS_FIREWALL_WASM"));
const NAT_WASM: &[u8] = include_bytes!(env!("BEXOS_NAT_WASM"));
const DEFAULT_FIREWALL_CONFIG: &[u8] = b"NFW1\0\0\0\0";

pub fn firewall(generation: u64) -> Result<Extension, SwitchError> {
    firewall_with_config(DEFAULT_FIREWALL_CONFIG, generation)
}

pub fn firewall_with_config(config: &[u8], generation: u64) -> Result<Extension, SwitchError> {
    compile(
        FIREWALL_NAME,
        Hook::Firewall,
        FailurePolicy::Closed,
        FIREWALL_WASM,
        config,
        generation,
    )
}

pub fn nat(config: &[u8], generation: u64) -> Result<[Extension; 2], SwitchError> {
    Ok([
        compile(
            NAT_PRE_NAME,
            Hook::PreRouting,
            FailurePolicy::Closed,
            NAT_WASM,
            config,
            generation,
        )?,
        compile(
            NAT_POST_NAME,
            Hook::PostRouting,
            FailurePolicy::Closed,
            NAT_WASM,
            config,
            generation,
        )?,
    ])
}

fn compile(
    name: &str,
    hook: Hook,
    policy: FailurePolicy,
    wasm: &[u8],
    config: &[u8],
    generation: u64,
) -> Result<Extension, SwitchError> {
    let digest: [u8; 32] = Sha256::digest(wasm).into();
    Extension::compile(name, hook, policy, digest, wasm, config, generation)
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use bexos_network_extension_abi::Direction;

    use super::*;
    use crate::extensions::ExtensionSet;
    use crate::packet::{Packet, PacketDisposition};

    #[test]
    fn bundled_firewall_is_stateful_and_fail_closed_by_default() {
        let mut extensions = ExtensionSet::default();
        extensions.install(firewall(1).unwrap()).unwrap();

        let mut outbound = vec![udp_packet(
            [10, 0, 0, 2],
            [198, 51, 100, 9],
            41000,
            53,
            Direction::VirtualIngress,
        )];
        extensions.process(Hook::Firewall, &mut outbound);
        assert!(!extensions.iter().next().unwrap().faulted);
        assert_eq!(outbound[0].disposition, PacketDisposition::Continue);

        let mut reply = vec![udp_packet(
            [198, 51, 100, 9],
            [10, 0, 0, 2],
            53,
            41000,
            Direction::PhysicalIngress,
        )];
        extensions.process(Hook::Firewall, &mut reply);
        assert_eq!(reply[0].disposition, PacketDisposition::Continue);

        let mut unsolicited = vec![udp_packet(
            [198, 51, 100, 10],
            [10, 0, 0, 2],
            53,
            41001,
            Direction::PhysicalIngress,
        )];
        extensions.process(Hook::Firewall, &mut unsolicited);
        assert_eq!(unsolicited[0].disposition, PacketDisposition::Drop);
    }

    #[test]
    fn nat_pair_shares_mappings_between_graph_hooks() {
        let mut config = vec![0u8; 48];
        config[..4].copy_from_slice(b"NAT1");
        config[4..8].copy_from_slice(&[203, 0, 113, 5]);
        config[8..10].copy_from_slice(&40000u16.to_le_bytes());
        config[10..12].copy_from_slice(&40010u16.to_le_bytes());
        let mut extensions = ExtensionSet::default();
        extensions
            .replace_nat_pair(nat(&config, 7).unwrap())
            .unwrap();

        let mut outbound = vec![udp_packet(
            [10, 0, 0, 2],
            [198, 51, 100, 9],
            1234,
            53,
            Direction::VirtualIngress,
        )];
        extensions.process(Hook::PostRouting, &mut outbound);
        assert!(extensions.iter().all(|extension| !extension.faulted));
        assert_eq!(&outbound[0].bytes[26..30], &[203, 0, 113, 5]);
        assert_eq!(
            u16::from_be_bytes(outbound[0].bytes[34..36].try_into().unwrap()),
            40000
        );

        let mut inbound = vec![udp_packet(
            [198, 51, 100, 9],
            [203, 0, 113, 5],
            53,
            40000,
            Direction::PhysicalIngress,
        )];
        extensions.process(Hook::PreRouting, &mut inbound);
        assert_eq!(&inbound[0].bytes[30..34], &[10, 0, 0, 2]);
        assert_eq!(
            u16::from_be_bytes(inbound[0].bytes[36..38].try_into().unwrap()),
            1234
        );
    }

    #[test]
    fn firewall_and_nat_associate_later_ipv4_fragments() {
        let mut firewall_set = ExtensionSet::default();
        firewall_set.install(firewall(1).unwrap()).unwrap();
        let mut first = vec![ipv4_fragment(0x2000, true)];
        firewall_set.process(Hook::Firewall, &mut first);
        assert_eq!(first[0].disposition, PacketDisposition::Continue);
        let mut later = vec![ipv4_fragment(1, false)];
        firewall_set.process(Hook::Firewall, &mut later);
        assert_eq!(later[0].disposition, PacketDisposition::Continue);

        let mut config = vec![0u8; 48];
        config[..4].copy_from_slice(b"NAT1");
        config[4..8].copy_from_slice(&[203, 0, 113, 5]);
        config[8..10].copy_from_slice(&40000u16.to_le_bytes());
        config[10..12].copy_from_slice(&40010u16.to_le_bytes());
        let mut nat_set = ExtensionSet::default();
        nat_set.replace_nat_pair(nat(&config, 1).unwrap()).unwrap();
        let mut first = vec![ipv4_fragment(0x2000, true)];
        nat_set.process(Hook::PostRouting, &mut first);
        assert_eq!(&first[0].bytes[26..30], &[203, 0, 113, 5]);
        let mut later = vec![ipv4_fragment(1, false)];
        nat_set.process(Hook::PostRouting, &mut later);
        assert_eq!(&later[0].bytes[26..30], &[203, 0, 113, 5]);
    }

    #[test]
    fn nptv6_translation_preserves_pseudo_header_sum() {
        let mut config = vec![0u8; 48];
        config[..4].copy_from_slice(b"NAT1");
        config[4..8].copy_from_slice(&[203, 0, 113, 5]);
        config[8..10].copy_from_slice(&40000u16.to_le_bytes());
        config[10..12].copy_from_slice(&40010u16.to_le_bytes());
        config[12] = 64;
        config[16..24].copy_from_slice(&[0xfd, 0, 0, 0, 0, 0, 0, 0]);
        config[32..40].copy_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 1, 0, 0]);
        let mut extensions = ExtensionSet::default();
        extensions
            .replace_nat_pair(nat(&config, 1).unwrap())
            .unwrap();
        let mut packets = vec![ipv6_udp_packet()];
        let before = address_sum(&packets[0].bytes[22..38]);
        extensions.process(Hook::PostRouting, &mut packets);
        assert_eq!(
            &packets[0].bytes[22..30],
            &[0x20, 0x01, 0x0d, 0xb8, 0, 1, 0, 0]
        );
        assert_eq!(address_sum(&packets[0].bytes[22..38]), before);
    }

    fn udp_packet(
        source: [u8; 4],
        destination: [u8; 4],
        source_port: u16,
        destination_port: u16,
        direction: Direction,
    ) -> Packet {
        let mut bytes = vec![0u8; 42];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&28u16.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&source);
        bytes[30..34].copy_from_slice(&destination);
        bytes[34..36].copy_from_slice(&source_port.to_be_bytes());
        bytes[36..38].copy_from_slice(&destination_port.to_be_bytes());
        bytes[38..40].copy_from_slice(&8u16.to_be_bytes());
        let checksum = ipv4_checksum(&bytes[14..34]);
        bytes[24..26].copy_from_slice(&checksum.to_be_bytes());
        Packet::parse(&bytes, 1, direction, 1, 1).unwrap()
    }

    fn ipv4_checksum(bytes: &[u8]) -> u16 {
        let mut sum = bytes
            .chunks(2)
            .map(|word| u32::from(u16::from_be_bytes([word[0], *word.get(1).unwrap_or(&0)])))
            .sum::<u32>();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn ipv4_fragment(fragment_word: u16, first: bool) -> Packet {
        let payload_len = 8;
        let mut bytes = vec![0u8; 14 + 20 + payload_len];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&(28u16).to_be_bytes());
        bytes[18..20].copy_from_slice(&77u16.to_be_bytes());
        bytes[20..22].copy_from_slice(&fragment_word.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&[10, 0, 0, 2]);
        bytes[30..34].copy_from_slice(&[198, 51, 100, 9]);
        if first {
            bytes[34..36].copy_from_slice(&1234u16.to_be_bytes());
            bytes[36..38].copy_from_slice(&53u16.to_be_bytes());
            bytes[38..40].copy_from_slice(&16u16.to_be_bytes());
        } else {
            bytes[34..].fill(0x5a);
        }
        let checksum = ipv4_checksum(&bytes[14..34]);
        bytes[24..26].copy_from_slice(&checksum.to_be_bytes());
        Packet::parse(&bytes, 1, Direction::VirtualIngress, 1, 1).unwrap()
    }

    fn ipv6_udp_packet() -> Packet {
        let mut bytes = vec![0u8; 14 + 40 + 8];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&0x86ddu16.to_be_bytes());
        bytes[14] = 0x60;
        bytes[18..20].copy_from_slice(&8u16.to_be_bytes());
        bytes[20] = 17;
        bytes[21] = 64;
        bytes[22..38].copy_from_slice(&[
            0xfd, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        ]);
        bytes[38..54].copy_from_slice(&[0x20, 1, 0x0d, 0xb8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        bytes[54..56].copy_from_slice(&1234u16.to_be_bytes());
        bytes[56..58].copy_from_slice(&53u16.to_be_bytes());
        bytes[58..60].copy_from_slice(&8u16.to_be_bytes());
        bytes[60..62].copy_from_slice(&0x1234u16.to_be_bytes());
        Packet::parse(&bytes, 1, Direction::VirtualIngress, 1, 1).unwrap()
    }

    fn address_sum(bytes: &[u8]) -> u16 {
        bytes.chunks_exact(2).fold(0u16, |sum, word| {
            let value = u16::from_be_bytes([word[0], word[1]]);
            let total = u32::from(sum) + u32::from(value);
            (total as u16).wrapping_add((total >> 16) as u16)
        })
    }
}
