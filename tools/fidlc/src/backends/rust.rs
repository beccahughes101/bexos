use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{self, PrimitiveType, TypeKind, TypeRef};
use crate::capability::CapabilityGroup;
use crate::ir::{self, WIRE_ABI_VERSION};

pub fn generate(library: &ir::Library, groups: &[CapabilityGroup]) -> String {
    let mut out = String::new();
    let ctx = RustContext::new(library);

    emit_prelude(&mut out, library);
    emit_runtime_hooks(&mut out);
    emit_declarations(&mut out, library, &ctx);
    emit_protocol_metadata(&mut out, library);
    emit_capability_metadata(&mut out, groups);
    emit_payload_types(&mut out, library, &ctx);
    emit_protocols(&mut out, library, groups, &ctx);

    out
}

struct RustContext {
    aliases: BTreeMap<String, TypeRef>,
    bits_reprs: BTreeMap<String, PrimitiveType>,
    enum_reprs: BTreeMap<String, PrimitiveType>,
    lifetime_aliases: BTreeSet<String>,
    struct_like: BTreeSet<String>,
    struct_lifetimes: BTreeSet<String>,
    type_names: BTreeMap<String, String>,
    union_like: BTreeSet<String>,
    union_lifetimes: BTreeSet<String>,
    usings: Vec<String>,
}

