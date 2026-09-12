use alloc::collections::BTreeSet;
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use fonts_fidl::{FontFormat, FontStyle};
use skrifa::{FontRef, MetadataProvider, attribute::Style, string::StringId};

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FACES: usize = 16;
const MAX_TABLES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaceMetadata {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    pub format: FontFormat,
    pub index: u32,
    pub scripts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Unsupported,
    Bounds,
}

pub fn parse(bytes: &[u8]) -> Result<Vec<FaceMetadata>, Error> {
    if bytes.len() < 12 || bytes.len() > MAX_FONT_BYTES {
        return Err(Error::Bounds);
    }
    if bytes.starts_with(b"wOF2") {
        return Err(Error::Unsupported);
    }
    let offsets = face_offsets(bytes)?;
    if offsets.is_empty() || offsets.len() > MAX_FACES {
        return Err(Error::Bounds);
    }
    for offset in &offsets {
        validate_face(bytes, *offset)?;
    }
    let collection = offsets.len() > 1 || bytes.starts_with(b"ttcf");
    offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| metadata(bytes, index, *offset, collection))
        .collect()
}

fn face_offsets(bytes: &[u8]) -> Result<Vec<usize>, Error> {
    if !bytes.starts_with(b"ttcf") {
        return Ok(vec![0]);
    }
    let count = be_u32(bytes, 8)? as usize;
    if count == 0
        || count > MAX_FACES
        || 12usize.checked_add(count * 4).ok_or(Error::Bounds)? > bytes.len()
    {
        return Err(Error::Bounds);
    }
    (0..count)
        .map(|index| be_u32(bytes, 12 + index * 4).map(|v| v as usize))
        .collect()
}

fn validate_face(bytes: &[u8], face: usize) -> Result<(), Error> {
    let header = bytes
        .get(face..face.checked_add(12).ok_or(Error::Bounds)?)
        .ok_or(Error::Bounds)?;
    if !matches!(&header[..4], b"\0\x01\0\0" | b"OTTO" | b"true") {
        return Err(Error::Unsupported);
    }
    let count = u16::from_be_bytes(header[4..6].try_into().unwrap()) as usize;
    if count == 0 || count > MAX_TABLES {
        return Err(Error::Bounds);
    }
    let end = face.checked_add(12 + count * 16).ok_or(Error::Bounds)?;
    if end > bytes.len() {
        return Err(Error::Bounds);
    }
    let mut ranges = Vec::with_capacity(count);
    let mut tags = BTreeSet::new();
    let mut required = 0u8;
    for index in 0..count {
        let record = &bytes[face + 12 + index * 16..face + 28 + index * 16];
        let tag: [u8; 4] = record[..4].try_into().unwrap();
        if !tags.insert(tag) {
            return Err(Error::Invalid);
        }
        required |= match &tag {
            b"head" => 1,
            b"cmap" => 2,
            b"name" => 4,
            b"maxp" => 8,
            _ => 0,
        };
        let start = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
        let len = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
        let finish = start.checked_add(len).ok_or(Error::Bounds)?;
        if len == 0 || finish > bytes.len() {
            return Err(Error::Bounds);
        }
        for &(prior_start, prior_finish) in &ranges {
            let overlap = start < prior_finish && prior_start < finish;
            if overlap {
                return Err(Error::Invalid);
            }
        }
        ranges.push((start, finish));
    }
    if required != 0b1111 {
        return Err(Error::Invalid);
    }
    Ok(())
}

fn metadata(
    bytes: &[u8],
    index: usize,
    face: usize,
    collection: bool,
) -> Result<FaceMetadata, Error> {
    let font = FontRef::from_index(bytes, index as u32).map_err(|_| Error::Invalid)?;
    let family = font
        .localized_strings(StringId::new(16))
        .english_or_first()
        .or_else(|| font.localized_strings(StringId::new(1)).english_or_first())
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty() && value.len() <= 64)
        .ok_or(Error::Invalid)?;
    let attributes = font.attributes();
    let style = match attributes.style {
        Style::Normal => FontStyle::Normal,
        Style::Italic => FontStyle::Italic,
        Style::Oblique(_) => FontStyle::Oblique,
    };
    let format = if collection {
        FontFormat::Collection
    } else if bytes.get(face..face + 4) == Some(b"OTTO") {
        FontFormat::Opentype
    } else {
        FontFormat::Truetype
    };
    let charmap = font.charmap();
    let mut scripts = 0;
    if ['A', 'z', '\u{00e9}']
        .into_iter()
        .any(|ch| charmap.map(ch).is_some())
    {
        scripts |= 1;
    }
    if ['\u{0627}', '\u{0645}', '\u{064a}']
        .into_iter()
        .any(|ch| charmap.map(ch).is_some())
    {
        scripts |= 2;
    }
    if ['\u{0905}', '\u{0915}', '\u{0939}']
        .into_iter()
        .any(|ch| charmap.map(ch).is_some())
    {
        scripts |= 4;
    }
    Ok(FaceMetadata {
        family,
        weight: attributes.weight.value().round().clamp(1.0, 1000.0) as u16,
        style,
        format,
        index: index as u32,
        scripts,
    })
}

fn be_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Bounds)?
            .try_into()
            .unwrap(),
    ))
}
