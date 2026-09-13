use bexos_userspace::{Channel, Memory, Rpc, log};
use fonts_fidl::{
    FidlDecode, FidlEncode, FontDescriptor, FontFormat, FontProviderResolveFontRequest,
    FontProviderResolveFontResponse, FontStatus, FontStyle, HandleRef,
};
fn resolve(channel: Channel, family: &str, allow: bool, expected: FontStatus) {
    let request = FontProviderResolveFontRequest {
        query: FontDescriptor {
            family_name: family,
            weight: 400,
            style: FontStyle::Normal,
            format_preference: FontFormat::Truetype,
        },
        allow_network_fetch: allow,
    };
    let mut bytes = [0; 1024];
    let encoded = request.encode(&mut bytes, &mut []).unwrap();
    let reply = Rpc(channel)
        .call_raw(1, &bytes[..encoded.bytes], &[], true)
        .unwrap();
    let handles: Vec<_> = reply
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect();
    let response = FontProviderResolveFontResponse::decode(&reply.bytes, &handles).unwrap();
    assert_eq!(response.status, expected, "font {family}, allow={allow}");
    if expected == FontStatus::Ok {
        assert_eq!(handles.len(), 1);
        let font = response.font.get(0).unwrap();
        assert!(font.data_len > 1000);
        assert!(Memory::map(font.data.raw, font.data_len, kernel_fidl::Rights::WRITE.0).is_err());
    } else {
        assert!(handles.is_empty());
    }
    for handle in handles {
        Memory::close(handle.raw).unwrap();
    }
}
pub fn check(channel: Channel, first_boot: bool) {
    resolve(channel, "Inter", false, FontStatus::Ok);
    if first_boot {
        resolve(channel, "Probe", false, FontStatus::NotFound);
    }
    resolve(channel, "Probe", true, FontStatus::Ok);
    resolve(channel, "Probe", false, FontStatus::Ok);
    log("pkg-probe: local and remote immutable fonts verified\n");
    Memory::close(channel.0).unwrap();
}
