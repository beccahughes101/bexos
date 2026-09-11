use crate::ast::{self, TypeRef};

pub const WIRE_ABI_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Library {
    pub name: String,
    pub usings: Vec<String>,
    pub declarations: Vec<Decl>,
    pub protocols: Vec<Protocol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decl {
    Alias(ast::Alias),
    Bits(ast::Bits),
    Enum(ast::Enum),
    Struct(ast::Struct),
    Table(ast::Table),
    Union(ast::Union),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Protocol {
    pub name: String,
    pub methods: Vec<Method>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Method {
    pub ordinal: u64,
    pub name: String,
    pub permission: Option<String>,
    pub request: Payload,
    pub response: Payload,
    pub one_way: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payload {
    pub name: String,
    pub resource: bool,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
}

impl Library {
    pub fn from_ast(file: ast::File) -> Self {
        let mut declarations = Vec::new();
        let mut protocol_decls = Vec::new();

        for decl in file.declarations {
            match decl {
                ast::Decl::Alias(decl) => declarations.push(Decl::Alias(decl)),
                ast::Decl::Bits(decl) => declarations.push(Decl::Bits(decl)),
                ast::Decl::Enum(decl) => declarations.push(Decl::Enum(decl)),
                ast::Decl::Struct(decl) => declarations.push(Decl::Struct(decl)),
                ast::Decl::Table(decl) => declarations.push(Decl::Table(decl)),
                ast::Decl::Union(decl) => declarations.push(Decl::Union(decl)),
                ast::Decl::Protocol(protocol) => {
                    protocol_decls.push(protocol);
                }
            }
        }

        let mut protocols = Vec::new();
        for protocol in &protocol_decls {
            let mut methods = Vec::new();
            collect_protocol_methods(protocol, &protocol_decls, &mut methods);
            protocols.push(Protocol {
                methods,
                name: protocol.name.clone(),
            });
        }

        Self {
            name: file.library,
            usings: file.usings.into_iter().map(|using| using.library).collect(),
            declarations,
            protocols,
        }
    }
}

fn collect_protocol_methods(
    protocol: &ast::Protocol,
    protocols: &[ast::Protocol],
    out: &mut Vec<Method>,
) {
    for parent_name in &protocol.composes {
        let short = parent_name.rsplit('.').next().unwrap_or(parent_name);
        if let Some(parent) = protocols.iter().find(|candidate| candidate.name == short) {
            collect_protocol_methods(parent, protocols, out);
        }
    }
    out.extend(
        protocol
            .methods
            .iter()
            .cloned()
            .map(|method| lower_method(&protocol.name, method)),
    );
}

fn lower_method(protocol: &str, method: ast::Method) -> Method {
    let one_way = method.response.is_none();
    Method {
        ordinal: method
            .ordinal
            .unwrap_or_else(|| stable_ordinal(protocol, &method.name)),
        name: method.name.clone(),
        permission: method.permission().map(str::to_string),
        request: lower_payload(
            format!("{}{}Request", protocol, method.name),
            method.request,
        ),
        response: lower_payload(
            format!("{}{}Response", protocol, method.name),
            method.response,
        ),
        one_way,
    }
}

fn lower_payload(name: String, payload: Option<ast::Payload>) -> Payload {
    match payload {
        Some(ast::Payload::Struct(payload)) => Payload {
            name,
            resource: payload.resource,
            fields: payload
                .fields
                .into_iter()
                .map(|field| Field {
                    name: field.name,
                    ty: field.ty,
                })
                .collect(),
        },
        Some(ast::Payload::Type(ty)) => Payload {
            name,
            resource: false,
            fields: vec![Field {
                name: "value".to_string(),
                ty,
            }],
        },
        None => Payload {
            name,
            resource: false,
            fields: Vec::new(),
        },
    }
}

pub fn stable_ordinal(protocol: &str, method: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in protocol.bytes().chain([b'.']).chain(method.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash & 0x7fff_ffff_ffff_ffff
}