impl RustContext {
    fn new(library: &ir::Library) -> Self {
        let mut aliases = BTreeMap::new();
        let mut bits_reprs = BTreeMap::new();
        let mut enum_reprs = BTreeMap::new();
        let mut struct_like = BTreeSet::new();
        let mut lifetime_aliases = BTreeSet::new();
        let mut type_names = BTreeMap::new();
        let mut pending_structs = Vec::new();
        let mut pending_tables = Vec::new();
        let mut pending_unions = Vec::new();
        let mut union_like = BTreeSet::new();
        for decl in &library.declarations {
            match decl {
                ir::Decl::Alias(decl) => {
                    if type_needs_direct_lifetime(&decl.ty) {
                        lifetime_aliases.insert(decl.name.clone());
                    }
                    aliases.insert(decl.name.clone(), decl.ty.clone());
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
                ir::Decl::Bits(decl) => {
                    bits_reprs.insert(decl.name.clone(), int_repr(decl.ty.as_ref()));
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
                ir::Decl::Enum(decl) => {
                    enum_reprs.insert(decl.name.clone(), int_repr(decl.ty.as_ref()));
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
                ir::Decl::Struct(decl) => {
                    struct_like.insert(decl.name.clone());
                    pending_structs.push(decl);
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
                ir::Decl::Table(decl) => {
                    struct_like.insert(decl.name.clone());
                    pending_tables.push(decl);
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
                ir::Decl::Union(decl) => {
                    union_like.insert(decl.name.clone());
                    pending_unions.push(decl);
                    type_names.insert(decl.name.clone(), to_pascal_case(&decl.name));
                }
            }
        }
        let mut struct_lifetimes = BTreeSet::new();
        for decl in pending_structs {
            if decl.fields.iter().any(|field| {
                type_needs_lifetime_with_sets(&field.ty, &struct_lifetimes, &lifetime_aliases)
            }) {
                struct_lifetimes.insert(decl.name.clone());
            }
        }
        for decl in pending_tables {
            if decl.fields.iter().any(|field| {
                type_needs_lifetime_with_sets(&field.ty, &struct_lifetimes, &lifetime_aliases)
            }) {
                struct_lifetimes.insert(decl.name.clone());
            }
        }
        let mut union_lifetimes = BTreeSet::new();
        for decl in pending_unions {
            if decl.members.iter().any(|member| {
                type_needs_lifetime_with_sets(&member.ty, &struct_lifetimes, &lifetime_aliases)
            }) {
                union_lifetimes.insert(decl.name.clone());
            }
        }
        Self {
            aliases,
            bits_reprs,
            enum_reprs,
            lifetime_aliases,
            struct_like,
            struct_lifetimes,
            type_names,
            union_like,
            union_lifetimes,
            usings: library.usings.clone(),
        }
    }

    fn type_name(&self, name: &str) -> String {
        if let Some((library, short)) = self.external_type(name) {
            return format!(
                "{}::{}",
                rust_crate_for_library(library),
                to_pascal_case(short)
            );
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        self.type_names
            .get(short)
            .cloned()
            .unwrap_or_else(|| to_pascal_case(short))
    }

    fn is_struct_like(&self, name: &str) -> bool {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.struct_like.contains(short)
    }

    fn is_union_like(&self, name: &str) -> bool {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.union_like.contains(short)
    }

    fn struct_needs_lifetime(&self, name: &str) -> bool {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.struct_lifetimes.contains(short)
    }

    fn union_needs_lifetime(&self, name: &str) -> bool {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.union_lifetimes.contains(short)
    }

    fn alias_needs_lifetime(&self, name: &str) -> bool {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.lifetime_aliases.contains(short)
    }

    fn alias_type(&self, name: &str) -> Option<&TypeRef> {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.aliases.get(short)
    }

    fn bits_repr(&self, name: &str) -> Option<PrimitiveType> {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.bits_reprs.get(short).copied()
    }

    fn enum_repr(&self, name: &str) -> Option<PrimitiveType> {
        let short = name.rsplit('.').next().unwrap_or(name);
        self.enum_reprs.get(short).copied()
    }

    fn external_type<'a>(&'a self, name: &'a str) -> Option<(&'a str, &'a str)> {
        let (library, short) = name.rsplit_once('.')?;
        self.usings
            .iter()
            .any(|using| using == library)
            .then_some((library, short))
    }
}

fn emit_prelude(out: &mut String, library: &ir::Library) {
    out.push_str("// @generated by bexos fidlc. Do not edit.\n");
    out.push_str("#![no_std]\n");
    out.push_str("#![allow(dead_code)]\n");
    out.push_str("#![allow(unreachable_code)]\n");
    out.push_str("#![allow(unused_mut)]\n");
    out.push_str("#![allow(unused_variables)]\n\n");
    out.push_str(&format!(
        "pub const FIDL_LIBRARY: &str = {};\n",
        rust_string(&library.name)
    ));
    out.push_str(&format!(
        "pub const BEXOS_FIDL_WIRE_ABI_VERSION: u32 = {};\n\n",
        WIRE_ABI_VERSION
    ));
}

fn emit_runtime_hooks(out: &mut String) {
    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str("pub struct HandleRef {\n");
    out.push_str("    pub raw: u64,\n");
    out.push_str("}\n\n");

    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str("pub struct EncodeResult {\n");
    out.push_str("    pub bytes: usize,\n");
    out.push_str("    pub handles: usize,\n");
    out.push_str("}\n\n");

    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str("pub enum FidlWireError {\n");
    out.push_str("    BufferTooSmall,\n");
    out.push_str("    HandleTableTooSmall,\n");
    out.push_str("    InvalidUtf8,\n");
    out.push_str("    InvalidPresence,\n");
    out.push_str("    UnsupportedType,\n");
    out.push_str("    UnknownOrdinal(u64),\n");
    out.push_str("    Transport,\n");
    out.push_str("}\n\n");

    out.push_str("pub trait FidlEncode {\n");
    out.push_str("    fn encode(&self, bytes: &mut [u8], handles: &mut [HandleRef]) -> Result<EncodeResult, FidlWireError>;\n");
    out.push_str("    fn encode_with_handle_offset(&self, bytes: &mut [u8], handles: &mut [HandleRef], handle_offset: usize) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str("        if handle_offset > handles.len() { return Err(FidlWireError::HandleTableTooSmall); }\n");
    out.push_str("        self.encode(bytes, &mut handles[handle_offset..])\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str("pub trait FidlDecode<'a>: Sized {\n");
    out.push_str("    fn decode(bytes: &'a [u8], handles: &'a [HandleRef]) -> Result<Self, FidlWireError>;\n");
    out.push_str("}\n\n");

    out.push_str("pub trait FidlTransport {\n");
    out.push_str("    fn call(&mut self, ordinal: u64, request: &[u8], request_handles: &[HandleRef], response: &mut [u8], response_handles: &mut [HandleRef]) -> Result<EncodeResult, FidlWireError>;\n");
    out.push_str("    fn send(&mut self, ordinal: u64, request: &[u8], request_handles: &[HandleRef]) -> Result<(), FidlWireError> {\n");
    out.push_str("        let mut response = [];\n        let mut response_handles = [];\n        self.call(ordinal, request, request_handles, &mut response, &mut response_handles).map(|_| ())\n    }\n");
    out.push_str("}\n\n");

    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str("pub struct MethodBinding {\n");
    out.push_str("    pub name: &'static str,\n");
    out.push_str("    pub ordinal: u64,\n");
    out.push_str("}\n\n");

    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str("pub struct CapabilityBinding {\n");
    out.push_str("    pub protocol: &'static str,\n");
    out.push_str("    pub capability: &'static str,\n");
    out.push_str("    pub permission: Option<&'static str>,\n");
    out.push_str("    pub methods: &'static [MethodBinding],\n");
    out.push_str("}\n\n");

    out.push_str("#[inline]\nfn put_u8(bytes: &mut [u8], offset: usize, value: u8) -> Result<(), FidlWireError> {\n");
    out.push_str(
        "    if offset + 1 > bytes.len() { return Err(FidlWireError::BufferTooSmall); }\n",
    );
    out.push_str("    bytes[offset] = value;\n    Ok(())\n}\n\n");
    out.push_str("#[inline]\nfn put_bytes(bytes: &mut [u8], offset: usize, value: &[u8]) -> Result<(), FidlWireError> {\n");
    out.push_str("    if offset + value.len() > bytes.len() { return Err(FidlWireError::BufferTooSmall); }\n");
    out.push_str(
        "    bytes[offset..offset + value.len()].copy_from_slice(value);\n    Ok(())\n}\n\n",
    );
    for (name, ty) in [
        ("u16", "u16"),
        ("u32", "u32"),
        ("u64", "u64"),
        ("i8", "i8"),
        ("i16", "i16"),
        ("i32", "i32"),
        ("i64", "i64"),
        ("f32", "f32"),
        ("f64", "f64"),
    ] {
        out.push_str(&format!("#[inline]\nfn put_{name}(bytes: &mut [u8], offset: usize, value: {ty}) -> Result<(), FidlWireError> {{\n"));
        out.push_str(&format!(
            "    put_bytes(bytes, offset, &value.to_le_bytes())\n}}\n\n"
        ));
    }
    for (name, ty, len) in [
        ("u8", "u8", 1usize),
        ("u16", "u16", 2usize),
        ("u32", "u32", 4),
        ("u64", "u64", 8),
        ("i8", "i8", 1),
        ("i16", "i16", 2),
        ("i32", "i32", 4),
        ("i64", "i64", 8),
        ("f32", "f32", 4),
        ("f64", "f64", 8),
    ] {
        out.push_str(&format!("#[inline]\nfn get_{name}(bytes: &[u8], offset: usize) -> Result<{ty}, FidlWireError> {{\n"));
        out.push_str(&format!(
            "    if offset + {len} > bytes.len() {{ return Err(FidlWireError::BufferTooSmall); }}\n"
        ));
        if name == "i8" {
            out.push_str("    Ok(bytes[offset] as i8)\n}\n\n");
        } else {
            out.push_str(&format!("    let mut raw = [0u8; {len}];\n"));
            out.push_str(&format!(
                "    raw.copy_from_slice(&bytes[offset..offset + {len}]);\n"
            ));
            out.push_str(&format!("    Ok({ty}::from_le_bytes(raw))\n}}\n\n"));
        }
    }
    out.push_str("#[inline]\nfn align8(value: usize) -> usize {\n    (value + 7) & !7\n}\n\n");
    out.push_str(r#"
/// Borrowed vector supporting nested wire values without an allocator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireVector<'a, T> {
    values: Option<&'a [T]>,
    bytes: &'a [u8],
    handles: &'a [HandleRef],
    count: usize,
}
impl<'a, T> WireVector<'a, T> {
    pub fn from_slice(values: &'a [T]) -> Self { Self { values: Some(values), bytes: &[], handles: &[], count: values.len() } }
    pub fn len(&self) -> usize { self.count }
    pub fn is_empty(&self) -> bool { self.count == 0 }
    fn from_wire(bytes: &'a [u8], handles: &'a [HandleRef], count: usize) -> Result<Self, FidlWireError> {
        if count.checked_mul(16).is_none_or(|n| n > bytes.len()) { return Err(FidlWireError::BufferTooSmall); }
        Ok(Self { values: None, bytes, handles, count })
    }
}
impl<'a, T: FidlDecode<'a> + Copy> WireVector<'a, T> {
    pub fn get(&self, index: usize) -> Result<T, FidlWireError> {
        if index >= self.count { return Err(FidlWireError::BufferTooSmall); }
        if let Some(values) = self.values { return Ok(values[index]); }
        let start = get_u64(self.bytes, index * 16)? as usize;
        let len = get_u64(self.bytes, index * 16 + 8)? as usize;
        let end = start.checked_add(len).ok_or(FidlWireError::BufferTooSmall)?;
        if start < self.count * 16 { return Err(FidlWireError::BufferTooSmall); }
        T::decode(self.bytes.get(start..end).ok_or(FidlWireError::BufferTooSmall)?, self.handles)
    }
}
impl<T: FidlEncode> WireVector<'_, T> {
    fn encode_wire(&self, out: &mut [u8], handles: &mut [HandleRef], handle_offset: usize) -> Result<EncodeResult, FidlWireError> {
        if let Some(values) = self.values {
            let mut cursor = values.len().checked_mul(16).ok_or(FidlWireError::BufferTooSmall)?;
            let mut handle_count = 0usize;
            if cursor > out.len() { return Err(FidlWireError::BufferTooSmall); }
            for (index, value) in values.iter().enumerate() {
                cursor = align8(cursor);
                let encoded = value.encode_with_handle_offset(out.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;
                put_u64(out, index * 16, cursor as u64)?;
                put_u64(out, index * 16 + 8, encoded.bytes as u64)?;
                cursor += encoded.bytes;
                handle_count += encoded.handles;
            }
            Ok(EncodeResult { bytes: cursor, handles: handle_count })
        } else {
            put_bytes(out, 0, self.bytes)?;
            Ok(EncodeResult { bytes: self.bytes.len(), handles: 0 })
        }
    }
}

/// Borrowed vector of borrowed UTF-8 strings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireStringVector<'a> {
    values: Option<&'a [&'a str]>,
    bytes: &'a [u8],
    count: usize,
}
impl<'a> WireStringVector<'a> {
    pub fn from_slice(values: &'a [&'a str]) -> Self { Self { values: Some(values), bytes: &[], count: values.len() } }
    pub fn len(&self) -> usize { self.count }
    pub fn is_empty(&self) -> bool { self.count == 0 }
    fn from_wire(bytes: &'a [u8], count: usize) -> Result<Self, FidlWireError> {
        if count.checked_mul(16).is_none_or(|n| n > bytes.len()) { return Err(FidlWireError::BufferTooSmall); }
        Ok(Self { values: None, bytes, count })
    }
    pub fn get(&self, index: usize) -> Result<&'a str, FidlWireError> {
        if index >= self.count { return Err(FidlWireError::BufferTooSmall); }
        if let Some(values) = self.values { return Ok(values[index]); }
        let start = get_u64(self.bytes, index * 16)? as usize;
        let len = get_u64(self.bytes, index * 16 + 8)? as usize;
        let end = start.checked_add(len).ok_or(FidlWireError::BufferTooSmall)?;
        if start < self.count * 16 { return Err(FidlWireError::BufferTooSmall); }
        core::str::from_utf8(self.bytes.get(start..end).ok_or(FidlWireError::BufferTooSmall)?).map_err(|_| FidlWireError::InvalidUtf8)
    }
    fn encode_wire(&self, out: &mut [u8]) -> Result<usize, FidlWireError> {
        if let Some(values) = self.values {
            let mut cursor = values.len().checked_mul(16).ok_or(FidlWireError::BufferTooSmall)?;
            if cursor > out.len() { return Err(FidlWireError::BufferTooSmall); }
            for (index, value) in values.iter().enumerate() {
                cursor = align8(cursor);
                put_u64(out, index * 16, cursor as u64)?;
                put_u64(out, index * 16 + 8, value.len() as u64)?;
                put_bytes(out, cursor, value.as_bytes())?;
                cursor += value.len();
            }
            Ok(cursor)
        } else {
            put_bytes(out, 0, self.bytes)?;
            Ok(self.bytes.len())
        }
    }
}

"#);
}

fn emit_declarations(out: &mut String, library: &ir::Library, ctx: &RustContext) {
    for decl in &library.declarations {
        match decl {
            ir::Decl::Alias(decl) => {
                out.push_str(&format!(
                    "pub type {}{} = {};\n\n",
                    ctx.type_name(&decl.name),
                    lifetime_decl(type_needs_lifetime(&decl.ty, ctx)),
                    rust_type(&decl.ty, ctx)
                ));
            }
            ir::Decl::Bits(decl) => emit_bits(out, decl, ctx),
            ir::Decl::Enum(decl) => emit_enum(out, decl, ctx),
            ir::Decl::Struct(decl) => {
                emit_struct(out, &ctx.type_name(&decl.name), &decl.fields, ctx)
            }
            ir::Decl::Table(decl) => emit_table(out, &ctx.type_name(&decl.name), &decl.fields, ctx),
            ir::Decl::Union(decl) => emit_union(out, decl, ctx),
        }
    }
}

fn emit_union(out: &mut String, decl: &ast::Union, ctx: &RustContext) {
    let name = ctx.type_name(&decl.name);
    let needs_lifetime = decl
        .members
        .iter()
        .any(|member| type_needs_lifetime(&member.ty, ctx));
    out.push_str("#[derive(Clone, Copy, Debug, PartialEq)]\n");
    out.push_str(&format!(
        "pub enum {name}{} {{\n",
        lifetime_decl(needs_lifetime)
    ));
    for member in &decl.members {
        out.push_str(&format!(
            "    {}({}),\n",
            to_pascal_case(&member.name),
            rust_type(&member.ty, ctx)
        ));
    }
    out.push_str("}\n\n");

    out.push_str(&format!(
        "impl{} FidlEncode for {name}{} {{\n",
        lifetime_impl(needs_lifetime),
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn encode(&self, bytes: &mut [u8], handles: &mut [HandleRef]) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str("        self.encode_with_handle_offset(bytes, handles, 0)\n");
    out.push_str("    }\n");
    out.push_str("    fn encode_with_handle_offset(&self, bytes: &mut [u8], handles: &mut [HandleRef], handle_offset: usize) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str("        let mut cursor = 24usize;\n");
    out.push_str("        let mut handle_count = 0usize;\n");
    out.push_str(
        "        if bytes.len() < cursor { return Err(FidlWireError::BufferTooSmall); }\n",
    );
    out.push_str("        match self {\n");
    for member in &decl.members {
        out.push_str(&format!(
            "            Self::{}(value) => {{\n",
            to_pascal_case(&member.name)
        ));
        out.push_str(&format!(
            "                put_u64(bytes, 0, {})?;\n",
            member.ordinal
        ));
        emit_encode_union_member(out, &member.ty, ctx);
        out.push_str("            }\n");
    }
    out.push_str("        }\n");
    out.push_str("        Ok(EncodeResult { bytes: cursor, handles: handle_count })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str(&format!(
        "impl<'a> FidlDecode<'a> for {name}{} {{\n",
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn decode(bytes: &'a [u8], handles: &'a [HandleRef]) -> Result<Self, FidlWireError> {\n");
    out.push_str("        if bytes.len() < 24 { return Err(FidlWireError::BufferTooSmall); }\n");
    out.push_str("        let ordinal = get_u64(bytes, 0)?;\n");
    out.push_str("        let start = get_u64(bytes, 8)? as usize;\n");
    out.push_str("        let len = get_u64(bytes, 16)? as usize;\n");
    out.push_str(
        "        let end = start.checked_add(len).ok_or(FidlWireError::BufferTooSmall)?;\n",
    );
    out.push_str(
        "        let payload = bytes.get(start..end).ok_or(FidlWireError::BufferTooSmall)?;\n",
    );
    out.push_str("        match ordinal {\n");
    for member in &decl.members {
        out.push_str(&format!(
            "            {} => Ok(Self::{}({})),\n",
            member.ordinal,
            to_pascal_case(&member.name),
            decode_union_member_expr(&member.ty, ctx)
        ));
    }
    out.push_str("            _ => Err(FidlWireError::UnknownOrdinal(ordinal)),\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

fn emit_enum(out: &mut String, decl: &ast::Enum, ctx: &RustContext) {
    let repr = decl
        .ty
        .as_ref()
        .map(|ty| rust_type(ty, ctx))
        .unwrap_or_else(|| "u32".to_string());
    out.push_str(&format!("#[repr({repr})]\n"));
    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str(&format!("pub enum {} {{\n", ctx.type_name(&decl.name)));
    for (index, member) in decl.members.iter().enumerate() {
        let value = member.value.unwrap_or(index as i64);
        out.push_str(&format!(
            "    {} = {},\n",
            to_pascal_case(&member.name),
            value
        ));
    }
    out.push_str("}\n\n");
    let name = ctx.type_name(&decl.name);
    out.push_str(&format!("impl {name} {{\n"));
    out.push_str(&format!(
        "    pub fn decode_value(value: {repr}) -> Result<Self, FidlWireError> {{\n"
    ));
    out.push_str("        match value {\n");
    for (index, member) in decl.members.iter().enumerate() {
        let value = member.value.unwrap_or(index as i64);
        out.push_str(&format!(
            "            {} => Ok(Self::{}),\n",
            value,
            to_pascal_case(&member.name)
        ));
    }
    out.push_str("            _ => Err(FidlWireError::InvalidPresence),\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

fn emit_bits(out: &mut String, decl: &ast::Bits, ctx: &RustContext) {
    let repr = decl
        .ty
        .as_ref()
        .map(|ty| rust_type(ty, ctx))
        .unwrap_or_else(|| "u32".to_string());
    let name = ctx.type_name(&decl.name);
    out.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    out.push_str(&format!("pub struct {name}(pub {repr});\n\n"));
    out.push_str(&format!("impl {name} {{\n"));
    for (index, member) in decl.members.iter().enumerate() {
        let value = member.value.unwrap_or(1_i64 << index);
        out.push_str(&format!(
            "    pub const {}: Self = Self({});\n",
            to_screaming_snake_case(&member.name),
            value
        ));
    }
    out.push_str("}\n\n");
}

fn emit_struct(out: &mut String, name: &str, fields: &[ast::Field], ctx: &RustContext) {
    let needs_lifetime = fields
        .iter()
        .any(|field| type_needs_lifetime(&field.ty, ctx));
    out.push_str("#[derive(Clone, Copy, Debug, PartialEq)]\n");
    out.push_str(&format!(
        "pub struct {name}{} {{\n",
        lifetime_decl(needs_lifetime)
    ));
    for field in fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake_case(&field.name),
            rust_type(&field.ty, ctx)
        ));
    }
    out.push_str("}\n\n");
    emit_codec_impl(out, name, fields, needs_lifetime, ctx);
}

fn emit_table(out: &mut String, name: &str, fields: &[ast::TableField], ctx: &RustContext) {
    let needs_lifetime = fields
        .iter()
        .any(|field| type_needs_lifetime(&field.ty, ctx));
    out.push_str("#[derive(Clone, Copy, Debug, PartialEq)]\n");
    out.push_str(&format!(
        "pub struct {name}{} {{\n",
        lifetime_decl(needs_lifetime)
    ));
    for field in fields {
        out.push_str(&format!(
            "    pub {}: Option<{}>,\n",
            to_snake_case(&field.name),
            rust_type(&field.ty, ctx)
        ));
    }
    out.push_str("}\n\n");
    emit_table_codec_impl(out, name, fields, needs_lifetime, ctx);
}

fn emit_table_codec_impl(
    out: &mut String,
    name: &str,
    fields: &[ast::TableField],
    needs_lifetime: bool,
    ctx: &RustContext,
) {
    let header_size = 8 + fields
        .iter()
        .map(|field| slot_size(&field.ty, ctx))
        .sum::<usize>();
    out.push_str(&format!(
        "impl{} FidlEncode for {name}{} {{\n",
        lifetime_impl(needs_lifetime),
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn encode(&self, bytes: &mut [u8], _handles: &mut [HandleRef]) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str(&format!(
        "        let cursor = {}usize;\n",
        align8_const(header_size)
    ));
    out.push_str(
        "        if bytes.len() < cursor { return Err(FidlWireError::BufferTooSmall); }\n",
    );
    out.push_str("        let mut presence = 0u64;\n");
    let mut offset = 8usize;
    for (index, field) in fields.iter().enumerate() {
        let access = format!("self.{}", to_snake_case(&field.name));
        out.push_str(&format!(
            "        if let Some(value) = {access} {{\n            presence |= 1u64 << {index};\n"
        ));
        emit_encode_scalar_value(out, &field.ty, "value", offset, ctx);
        out.push_str("        }\n");
        offset += slot_size(&field.ty, ctx);
    }
    out.push_str("        put_u64(bytes, 0, presence)?;\n");
    out.push_str("        Ok(EncodeResult { bytes: cursor, handles: 0 })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str(&format!(
        "impl<'a> FidlDecode<'a> for {name}{} {{\n",
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn decode(bytes: &'a [u8], _handles: &'a [HandleRef]) -> Result<Self, FidlWireError> {\n");
    out.push_str(&format!(
        "        if bytes.len() < {} {{ return Err(FidlWireError::BufferTooSmall); }}\n",
        align8_const(header_size)
    ));
    out.push_str("        let presence = get_u64(bytes, 0)?;\n");
    out.push_str("        Ok(Self {\n");
    let mut offset = 8usize;
    for (index, field) in fields.iter().enumerate() {
        out.push_str(&format!(
            "            {}: if presence & (1u64 << {index}) != 0 {{ Some({}) }} else {{ None }},\n",
            to_snake_case(&field.name),
            decode_expr(&field.ty, offset, ctx)
        ));
        offset += slot_size(&field.ty, ctx);
    }
    out.push_str("        })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

fn emit_protocol_metadata(out: &mut String, library: &ir::Library) {
    out.push_str("pub static PROTOCOLS: &[&str] = &[\n");
    for protocol in &library.protocols {
        out.push_str(&format!("    {},\n", rust_string(&protocol.name)));
    }
    out.push_str("];\n\n");
}

fn emit_capability_metadata(out: &mut String, groups: &[CapabilityGroup]) {
    for group in groups {
        out.push_str(&format!(
            "pub static {}_{}_METHODS: &[MethodBinding] = &[\n",
            to_screaming_snake_case(&group.protocol),
            to_screaming_snake_case(&group.capability)
        ));
        for method in &group.methods {
            out.push_str(&format!(
                "    MethodBinding {{ name: {}, ordinal: {} }},\n",
                rust_string(&method.name),
                method.ordinal
            ));
        }
        out.push_str("];\n\n");
    }

    out.push_str("pub static CAPABILITY_BINDINGS: &[CapabilityBinding] = &[\n");
    for group in groups {
        out.push_str("    CapabilityBinding {\n");
        out.push_str(&format!(
            "        protocol: {},\n",
            rust_string(&group.protocol)
        ));
        out.push_str(&format!(
            "        capability: {},\n",
            rust_string(&group.capability)
        ));
        match &group.permission {
            Some(permission) => out.push_str(&format!(
                "        permission: Some({}),\n",
                rust_string(permission)
            )),
            None => out.push_str("        permission: None,\n"),
        }
        out.push_str(&format!(
            "        methods: {}_{}_METHODS,\n",
            to_screaming_snake_case(&group.protocol),
            to_screaming_snake_case(&group.capability)
        ));
        out.push_str("    },\n");
    }
    out.push_str("];\n\n");
}

fn emit_payload_types(out: &mut String, library: &ir::Library, ctx: &RustContext) {
    let mut emitted = BTreeSet::new();
    for protocol in &library.protocols {
        for method in &protocol.methods {
            if emitted.insert(method.request.name.clone()) {
                emit_ir_payload(out, &method.request, ctx);
            }
            if emitted.insert(method.response.name.clone()) {
                emit_ir_payload(out, &method.response, ctx);
            }
        }
    }
}

fn emit_ir_payload(out: &mut String, payload: &ir::Payload, ctx: &RustContext) {
    let fields: Vec<ast::Field> = payload
        .fields
        .iter()
        .map(|field| ast::Field {
            name: field.name.clone(),
            ty: field.ty.clone(),
        })
        .collect();
    emit_struct(out, &to_pascal_case(&payload.name), &fields, ctx);
}

fn emit_protocols(
    out: &mut String,
    library: &ir::Library,
    groups: &[CapabilityGroup],
    ctx: &RustContext,
) {
    for group in groups {
        let trait_name = format!(
            "{}{}Server",
            to_pascal_case(&group.protocol),
            group.capability
        );
        let client_name = format!(
            "{}{}Client",
            to_pascal_case(&group.protocol),
            group.capability
        );

        out.push_str(&format!("pub trait {trait_name} {{\n"));
        for method_ref in &group.methods {
            let method = find_method(library, &group.protocol, &method_ref.name);
            if method.one_way {
                out.push_str(&format!(
                    "    fn {}<'a>(&mut self, request: {}{}) -> Result<(), FidlWireError>;\n",
                    to_snake_case(&method.name),
                    to_pascal_case(&method.request.name),
                    lifetime_use(payload_needs_lifetime(&method.request, ctx)),
                ));
            } else {
                out.push_str(&format!(
                    "    fn {}<'a>(&mut self, request: {}{}) -> Result<{}{}, FidlWireError>;\n",
                    to_snake_case(&method.name),
                    to_pascal_case(&method.request.name),
                    lifetime_use(payload_needs_lifetime(&method.request, ctx)),
                    to_pascal_case(&method.response.name),
                    lifetime_use(payload_needs_lifetime(&method.response, ctx))
                ));
            }
        }
        out.push_str("}\n\n");

        out.push_str(&format!("pub struct {client_name}<T> {{\n"));
        out.push_str("    transport: T,\n");
        out.push_str("}\n\n");
        out.push_str(&format!("impl<T> {client_name}<T> {{\n"));
        out.push_str(
            "    pub fn new(transport: T) -> Self {\n        Self { transport }\n    }\n\n",
        );
        out.push_str("    pub fn into_inner(self) -> T {\n        self.transport\n    }\n");
        out.push_str("}\n\n");

        out.push_str(&format!("impl<T: FidlTransport> {client_name}<T> {{\n"));
        for method_ref in &group.methods {
            let method = find_method(library, &group.protocol, &method_ref.name);
            if method.one_way {
                out.push_str(&format!(
                    "    pub fn {}(&mut self, request: &{}{}, request_bytes: &mut [u8], request_handles: &mut [HandleRef]) -> Result<(), FidlWireError> {{\n",
                    to_snake_case(&method.name),
                    to_pascal_case(&method.request.name),
                    lifetime_infer(payload_needs_lifetime(&method.request, ctx)),
                ));
                out.push_str(
                    "        let encoded = request.encode(request_bytes, request_handles)?;\n",
                );
                out.push_str(&format!(
                    "        self.transport.send({}, &request_bytes[..encoded.bytes], &request_handles[..encoded.handles])\n",
                    method.ordinal
                ));
                out.push_str("    }\n\n");
                continue;
            }
            out.push_str(&format!(
                "    pub fn {}<'a>(&mut self, request: &{}{}, request_bytes: &mut [u8], request_handles: &mut [HandleRef], response_bytes: &'a mut [u8], response_handles: &'a mut [HandleRef]) -> Result<{}{}, FidlWireError> {{\n",
                to_snake_case(&method.name),
                to_pascal_case(&method.request.name),
                lifetime_infer(payload_needs_lifetime(&method.request, ctx)),
                to_pascal_case(&method.response.name),
                lifetime_use(payload_needs_lifetime(&method.response, ctx))
            ));
            out.push_str(
                "        let encoded = request.encode(request_bytes, request_handles)?;\n",
            );
            out.push_str(&format!(
                "        let received = self.transport.call({}, &request_bytes[..encoded.bytes], &request_handles[..encoded.handles], response_bytes, response_handles)?;\n",
                method.ordinal
            ));
            out.push_str(&format!(
                "        {}::decode(&response_bytes[..received.bytes], &response_handles[..received.handles])\n",
                to_pascal_case(&method.response.name)
            ));
            out.push_str("    }\n\n");
        }
        out.push_str("}\n\n");
    }
}

fn emit_codec_impl(
    out: &mut String,
    name: &str,
    fields: &[ast::Field],
    needs_lifetime: bool,
    ctx: &RustContext,
) {
    let header_size = fields
        .iter()
        .map(|field| slot_size(&field.ty, ctx))
        .sum::<usize>();
    out.push_str(&format!(
        "impl{} FidlEncode for {name}{} {{\n",
        lifetime_impl(needs_lifetime),
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn encode(&self, bytes: &mut [u8], handles: &mut [HandleRef]) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str("        self.encode_with_handle_offset(bytes, handles, 0)\n");
    out.push_str("    }\n");
    out.push_str("    fn encode_with_handle_offset(&self, bytes: &mut [u8], handles: &mut [HandleRef], handle_offset: usize) -> Result<EncodeResult, FidlWireError> {\n");
    out.push_str(&format!(
        "        let mut cursor = {}usize;\n",
        align8_const(header_size)
    ));
    out.push_str("        let mut handle_count = 0usize;\n");
    out.push_str(
        "        if bytes.len() < cursor { return Err(FidlWireError::BufferTooSmall); }\n",
    );
    let mut offset = 0usize;
    for field in fields {
        emit_encode_field(out, field, offset, ctx);
        offset += slot_size(&field.ty, ctx);
    }
    out.push_str("        Ok(EncodeResult { bytes: cursor, handles: handle_count })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    out.push_str(&format!(
        "impl<'a> FidlDecode<'a> for {name}{} {{\n",
        lifetime_use(needs_lifetime)
    ));
    out.push_str("    fn decode(bytes: &'a [u8], handles: &'a [HandleRef]) -> Result<Self, FidlWireError> {\n");
    if header_size > 0 {
        out.push_str(&format!(
            "        if bytes.len() < {} {{ return Err(FidlWireError::BufferTooSmall); }}\n",
            align8_const(header_size)
        ));
    }
    out.push_str("        Ok(Self {\n");
    let mut offset = 0usize;
    for field in fields {
        out.push_str(&format!(
            "            {}: {},\n",
            to_snake_case(&field.name),
            decode_expr(&field.ty, offset, ctx)
        ));
        offset += slot_size(&field.ty, ctx);
    }
    out.push_str("        })\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

fn emit_encode_field(out: &mut String, field: &ast::Field, offset: usize, ctx: &RustContext) {
    if let TypeKind::Identifier(name) = &field.ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            let field = ast::Field {
                name: field.name.clone(),
                ty: ty.clone(),
            };
            emit_encode_field(out, &field, offset, ctx);
            return;
        }
    }
    let access = format!("self.{}", to_snake_case(&field.name));
    if field.ty.nullable {
        if let TypeKind::Identifier(name) = &field.ty.kind {
            if ctx.is_struct_like(name) || ctx.is_union_like(name) {
                out.push_str(&format!(
                    "        if let Some(value) = {access} {{\n        cursor = align8(cursor);\n        let nested = value.encode_with_handle_offset(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, nested.bytes as u64)?;\n        cursor += nested.bytes;\n        handle_count += nested.handles;\n        }} else {{\n        put_u64(bytes, {offset}, u64::MAX)?;\n        put_u64(bytes, {}, 0)?;\n        }}\n",
                    offset + 8,
                    offset + 8
                ));
                return;
            }
        }
    }
    match &field.ty.kind {
        TypeKind::Primitive(PrimitiveType::Bool) => {
            out.push_str(&format!(
                "        put_u8(bytes, {offset}, if {access} {{ 1 }} else {{ 0 }})?;\n"
            ));
        }
        TypeKind::Primitive(PrimitiveType::Uint8) => {
            out.push_str(&format!("        put_u8(bytes, {offset}, {access})?;\n"));
        }
        TypeKind::Primitive(primitive) => {
            out.push_str(&format!(
                "        put_{}(bytes, {offset}, {access})?;\n",
                primitive_fn(*primitive)
            ));
        }
        TypeKind::String(_) => {
            out.push_str(&format!("        cursor = align8(cursor);\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, {access}.len() as u64)?;\n        put_bytes(bytes, cursor, {access}.as_bytes())?;\n        cursor += {access}.len();\n", offset + 8));
        }
        TypeKind::Vector(element, _)
            if matches!(element.kind, TypeKind::Primitive(PrimitiveType::Uint8)) =>
        {
            out.push_str(&format!("        cursor = align8(cursor);\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, {access}.len() as u64)?;\n        put_bytes(bytes, cursor, {access})?;\n        cursor += {access}.len();\n", offset + 8));
        }
        TypeKind::Vector(element, _) if matches!(element.kind, TypeKind::String(_)) => {
            out.push_str(&format!("        cursor = align8(cursor);\n        let nested_size = {access}.encode_wire(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?)?;\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, {access}.len() as u64)?;\n        cursor += nested_size;\n", offset + 8));
        }
        TypeKind::Vector(element, _)
            if matches!(
                element.kind,
                TypeKind::Handle(_) | TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_)
            ) =>
        {
            out.push_str(&format!("        if handle_offset + handle_count + {access}.len() > handles.len() {{ return Err(FidlWireError::HandleTableTooSmall); }}\n"));
            out.push_str(&format!("        put_u64(bytes, {offset}, (handle_offset + handle_count) as u64)?;\n        put_u64(bytes, {}, {access}.len() as u64)?;\n", offset + 8));
            out.push_str(&format!("        handles[handle_offset + handle_count..handle_offset + handle_count + {access}.len()].copy_from_slice({access});\n"));
            out.push_str(&format!("        handle_count += {access}.len();\n"));
        }
        TypeKind::Handle(_) | TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_) => {
            out.push_str("        if handle_offset + handle_count >= handles.len() { return Err(FidlWireError::HandleTableTooSmall); }\n");
            out.push_str(&format!(
                "        handles[handle_offset + handle_count] = {access};\n"
            ));
            out.push_str(&format!(
                "        put_u32(bytes, {offset}, (handle_offset + handle_count) as u32)?;\n"
            ));
            out.push_str("        handle_count += 1;\n");
        }
        TypeKind::Identifier(name) if ctx.external_type(name).is_some() => {
            out.push_str(&format!(
                "        put_i32(bytes, {offset}, {access} as i32)?;\n"
            ));
        }
        TypeKind::Identifier(name) if ctx.enum_repr(name).is_some() => {
            let primitive = ctx.enum_repr(name).expect("checked above");
            out.push_str(&format!(
                "        put_{}(bytes, {offset}, {access} as {})?;\n",
                primitive_fn(primitive),
                rust_primitive(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.bits_repr(name).is_some() => {
            let primitive = ctx.bits_repr(name).expect("checked above");
            out.push_str(&format!(
                "        put_{}(bytes, {offset}, {access}.0)?;\n",
                primitive_fn(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.is_struct_like(name) => {
            out.push_str(&format!(
                "        cursor = align8(cursor);\n        let nested = {access}.encode_with_handle_offset(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, nested.bytes as u64)?;\n        cursor += nested.bytes;\n        handle_count += nested.handles;\n", offset + 8
            ));
        }
        TypeKind::Identifier(name) if ctx.is_union_like(name) => {
            out.push_str(&format!(
                "        cursor = align8(cursor);\n        let nested = {access}.encode_with_handle_offset(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, nested.bytes as u64)?;\n        cursor += nested.bytes;\n        handle_count += nested.handles;\n", offset + 8
            ));
        }
        TypeKind::Vector(element, _) if matches!(&element.kind, TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name)) =>
        {
            out.push_str(&format!("        cursor = align8(cursor);\n        let nested = {access}.encode_wire(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;\n        put_u64(bytes, {offset}, cursor as u64)?;\n        put_u64(bytes, {}, {access}.len() as u64)?;\n        cursor += nested.bytes;\n        handle_count += nested.handles;\n", offset + 8));
        }
        TypeKind::Array(element, count) => {
            for index in 0..*count {
                emit_encode_array_element(
                    out,
                    element,
                    &format!("{access}[{index}]"),
                    offset + index as usize * slot_size(element, ctx),
                    ctx,
                    "        ",
                );
            }
        }
        _ => {
            out.push_str(&format!(
                "        let _ = {access};\n        return Err(FidlWireError::UnsupportedType);\n"
            ));
        }
    }
}

fn emit_encode_scalar_value(
    out: &mut String,
    ty: &TypeRef,
    access: &str,
    offset: usize,
    ctx: &RustContext,
) {
    if let TypeKind::Identifier(name) = &ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            emit_encode_scalar_value(out, ty, access, offset, ctx);
            return;
        }
    }
    match &ty.kind {
        TypeKind::Primitive(PrimitiveType::Bool) => out.push_str(&format!(
            "            put_u8(bytes, {offset}, if {access} {{ 1 }} else {{ 0 }})?;\n"
        )),
        TypeKind::Primitive(PrimitiveType::Uint8) => {
            out.push_str(&format!(
                "            put_u8(bytes, {offset}, {access})?;\n"
            ));
        }
        TypeKind::Primitive(primitive) => out.push_str(&format!(
            "            put_{}(bytes, {offset}, {access})?;\n",
            primitive_fn(*primitive)
        )),
        TypeKind::Identifier(name) if ctx.enum_repr(name).is_some() => {
            let primitive = ctx.enum_repr(name).expect("checked above");
            out.push_str(&format!(
                "            put_{}(bytes, {offset}, {access} as {})?;\n",
                primitive_fn(primitive),
                rust_primitive(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.bits_repr(name).is_some() => {
            let primitive = ctx.bits_repr(name).expect("checked above");
            out.push_str(&format!(
                "            put_{}(bytes, {offset}, {access}.0)?;\n",
                primitive_fn(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.external_type(name).is_some() => {
            out.push_str(&format!(
                "            put_i32(bytes, {offset}, {access} as i32)?;\n"
            ));
        }
        TypeKind::Array(element, count) => {
            for index in 0..*count {
                emit_encode_array_element(
                    out,
                    element,
                    &format!("{access}[{index}]"),
                    offset + index as usize * slot_size(element, ctx),
                    ctx,
                    "            ",
                );
            }
        }
        _ => out.push_str("            return Err(FidlWireError::UnsupportedType);\n"),
    }
}

fn decode_expr(ty: &TypeRef, offset: usize, ctx: &RustContext) -> String {
    decode_expr_at(ty, "bytes", offset, ctx)
}

fn decode_expr_at(ty: &TypeRef, bytes_ident: &str, offset: usize, ctx: &RustContext) -> String {
    if let TypeKind::Identifier(name) = &ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            return decode_expr_at(ty, bytes_ident, offset, ctx);
        }
    }
    if ty.nullable {
        if let TypeKind::Identifier(name) = &ty.kind {
            if ctx.is_struct_like(name) || ctx.is_union_like(name) {
                return format!(
                    "{{ let start_raw = get_u64({bytes_ident}, {offset})?; let len = get_u64({bytes_ident}, {})? as usize; if start_raw == u64::MAX {{ None }} else {{ let start = start_raw as usize; let end = start.checked_add(len).ok_or(FidlWireError::BufferTooSmall)?; Some({}::decode({bytes_ident}.get(start..end).ok_or(FidlWireError::BufferTooSmall)?, handles)?) }} }}",
                    offset + 8,
                    ctx.type_name(name)
                );
            }
        }
    }
    match &ty.kind {
        TypeKind::Primitive(PrimitiveType::Bool) => {
            format!(
                "match *{bytes_ident}.get({offset}).ok_or(FidlWireError::BufferTooSmall)? {{ 0 => false, _ => true }}"
            )
        }
        TypeKind::Primitive(PrimitiveType::Uint8) => {
            format!("*{bytes_ident}.get({offset}).ok_or(FidlWireError::BufferTooSmall)?")
        }
        TypeKind::Primitive(primitive) => {
            format!("get_{}({bytes_ident}, {offset})?", primitive_fn(*primitive))
        }
        TypeKind::String(_) => format!(
            "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let len = get_u64({bytes_ident}, {})? as usize; if start + len > {bytes_ident}.len() {{ return Err(FidlWireError::BufferTooSmall); }} core::str::from_utf8(&{bytes_ident}[start..start + len]).map_err(|_| FidlWireError::InvalidUtf8)? }}",
            offset + 8
        ),
        TypeKind::Vector(element, _)
            if matches!(element.kind, TypeKind::Primitive(PrimitiveType::Uint8)) =>
        {
            format!(
                "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let len = get_u64({bytes_ident}, {})? as usize; if start + len > {bytes_ident}.len() {{ return Err(FidlWireError::BufferTooSmall); }} &{bytes_ident}[start..start + len] }}",
                offset + 8
            )
        }
        TypeKind::Vector(element, _) if matches!(element.kind, TypeKind::String(_)) => {
            format!(
                "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let count = get_u64({bytes_ident}, {})? as usize; WireStringVector::from_wire({bytes_ident}.get(start..).ok_or(FidlWireError::BufferTooSmall)?, count)? }}",
                offset + 8
            )
        }
        TypeKind::Vector(element, _)
            if matches!(
                element.kind,
                TypeKind::Handle(_) | TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_)
            ) =>
        {
            format!(
                "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let len = get_u64({bytes_ident}, {})? as usize; if start + len > handles.len() {{ return Err(FidlWireError::HandleTableTooSmall); }} &handles[start..start + len] }}",
                offset + 8
            )
        }
        TypeKind::Handle(_) | TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_) => format!(
            "{{ let index = get_u32({bytes_ident}, {offset})? as usize; *handles.get(index).ok_or(FidlWireError::HandleTableTooSmall)? }}"
        ),
        TypeKind::Identifier(name) if ctx.external_type(name).is_some() => {
            format!(
                "{}::decode_value(get_i32({bytes_ident}, {offset})?).map_err(|_| FidlWireError::UnsupportedType)?",
                ctx.type_name(name)
            )
        }
        TypeKind::Identifier(name) if ctx.enum_repr(name).is_some() => {
            let primitive = ctx.enum_repr(name).expect("checked above");
            format!(
                "{}::decode_value(get_{}({bytes_ident}, {offset})?)?",
                ctx.type_name(name),
                primitive_fn(primitive)
            )
        }
        TypeKind::Identifier(name) if ctx.bits_repr(name).is_some() => {
            let primitive = ctx.bits_repr(name).expect("checked above");
            format!(
                "{}(get_{}({bytes_ident}, {offset})?)",
                ctx.type_name(name),
                primitive_fn(primitive)
            )
        }
        TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name) => {
            format!(
                "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let len = get_u64({bytes_ident}, {})? as usize; let end = start.checked_add(len).ok_or(FidlWireError::BufferTooSmall)?; {}::decode({bytes_ident}.get(start..end).ok_or(FidlWireError::BufferTooSmall)?, handles)? }}",
                offset + 8,
                ctx.type_name(name)
            )
        }
        TypeKind::Vector(element, _) if matches!(&element.kind, TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name)) =>
        {
            format!(
                "{{ let start = get_u64({bytes_ident}, {offset})? as usize; let count = get_u64({bytes_ident}, {})? as usize; WireVector::from_wire({bytes_ident}.get(start..).ok_or(FidlWireError::BufferTooSmall)?, handles, count)? }}",
                offset + 8
            )
        }
        TypeKind::Array(element, count) => {
            let values = (0..*count)
                .map(|index| {
                    decode_expr_at(
                        element,
                        bytes_ident,
                        offset + index as usize * slot_size(element, ctx),
                        ctx,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{values}]")
        }
        _ => "return Err(FidlWireError::UnsupportedType)".to_string(),
    }
}

fn emit_encode_union_member(out: &mut String, ty: &TypeRef, ctx: &RustContext) {
    out.push_str("                cursor = align8(cursor);\n");
    match &ty.kind {
        TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name) => {
            out.push_str("                let nested = value.encode_with_handle_offset(bytes.get_mut(cursor..).ok_or(FidlWireError::BufferTooSmall)?, handles, handle_offset + handle_count)?;\n");
            out.push_str("                put_u64(bytes, 8, cursor as u64)?;\n");
            out.push_str("                put_u64(bytes, 16, nested.bytes as u64)?;\n");
            out.push_str("                cursor += nested.bytes;\n");
            out.push_str("                handle_count += nested.handles;\n");
        }
        _ => {
            out.push_str("                return Err(FidlWireError::UnsupportedType);\n");
        }
    }
}

fn decode_union_member_expr(ty: &TypeRef, ctx: &RustContext) -> String {
    if let TypeKind::Identifier(name) = &ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            return decode_union_member_expr(ty, ctx);
        }
        if ctx.is_struct_like(name) || ctx.is_union_like(name) {
            return format!("{}::decode(payload, handles)?", ctx.type_name(name));
        }
    }
    "return Err(FidlWireError::UnsupportedType)".to_string()
}

fn emit_encode_array_element(
    out: &mut String,
    ty: &TypeRef,
    access: &str,
    offset: usize,
    ctx: &RustContext,
    indent: &str,
) {
    if let TypeKind::Identifier(name) = &ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            emit_encode_array_element(out, ty, access, offset, ctx, indent);
            return;
        }
    }
    match &ty.kind {
        TypeKind::Primitive(PrimitiveType::Bool) => out.push_str(&format!(
            "{indent}put_u8(bytes, {offset}, if {access} {{ 1 }} else {{ 0 }})?;\n"
        )),
        TypeKind::Primitive(PrimitiveType::Uint8) => {
            out.push_str(&format!("{indent}put_u8(bytes, {offset}, {access})?;\n"));
        }
        TypeKind::Primitive(primitive) => out.push_str(&format!(
            "{indent}put_{}(bytes, {offset}, {access})?;\n",
            primitive_fn(*primitive)
        )),
        TypeKind::Identifier(name) if ctx.enum_repr(name).is_some() => {
            let primitive = ctx.enum_repr(name).expect("checked above");
            out.push_str(&format!(
                "{indent}put_{}(bytes, {offset}, {access} as {})?;\n",
                primitive_fn(primitive),
                rust_primitive(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.bits_repr(name).is_some() => {
            let primitive = ctx.bits_repr(name).expect("checked above");
            out.push_str(&format!(
                "{indent}put_{}(bytes, {offset}, {access}.0)?;\n",
                primitive_fn(primitive)
            ));
        }
        TypeKind::Identifier(name) if ctx.external_type(name).is_some() => {
            out.push_str(&format!(
                "{indent}put_i32(bytes, {offset}, {access} as i32)?;\n"
            ));
        }
        _ => out.push_str(&format!(
            "{indent}return Err(FidlWireError::UnsupportedType);\n"
        )),
    }
}

fn int_repr(ty: Option<&TypeRef>) -> PrimitiveType {
    match ty.map(|ty| &ty.kind) {
        Some(TypeKind::Primitive(primitive)) => *primitive,
        _ => PrimitiveType::Uint32,
    }
}

fn rust_primitive(primitive: PrimitiveType) -> &'static str {
    match primitive {
        PrimitiveType::Bool => "bool",
        PrimitiveType::Int8 => "i8",
        PrimitiveType::Int16 => "i16",
        PrimitiveType::Int32 => "i32",
        PrimitiveType::Int64 => "i64",
        PrimitiveType::Uint8 => "u8",
        PrimitiveType::Uint16 => "u16",
        PrimitiveType::Uint32 => "u32",
        PrimitiveType::Uint64 => "u64",
        PrimitiveType::Float32 => "f32",
        PrimitiveType::Float64 => "f64",
    }
}

fn find_method<'a>(
    library: &'a ir::Library,
    protocol_name: &str,
    method_name: &str,
) -> &'a ir::Method {
    library
        .protocols
        .iter()
        .find(|protocol| protocol.name == protocol_name)
        .and_then(|protocol| {
            protocol
                .methods
                .iter()
                .find(|method| method.name == method_name)
        })
        .expect("capability splitter produced an unknown method")
}

fn rust_type(ty: &TypeRef, ctx: &RustContext) -> String {
    let base = match &ty.kind {
        TypeKind::Primitive(PrimitiveType::Bool) => "bool".to_string(),
        TypeKind::Primitive(PrimitiveType::Int8) => "i8".to_string(),
        TypeKind::Primitive(PrimitiveType::Int16) => "i16".to_string(),
        TypeKind::Primitive(PrimitiveType::Int32) => "i32".to_string(),
        TypeKind::Primitive(PrimitiveType::Int64) => "i64".to_string(),
        TypeKind::Primitive(PrimitiveType::Uint8) => "u8".to_string(),
        TypeKind::Primitive(PrimitiveType::Uint16) => "u16".to_string(),
        TypeKind::Primitive(PrimitiveType::Uint32) => "u32".to_string(),
        TypeKind::Primitive(PrimitiveType::Uint64) => "u64".to_string(),
        TypeKind::Primitive(PrimitiveType::Float32) => "f32".to_string(),
        TypeKind::Primitive(PrimitiveType::Float64) => "f64".to_string(),
        TypeKind::String(_) => "&'a str".to_string(),
        TypeKind::Vector(element, _)
            if matches!(element.kind, TypeKind::Primitive(PrimitiveType::Uint8)) =>
        {
            "&'a [u8]".to_string()
        }
        TypeKind::Vector(element, _) if matches!(element.kind, TypeKind::String(_)) => {
            "WireStringVector<'a>".to_string()
        }
        TypeKind::Vector(element, _) if matches!(&element.kind, TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name)) =>
        {
            format!("WireVector<'a, {}>", rust_type(element, ctx))
        }
        TypeKind::Vector(element, _) => format!("&'a [{}]", rust_type(element, ctx)),
        TypeKind::Array(element, count) => format!("[{}; {}]", rust_type(element, ctx), count),
        TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_) | TypeKind::Handle(_) => {
            "HandleRef".to_string()
        }
        TypeKind::Identifier(name)
            if ctx.struct_needs_lifetime(name)
                || ctx.union_needs_lifetime(name)
                || ctx.alias_needs_lifetime(name) =>
        {
            format!("{}<'a>", ctx.type_name(name))
        }
        TypeKind::Identifier(name) => ctx.type_name(name),
    };
    if ty.nullable {
        format!("Option<{base}>")
    } else {
        base
    }
}

fn type_needs_lifetime(ty: &TypeRef, ctx: &RustContext) -> bool {
    match &ty.kind {
        TypeKind::String(_) | TypeKind::Vector(_, _) => true,
        TypeKind::Array(element, _) => type_needs_lifetime(element, ctx),
        TypeKind::Identifier(name) => {
            ctx.struct_needs_lifetime(name)
                || ctx.union_needs_lifetime(name)
                || ctx.alias_needs_lifetime(name)
        }
        _ => false,
    }
}

fn type_needs_lifetime_with_sets(
    ty: &TypeRef,
    struct_lifetimes: &BTreeSet<String>,
    lifetime_aliases: &BTreeSet<String>,
) -> bool {
    match &ty.kind {
        TypeKind::String(_) | TypeKind::Vector(_, _) => true,
        TypeKind::Array(element, _) => {
            type_needs_lifetime_with_sets(element, struct_lifetimes, lifetime_aliases)
        }
        TypeKind::Identifier(name) => {
            let short = name.rsplit('.').next().unwrap_or(name);
            struct_lifetimes.contains(short) || lifetime_aliases.contains(short)
        }
        _ => false,
    }
}

fn type_needs_direct_lifetime(ty: &TypeRef) -> bool {
    match &ty.kind {
        TypeKind::String(_) | TypeKind::Vector(_, _) => true,
        TypeKind::Array(element, _) => type_needs_direct_lifetime(element),
        _ => false,
    }
}

fn payload_needs_lifetime(payload: &ir::Payload, ctx: &RustContext) -> bool {
    payload
        .fields
        .iter()
        .any(|field| type_needs_lifetime(&field.ty, ctx))
}

fn slot_size(ty: &TypeRef, ctx: &RustContext) -> usize {
    if let TypeKind::Identifier(name) = &ty.kind {
        if let Some(ty) = ctx.alias_type(name) {
            return slot_size(ty, ctx);
        }
        if let Some(primitive) = ctx.enum_repr(name).or_else(|| ctx.bits_repr(name)) {
            return primitive_slot_size(primitive);
        }
        if ctx.external_type(name).is_some() {
            return primitive_slot_size(PrimitiveType::Int32);
        }
    }
    match &ty.kind {
        TypeKind::Primitive(primitive) => primitive_slot_size(*primitive),
        TypeKind::Handle(_) | TypeKind::ClientEnd(_) | TypeKind::ServerEnd(_) => 4,
        TypeKind::String(_) | TypeKind::Vector(_, _) => 16,
        TypeKind::Array(element, count) => slot_size(element, ctx) * *count as usize,
        TypeKind::Identifier(name) if ctx.is_struct_like(name) || ctx.is_union_like(name) => 16,
        TypeKind::Identifier(_) => 16,
    }
}

fn primitive_slot_size(primitive: PrimitiveType) -> usize {
    match primitive {
        PrimitiveType::Bool | PrimitiveType::Uint8 | PrimitiveType::Int8 => 1,
        PrimitiveType::Uint16 | PrimitiveType::Int16 => 2,
        PrimitiveType::Uint32 | PrimitiveType::Int32 | PrimitiveType::Float32 => 4,
        PrimitiveType::Uint64 | PrimitiveType::Int64 | PrimitiveType::Float64 => 8,
    }
}

fn align8_const(value: usize) -> usize {
    (value + 7) & !7
}

fn primitive_fn(primitive: PrimitiveType) -> &'static str {
    match primitive {
        PrimitiveType::Bool => "u8",
        PrimitiveType::Int8 => "i8",
        PrimitiveType::Int16 => "i16",
        PrimitiveType::Int32 => "i32",
        PrimitiveType::Int64 => "i64",
        PrimitiveType::Uint8 => "u8",
        PrimitiveType::Uint16 => "u16",
        PrimitiveType::Uint32 => "u32",
        PrimitiveType::Uint64 => "u64",
        PrimitiveType::Float32 => "f32",
        PrimitiveType::Float64 => "f64",
    }
}

fn lifetime_decl(needs_lifetime: bool) -> &'static str {
    if needs_lifetime { "<'a>" } else { "" }
}

fn lifetime_impl(needs_lifetime: bool) -> &'static str {
    if needs_lifetime { "<'a>" } else { "" }
}

fn lifetime_use(needs_lifetime: bool) -> &'static str {
    if needs_lifetime { "<'a>" } else { "" }
}

fn lifetime_infer(needs_lifetime: bool) -> &'static str {
    if needs_lifetime { "<'_>" } else { "" }
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

fn rust_crate_for_library(library: &str) -> String {
    let leaf = library.rsplit('.').next().unwrap_or(library);
    format!("{}_fidl", to_snake_case(leaf))
}

fn to_pascal_case(value: &str) -> String {
    if value
        .chars()
        .any(|ch| !ch.is_ascii_alphanumeric() || ch == '_')
        || value
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .all(|ch| ch.is_ascii_uppercase())
    {
        let mut out = String::new();
        for part in value.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if part.is_empty() {
                continue;
            }
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                for ch in chars {
                    out.push(ch.to_ascii_lowercase());
                }
            }
        }
        if out.is_empty() {
            return "Generated".to_string();
        }
        if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            return format!("Type{out}");
        }
        return out;
    }

    let mut out = String::new();
    let mut capitalize = true;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                out.push(ch.to_ascii_uppercase());
                capitalize = false;
            } else {
                out.push(ch);
            }
        } else {
            capitalize = true;
        }
    }
    if out.is_empty() {
        "Generated".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("Type{out}")
    } else {
        out
    }
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    if out.is_empty() || matches!(out.as_str(), "type" | "match" | "self" | "super" | "crate") {
        format!("{out}_")
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("field_{out}")
    } else {
        out
    }
}

fn to_screaming_snake_case(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return value.to_string();
    }
    let snake = to_snake_case(value);
    snake.to_ascii_uppercase()
}
