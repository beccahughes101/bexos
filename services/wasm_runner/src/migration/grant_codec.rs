use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_wasm_runtime::resources::Grant;
pub fn estimate(grant: Option<&Grant>) -> usize {
    grant.map_or(8, |g| {
        128 + g.service.len()
            + g.protocol.len()
            + g.capability.len()
            + g.method_ordinals.len() * 8
            + g.permission_values
                .iter()
                .map(|v| v.len() + 8)
                .sum::<usize>()
            + g.caller_package.as_ref().map_or(0, String::len)
    })
}
pub fn write(e: &mut Encoder, grant: Option<&Grant>) {
    e.word(grant.is_some() as u64);
    let Some(g) = grant else { return };
    e.text(&g.service);
    e.text(&g.protocol);
    e.text(&g.capability);
    e.word(g.method_ordinals.len() as u64);
    for n in &g.method_ordinals {
        e.word(*n);
    }
    e.word(g.permission_values.len() as u64);
    for v in &g.permission_values {
        e.text(v);
    }
    e.word(g.caller_package.is_some() as u64);
    if let Some(v) = &g.caller_package {
        e.text(v);
    }
    e.word(g.caller_uid.is_some() as u64);
    if let Some(v) = g.caller_uid {
        e.word(v);
    }
    e.word(g.caller_foreground as u64);
}
pub fn read(d: &mut Decoder<'_>) -> Result<Option<Grant>, Error> {
    if !d.flag()? {
        return Ok(None);
    }
    let service = d.text(4096)?.into();
    let protocol = d.text(4096)?.into();
    let capability = d.text(4096)?.into();
    let mut method_ordinals = Vec::new();
    for _ in 0..d.count(256)? {
        method_ordinals.push(d.word()?);
    }
    let mut permission_values = Vec::new();
    for _ in 0..d.count(256)? {
        permission_values.push(d.text(4096)?.into());
    }
    let caller_package = if d.flag()? {
        Some(d.text(4096)?.into())
    } else {
        None
    };
    let caller_uid = if d.flag()? { Some(d.word()?) } else { None };
    let caller_foreground = d.flag()?;
    Ok(Some(Grant {
        service,
        protocol,
        capability,
        method_ordinals,
        permission_values,
        caller_package,
        caller_uid,
        caller_foreground,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_authenticated_grant_round_trips() {
        let grant = Grant {
            service: "net".into(),
            protocol: "udp".into(),
            capability: "public".into(),
            method_ordinals: vec![1, 3],
            permission_values: vec!["loopback".into()],
            caller_package: Some("test.client".into()),
            caller_uid: Some(42),
            caller_foreground: false,
        };
        let mut e = Encoder::new();
        write(&mut e, Some(&grant));
        let bytes = e.finish();
        let mut d = Decoder::new(&bytes);
        assert_eq!(read(&mut d).unwrap(), Some(grant));
        d.finish().unwrap();
        assert!(read(&mut Decoder::new(&bytes[..bytes.len() - 1])).is_err());
    }
}
