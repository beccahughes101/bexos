//! Canonical DMA ownership records shared by full and incremental snapshots.
use super::{DmaMapping, IommuDomain};
use crate::transplant::{
    TransplantError,
    codec::{Reader, Result, Writer},
};

pub(super) fn write_domain(w: &mut Writer<'_>, value: Option<IommuDomain>) -> Result<()> {
    w.word(value.is_some() as u64)?;
    if let Some(domain) = value {
        for word in [
            domain.owner as u64,
            domain.stream_id,
            domain.address_width as u64,
            domain.refs as u64,
            domain.closed as u64,
        ] {
            w.word(word)?;
        }
    }
    Ok(())
}

pub(super) fn read_domain(r: &mut Reader<'_>) -> Result<Option<IommuDomain>> {
    if !r.flag()? {
        return Ok(None);
    }
    let domain = IommuDomain {
        owner: r.index()?,
        stream_id: r.word()?,
        address_width: u8::try_from(r.word()?)
            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?,
        refs: r.index()?,
        closed: r.flag()?,
    };
    if domain.stream_id == 0 || !(32..=48).contains(&domain.address_width) {
        return Err(TransplantError::InvalidRuntimeSnapshot);
    }
    Ok(Some(domain))
}

pub(super) fn write_mapping(w: &mut Writer<'_>, value: Option<DmaMapping>) -> Result<()> {
    w.word(value.is_some() as u64)?;
    if let Some(mapping) = value {
        for word in [
            mapping.owner as u64,
            mapping.domain as u64,
            mapping.vmo as u64,
            mapping.offset,
            mapping.length,
            mapping.permissions as u64,
            mapping.device_address,
        ] {
            w.word(word)?;
        }
    }
    Ok(())
}

pub(super) fn read_mapping(r: &mut Reader<'_>) -> Result<Option<DmaMapping>> {
    if !r.flag()? {
        return Ok(None);
    }
    let mapping = DmaMapping {
        owner: r.index()?,
        domain: r.index()?,
        vmo: r.index()?,
        offset: r.word()?,
        length: r.word()?,
        permissions: u32::try_from(r.word()?)
            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?,
        device_address: r.word()?,
    };
    if mapping.length == 0
        || mapping.permissions == 0
        || mapping.permissions & !6 != 0
        || mapping.offset % 4096 != 0
        || mapping.length % 4096 != 0
        || mapping.device_address % 4096 != 0
    {
        return Err(TransplantError::InvalidRuntimeSnapshot);
    }
    Ok(Some(mapping))
}
