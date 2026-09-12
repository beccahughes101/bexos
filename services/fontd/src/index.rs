use crate::parser::FaceMetadata;
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use fonts_fidl::{FontFormat, FontStyle};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Scope {
    System,
    User(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Face {
    pub id: u64,
    pub digest: [u8; 32],
    pub metadata: FaceMetadata,
    pub scope: Scope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query {
    pub family: String,
    pub weight: u16,
    pub style: FontStyle,
    pub format: FontFormat,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    faces: Vec<Face>,
}

impl Index {
    pub fn insert(
        &mut self,
        scope: Scope,
        digest: [u8; 32],
        metadata: &[FaceMetadata],
    ) -> Result<Vec<u64>, ()> {
        let mut ids = Vec::new();
        for item in metadata {
            let id = font_id(&digest, item.index);
            if self.faces.iter().any(|face| {
                face.id == id && (face.digest != digest || face.metadata.index != item.index)
            }) {
                return Err(());
            }
            if !self.faces.iter().any(|face| {
                face.scope == scope && face.digest == digest && face.metadata.index == item.index
            }) {
                self.faces.push(Face {
                    id,
                    digest,
                    metadata: item.clone(),
                    scope,
                });
            }
            ids.push(id);
        }
        Ok(ids)
    }

    pub fn remove_user(&mut self, uid: u64) {
        self.faces.retain(|face| face.scope != Scope::User(uid));
    }

    pub fn resolve(&self, uid: u64, query: &Query) -> Option<&Face> {
        let family = normalize_family(&query.family)?;
        self.faces
            .iter()
            .filter(|face| {
                (face.scope == Scope::System || face.scope == Scope::User(uid))
                    && normalize_family(&face.metadata.family).as_deref() == Some(family.as_str())
            })
            .min_by_key(|face| rank(face, uid, query))
    }

    pub fn fallbacks(&self, uid: u64, script: &str) -> Option<Vec<&Face>> {
        let (bit, baseline): (u8, &[&str]) = match script {
            "Latn" => (1, &["Inter", "Noto Sans"]),
            "Arab" => (2, &["Noto Sans Arabic", "Noto Sans"]),
            "Deva" => (4, &["Noto Sans Devanagari", "Noto Sans"]),
            _ => return None,
        };
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut users: Vec<_> = self
            .faces
            .iter()
            .filter(|face| face.scope == Scope::User(uid) && face.metadata.scripts & bit != 0)
            .collect();
        users.sort_by_key(|face| (normalize_family(&face.metadata.family), face.id));
        let system = baseline.iter().filter_map(|family| {
            let query = Query {
                family: (*family).into(),
                weight: 400,
                style: FontStyle::Normal,
                format: FontFormat::Truetype,
            };
            self.faces
                .iter()
                .filter(|face| {
                    face.scope == Scope::System
                        && normalize_family(&face.metadata.family) == normalize_family(family)
                })
                .min_by_key(|face| rank(face, uid, &query))
        });
        for face in users.into_iter().chain(system) {
            if seen.insert(face.id) {
                out.push(face);
            }
            if out.len() == 8 {
                break;
            }
        }
        Some(out)
    }

    pub fn faces(&self) -> &[Face] {
        &self.faces
    }
    pub fn replace(&mut self, faces: Vec<Face>) {
        self.faces = faces;
    }
}

fn rank(face: &Face, uid: u64, query: &Query) -> (u8, u8, u16, u8, u8, u64) {
    let scope = if face.scope == Scope::User(uid) { 0 } else { 1 };
    let style = if face.metadata.style == query.style {
        0
    } else if face.metadata.style == FontStyle::Normal {
        1
    } else {
        2
    };
    let distance = face.metadata.weight.abs_diff(query.weight.clamp(1, 1000));
    let weight_tie = if query.weight <= 500 {
        (face.metadata.weight > query.weight) as u8
    } else {
        (face.metadata.weight < query.weight) as u8
    };
    let format = (face.metadata.format != query.format) as u8;
    (scope, style, distance, weight_tie, format, face.id)
}

pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn font_id(digest: &[u8; 32], index: u32) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"bexos-font-face-v1");
    hasher.update(digest);
    hasher.update(index.to_le_bytes());
    u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap()).max(1)
}

pub fn normalize_family(value: &str) -> Option<String> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (!normalized.is_empty() && normalized.len() <= 64).then_some(normalized)
}
