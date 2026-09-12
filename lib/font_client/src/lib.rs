#![no_std]

extern crate alloc;

mod mapped;

pub use mapped::MappedFont;

use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
use bexos_userspace::{Channel, Memory, Rpc};
use fonts_fidl::{
    FidlDecode, FidlEncode, FontDescriptor, FontFormat, FontHandle,
    FontProviderGetFallbackListRequest, FontProviderGetFallbackListResponse,
    FontProviderResolveFontRequest, FontProviderResolveFontResponse, FontStatus, FontStyle,
    HandleRef,
};

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const RIGHT_TRANSFER: u32 = 1;
pub(crate) const RIGHT_READ: u32 = 2;
pub(crate) const RIGHT_WRITE: u32 = 4;
pub(crate) const RIGHT_EXECUTE: u32 = 8;
pub(crate) const RIGHT_MAP: u32 = 16;
pub(crate) const RIGHT_DUPLICATE: u32 = 32;

pub fn acceptable_vmo_rights(rights: u32) -> bool {
    rights & (RIGHT_TRANSFER | RIGHT_READ | RIGHT_MAP) == (RIGHT_TRANSFER | RIGHT_READ | RIGHT_MAP)
        && rights & (RIGHT_WRITE | RIGHT_EXECUTE | RIGHT_DUPLICATE) == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotFound,
    AccessDenied,
    InvalidRequest,
    UnsupportedFormat,
    Storage,
    ResourceExhausted,
    Busy,
    InvalidResponse,
    InvalidRights,
}

#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub family: &'a str,
    pub weight: u16,
    pub style: FontStyle,
    pub format: FontFormat,
    pub allow_network_fetch: bool,
}

impl<'a> Request<'a> {
    pub fn sans(family: &'a str) -> Self {
        Self {
            family,
            weight: 400,
            style: FontStyle::Normal,
            format: FontFormat::Truetype,
            allow_network_fetch: false,
        }
    }
}

pub struct Client {
    endpoint: Channel,
    cache: BTreeMap<(u64, u32), Arc<MappedFont>>,
}

impl Client {
    pub fn new(endpoint: u64) -> Self {
        Self {
            endpoint: Channel(endpoint),
            cache: BTreeMap::new(),
        }
    }

    pub fn resolve(&mut self, request: Request<'_>) -> Result<Arc<MappedFont>, Error> {
        if request.family.is_empty()
            || request.family.len() > 64
            || !(1..=1000).contains(&request.weight)
        {
            return Err(Error::InvalidRequest);
        }
        let query = FontDescriptor {
            family_name: request.family,
            weight: request.weight,
            style: request.style,
            format_preference: request.format,
        };
        let (bytes, handles) = call_raw(
            self.endpoint,
            1,
            &FontProviderResolveFontRequest {
                query,
                allow_network_fetch: request.allow_network_fetch,
            },
        )?;
        let refs = handles
            .iter()
            .map(|raw| HandleRef { raw: *raw })
            .collect::<Vec<_>>();
        let response = FontProviderResolveFontResponse::decode(&bytes, &refs).map_err(|_| {
            close_all(&handles);
            Error::InvalidResponse
        })?;
        if let Err(error) = check(response.status) {
            close_all(&handles);
            return Err(error);
        }
        if response.font.len() != 1 || handles.len() != 1 {
            close_all(&handles);
            return Err(Error::InvalidResponse);
        }
        let font = response.font.get(0).map_err(|_| {
            close_all(&handles);
            Error::InvalidResponse
        })?;
        self.adopt(font)
    }

    pub fn fallback(&mut self, script: &str) -> Result<Vec<Arc<MappedFont>>, Error> {
        if script.len() != 4 {
            return Err(Error::InvalidRequest);
        }
        let response_bytes = call_raw(
            self.endpoint,
            2,
            &FontProviderGetFallbackListRequest { script },
        )?;
        let refs = response_bytes
            .1
            .iter()
            .map(|raw| HandleRef { raw: *raw })
            .collect::<Vec<_>>();
        let response = FontProviderGetFallbackListResponse::decode(&response_bytes.0, &refs)
            .map_err(|_| {
                close_all(&response_bytes.1);
                Error::InvalidResponse
            })?;
        if let Err(error) = check(response.status) {
            close_all(&response_bytes.1);
            return Err(error);
        }
        if response.fonts.len() != response_bytes.1.len() {
            close_all(&response_bytes.1);
            return Err(Error::InvalidResponse);
        }
        let mut received = Vec::with_capacity(response.fonts.len());
        for index in 0..response.fonts.len() {
            let font = response.fonts.get(index).map_err(|_| {
                close_all(&response_bytes.1);
                Error::InvalidResponse
            })?;
            received.push(font);
        }
        let mut fonts = Vec::new();
        for (index, font) in received.iter().copied().enumerate() {
            match self.adopt(font) {
                Ok(font) => fonts.push(font),
                Err(error) => {
                    for font in &received[index + 1..] {
                        let _ = Memory::close(font.data.raw);
                    }
                    return Err(error);
                }
            }
        }
        Ok(fonts)
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
    pub fn cached_fonts(&self) -> usize {
        self.cache.len()
    }

    fn adopt(&mut self, font: FontHandle) -> Result<Arc<MappedFont>, Error> {
        let key = (font.font_id, font.index_in_collection);
        if font.font_id == 0 || font.data.raw == 0 {
            if font.data.raw != 0 {
                let _ = Memory::close(font.data.raw);
            }
            return Err(Error::InvalidResponse);
        }
        if let Some(existing) = self.cache.get(&key) {
            let _ = Memory::close(font.data.raw);
            return Ok(existing.clone());
        }
        let mapped = Arc::new(MappedFont::new(
            font.data.raw,
            font.data_len,
            font.font_id,
            font.index_in_collection,
        )?);
        self.cache.insert(key, mapped.clone());
        Ok(mapped)
    }
}

fn call_raw<Q: FidlEncode>(
    channel: Channel,
    ordinal: u64,
    request: &Q,
) -> Result<(Vec<u8>, Vec<u64>), Error> {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 8];
    let encoded = request
        .encode(&mut bytes, &mut handles)
        .map_err(|_| Error::InvalidRequest)?;
    let sent = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    let message = Rpc(channel)
        .call_raw(ordinal, &bytes[..encoded.bytes], &sent, true)
        .map_err(|_| Error::Storage)?;
    Ok((message.bytes, message.handles))
}

fn check(status: FontStatus) -> Result<(), Error> {
    match status {
        FontStatus::Ok => Ok(()),
        FontStatus::NotFound => Err(Error::NotFound),
        FontStatus::AccessDenied => Err(Error::AccessDenied),
        FontStatus::InvalidArgs => Err(Error::InvalidRequest),
        FontStatus::UnsupportedFormat => Err(Error::UnsupportedFormat),
        FontStatus::Storage => Err(Error::Storage),
        FontStatus::ResourceExhausted => Err(Error::ResourceExhausted),
        FontStatus::Busy => Err(Error::Busy),
    }
}

fn close_all(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}
