#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File {
    pub library: String,
    pub usings: Vec<Using>,
    pub declarations: Vec<Decl>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Using {
    pub library: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decl {
    Alias(Alias),
    Bits(Bits),
    Enum(Enum),
    Protocol(Protocol),
    Struct(Struct),
    Table(Table),
    Union(Union),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: Option<AttributeValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttributeValue {
    String(String),
    Int(i64),
    Ident(String),
    Raw(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Alias {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Struct {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub resource: bool,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Table {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub resource: bool,
    pub fields: Vec<TableField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Union {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub strict: bool,
    pub members: Vec<UnionMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Enum {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub members: Vec<ConstMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bits {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: Option<TypeRef>,
    pub members: Vec<ConstMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Protocol {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub composes: Vec<String>,
    pub methods: Vec<Method>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Method {
    pub attrs: Vec<Attribute>,
    pub ordinal: Option<u64>,
    pub name: String,
    pub request: Option<Payload>,
    pub response: Option<Payload>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Payload {
    Struct(StructPayload),
    Type(TypeRef),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructPayload {
    pub resource: bool,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableField {
    pub ordinal: u64,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionMember {
    pub ordinal: u64,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstMember {
    pub name: String,
    pub value: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeRef {
    pub kind: TypeKind,
    pub nullable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeKind {
    Primitive(PrimitiveType),
    Identifier(String),
    String(Option<u64>),
    Vector(Box<TypeRef>, Option<u64>),
    Array(Box<TypeRef>, u64),
    ClientEnd(String),
    ServerEnd(String),
    Handle(Option<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimitiveType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float32,
    Float64,
}

impl Decl {
    pub fn name(&self) -> &str {
        match self {
            Decl::Alias(decl) => &decl.name,
            Decl::Bits(decl) => &decl.name,
            Decl::Enum(decl) => &decl.name,
            Decl::Protocol(decl) => &decl.name,
            Decl::Struct(decl) => &decl.name,
            Decl::Table(decl) => &decl.name,
            Decl::Union(decl) => &decl.name,
        }
    }
}

impl Method {
    pub fn permission(&self) -> Option<&str> {
        self.attrs.iter().find_map(|attr| {
            if attr.name != "permission" {
                return None;
            }
            match &attr.value {
                Some(AttributeValue::String(value)) => Some(value.as_str()),
                _ => None,
            }
        })
    }
}
