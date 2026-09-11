use crate::ast::{
    Alias, Attribute, AttributeValue, Bits, ConstMember, Decl, Enum, Field, File, Method, Payload,
    PrimitiveType, Protocol, Struct, StructPayload, Table, TableField, TypeKind, TypeRef, Union,
    UnionMember, Using,
};
use crate::lexer::{Token, TokenKind, lex};

pub fn parse_file(source: &str) -> Result<File, String> {
    Parser::new(source)?.parse_file()
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    synthetic_structs: Vec<Struct>,
}

impl Parser {
    fn new(source: &str) -> Result<Self, String> {
        Ok(Self {
            tokens: lex(source)?,
            cursor: 0,
            synthetic_structs: Vec::new(),
        })
    }

    fn parse_file(&mut self) -> Result<File, String> {
        self.expect_ident_value("library")?;
        let library = self.parse_library_name()?;
        self.expect_symbol(';')?;

        let mut usings = Vec::new();
        while self.peek_ident_value("using") {
            self.expect_ident_value("using")?;
            usings.push(Using {
                library: self.parse_library_name()?,
            });
            self.expect_symbol(';')?;
        }

        let mut declarations = Vec::new();
        while !self.is_eof() {
            let attrs = self.parse_attributes()?;
            declarations.push(self.parse_decl(attrs)?);
        }

        declarations.extend(self.synthetic_structs.drain(..).map(Decl::Struct));

        if declarations.is_empty() {
            return Err("expected at least one declaration".to_string());
        }

        Ok(File {
            library,
            usings,
            declarations,
        })
    }

    fn parse_decl(&mut self, attrs: Vec<Attribute>) -> Result<Decl, String> {
        let resource = self.eat_ident_value("resource");
        let keyword = self.expect_ident()?;
        match keyword.as_str() {
            "alias" if !resource => self.parse_alias(attrs).map(Decl::Alias),
            "bits" if !resource => self.parse_bits(attrs).map(Decl::Bits),
            "enum" if !resource => self.parse_enum(attrs).map(Decl::Enum),
            "protocol" if !resource => self.parse_protocol(attrs).map(Decl::Protocol),
            "struct" => self.parse_struct(attrs, resource).map(Decl::Struct),
            "table" => self.parse_table(attrs, resource).map(Decl::Table),
            "type" if !resource => self.parse_type_decl(attrs),
            _ if resource => Err(format!("`resource` is not valid before `{keyword}`")),
            _ => Err(format!("expected declaration keyword, got `{keyword}`")),
        }
    }

    fn parse_type_decl(&mut self, attrs: Vec<Attribute>) -> Result<Decl, String> {
        let name = self.expect_ident()?;
        self.expect_symbol('=')?;
        let strict = self.eat_ident_value("strict");
        let flexible = if !strict {
            self.eat_ident_value("flexible")
        } else {
            false
        };
        let keyword = self.expect_ident()?;
        if flexible {
            if keyword == "union" {
                return Err("flexible unions are not supported in FIDL v1".to_string());
            }
            return Err(format!(
                "flexible `{keyword}` declarations are not supported"
            ));
        }
        match keyword.as_str() {
            "bits" => {
                let ty = if self.eat_symbol(':') {
                    Some(self.parse_type_ref()?)
                } else {
                    None
                };
                let members = self.parse_const_members()?;
                self.expect_symbol(';')?;
                Ok(Decl::Bits(Bits {
                    attrs,
                    name,
                    ty,
                    members,
                }))
            }
            "enum" => {
                let ty = if self.eat_symbol(':') {
                    Some(self.parse_type_ref()?)
                } else {
                    None
                };
                let members = self.parse_const_members()?;
                self.expect_symbol(';')?;
                Ok(Decl::Enum(Enum {
                    attrs,
                    name,
                    ty,
                    members,
                }))
            }
            "union" => self.parse_union(attrs, name, strict).map(Decl::Union),
            other => Err(format!(
                "type aliases may only name `strict bits`, `strict enum`, or `strict union`, got `{other}`"
            )),
        }
    }

    fn parse_alias(&mut self, attrs: Vec<Attribute>) -> Result<Alias, String> {
        let name = self.expect_ident()?;
        self.expect_symbol('=')?;
        let ty = self.parse_type_ref()?;
        self.expect_symbol(';')?;
        Ok(Alias { attrs, name, ty })
    }

    fn parse_struct(&mut self, attrs: Vec<Attribute>, resource: bool) -> Result<Struct, String> {
        let name = self.expect_ident()?;
        let fields = self.parse_field_block()?;
        self.expect_symbol(';')?;
        Ok(Struct {
            attrs,
            name,
            resource,
            fields,
        })
    }

    fn parse_table(&mut self, attrs: Vec<Attribute>, resource: bool) -> Result<Table, String> {
        let name = self.expect_ident()?;
        self.expect_symbol('{')?;
        let mut fields = Vec::new();
        while !self.eat_symbol('}') {
            let ordinal = self.expect_u64()?;
            self.expect_symbol(':')?;
            let name = self.expect_ident()?;
            let ty = self.parse_type_ref()?;
            self.expect_field_separator()?;
            fields.push(TableField { ordinal, name, ty });
        }
        self.expect_symbol(';')?;
        Ok(Table {
            attrs,
            name,
            resource,
            fields,
        })
    }

    fn parse_union(
        &mut self,
        attrs: Vec<Attribute>,
        name: String,
        strict: bool,
    ) -> Result<Union, String> {
        if !strict {
            return Err("union declarations must be `strict union` in FIDL v1".to_string());
        }
        self.expect_symbol('{')?;
        let mut members = Vec::new();
        while !self.eat_symbol('}') {
            let ordinal = self.expect_u64()?;
            self.expect_symbol(':')?;
            let name = self.expect_ident()?;
            let ty = self.parse_type_ref()?;
            self.expect_field_separator()?;
            members.push(UnionMember { ordinal, name, ty });
        }
        self.expect_symbol(';')?;
        Ok(Union {
            attrs,
            name,
            strict,
            members,
        })
    }

    fn parse_enum(&mut self, attrs: Vec<Attribute>) -> Result<Enum, String> {
        let name = self.expect_ident()?;
        let ty = if self.eat_symbol(':') {
            Some(self.parse_type_ref()?)
        } else {
            None
        };
        let members = self.parse_const_members()?;
        self.expect_symbol(';')?;
        Ok(Enum {
            attrs,
            name,
            ty,
            members,
        })
    }

    fn parse_bits(&mut self, attrs: Vec<Attribute>) -> Result<Bits, String> {
        let name = self.expect_ident()?;
        let ty = if self.eat_symbol(':') {
            Some(self.parse_type_ref()?)
        } else {
            None
        };
        let members = self.parse_const_members()?;
        self.expect_symbol(';')?;
        Ok(Bits {
            attrs,
            name,
            ty,
            members,
        })
    }

    fn parse_protocol(&mut self, attrs: Vec<Attribute>) -> Result<Protocol, String> {
        let name = self.expect_ident()?;
        self.expect_symbol('{')?;

        let mut methods = Vec::new();
        let mut composes = Vec::new();
        while !self.eat_symbol('}') {
            if self.is_eof() {
                return Err(format!("protocol {name} is missing closing `}}`"));
            }
            if self.eat_ident_value("compose") {
                composes.push(self.parse_qualified_name()?);
                self.expect_symbol(';')?;
            } else {
                methods.push(self.parse_method()?);
            }
        }
        self.expect_symbol(';')?;

        Ok(Protocol {
            attrs,
            name,
            composes,
            methods,
        })
    }

    fn parse_method(&mut self) -> Result<Method, String> {
        let attrs = self.parse_attributes()?;
        let ordinal = if matches!(self.peek_kind(), Some(TokenKind::IntLiteral(_)))
            && self.peek_symbol_at(1, ':')
        {
            let ordinal = self.expect_u64()?;
            self.expect_symbol(':')?;
            Some(ordinal)
        } else {
            ordinal_attribute(&attrs)?
        };

        let name = self.expect_ident()?;
        let request = self.parse_method_payload()?;
        let response = if self.eat_symbol('-') {
            self.expect_symbol('>')?;
            self.parse_method_payload()?
        } else {
            None
        };
        self.expect_symbol(';')?;

        Ok(Method {
            attrs,
            ordinal,
            name,
            request,
            response,
        })
    }

    fn parse_method_payload(&mut self) -> Result<Option<Payload>, String> {
        if !self.eat_symbol('(') {
            return Ok(None);
        }
        if self.eat_symbol(')') {
            return Ok(Some(Payload::Struct(StructPayload {
                resource: false,
                fields: Vec::new(),
            })));
        }

        let resource = self.eat_ident_value("resource");
        let payload = if self.eat_ident_value("struct") {
            Payload::Struct(StructPayload {
                resource,
                fields: self.parse_inline_fields()?,
            })
        } else {
            if resource {
                return Err("resource method payloads must use `resource struct`".to_string());
            }
            Payload::Type(self.parse_type_ref()?)
        };
        self.expect_symbol(')')?;
        Ok(Some(payload))
    }

    fn parse_field_block(&mut self) -> Result<Vec<Field>, String> {
        self.expect_symbol('{')?;
        let fields = self.parse_fields_until_close()?;
        self.expect_symbol('}')?;
        Ok(fields)
    }

    fn parse_inline_fields(&mut self) -> Result<Vec<Field>, String> {
        self.expect_symbol('{')?;
        let fields = self.parse_fields_until_close()?;
        self.expect_symbol('}')?;
        Ok(fields)
    }

    fn parse_fields_until_close(&mut self) -> Result<Vec<Field>, String> {
        let mut fields = Vec::new();
        while !self.peek_symbol('}') {
            let attrs = self.parse_attributes()?;
            if !attrs.is_empty() {
                return Err("field attributes are not supported in FIDL v1".to_string());
            }
            let name = self.expect_ident()?;
            let ty = self.parse_type_ref()?;
            self.expect_field_separator()?;
            fields.push(Field { name, ty });
        }
        Ok(fields)
    }

    fn parse_const_members(&mut self) -> Result<Vec<ConstMember>, String> {
        self.expect_symbol('{')?;
        let mut members = Vec::new();
        while !self.eat_symbol('}') {
            let name = self.expect_ident()?;
            let value = if self.eat_symbol('=') {
                Some(self.expect_i64()?)
            } else {
                None
            };
            self.expect_field_separator()?;
            members.push(ConstMember { name, value });
        }
        Ok(members)
    }

    fn parse_type_ref(&mut self) -> Result<TypeRef, String> {
        let name = self.expect_ident()?;
        let kind = match name.as_str() {
            "bool" => TypeKind::Primitive(PrimitiveType::Bool),
            "int8" => TypeKind::Primitive(PrimitiveType::Int8),
            "int16" => TypeKind::Primitive(PrimitiveType::Int16),
            "int32" => TypeKind::Primitive(PrimitiveType::Int32),
            "int64" => TypeKind::Primitive(PrimitiveType::Int64),
            "uint8" => TypeKind::Primitive(PrimitiveType::Uint8),
            "uint16" => TypeKind::Primitive(PrimitiveType::Uint16),
            "uint32" => TypeKind::Primitive(PrimitiveType::Uint32),
            "uint64" => TypeKind::Primitive(PrimitiveType::Uint64),
            "float32" => TypeKind::Primitive(PrimitiveType::Float32),
            "float64" => TypeKind::Primitive(PrimitiveType::Float64),
            "string" => TypeKind::String(self.parse_bound()?),
            "vector" => {
                self.expect_symbol('<')?;
                let element = self.parse_type_ref()?;
                self.expect_symbol('>')?;
                TypeKind::Vector(Box::new(element), self.parse_bound()?)
            }
            "struct" => {
                let fields = self.parse_inline_fields()?;
                TypeKind::Identifier(self.push_synthetic_struct(fields))
            }
            "array" => {
                self.expect_symbol('<')?;
                let element = self.parse_type_ref()?;
                self.expect_symbol(',')?;
                let count = self.expect_u64()?;
                self.expect_symbol('>')?;
                TypeKind::Array(Box::new(element), count)
            }
            "client_end" => {
                self.expect_symbol(':')?;
                TypeKind::ClientEnd(self.parse_qualified_name()?)
            }
            "server_end" => {
                self.expect_symbol(':')?;
                TypeKind::ServerEnd(self.parse_qualified_name()?)
            }
            "handle" => TypeKind::Handle(if self.eat_symbol(':') {
                Some(self.expect_ident()?)
            } else {
                None
            }),
            _ => TypeKind::Identifier(self.parse_qualified_name_tail(name)?),
        };
        let nullable = self.eat_symbol('?');
        Ok(TypeRef { kind, nullable })
    }

    fn parse_bound(&mut self) -> Result<Option<u64>, String> {
        if self.eat_symbol(':') {
            Ok(Some(self.expect_u64()?))
        } else {
            Ok(None)
        }
    }

    fn push_synthetic_struct(&mut self, fields: Vec<Field>) -> String {
        let name = format!("InlineVectorStruct{}", self.synthetic_structs.len() + 1);
        self.synthetic_structs.push(Struct {
            attrs: Vec::new(),
            name: name.clone(),
            resource: false,
            fields,
        });
        name
    }

    fn parse_attributes(&mut self) -> Result<Vec<Attribute>, String> {
        let mut attrs = Vec::new();
        while self.eat_symbol('@') {
            let name = self.expect_ident()?;
            let value = if self.eat_symbol('(') {
                let value = self.parse_attribute_value()?;
                self.expect_symbol(')')?;
                value
            } else {
                None
            };
            attrs.push(Attribute { name, value });
        }
        Ok(attrs)
    }

    fn parse_attribute_value(&mut self) -> Result<Option<AttributeValue>, String> {
        if self.peek_symbol(')') {
            return Ok(None);
        }
        let mut strings = Vec::new();
        while let Some(TokenKind::StringLiteral(value)) = self.peek_kind() {
            strings.push(value.clone());
            self.cursor += 1;
        }
        if !strings.is_empty() {
            return Ok(Some(AttributeValue::String(strings.concat())));
        }
        match self.next_kind() {
            Some(TokenKind::IntLiteral(value)) => Ok(Some(AttributeValue::Int(value))),
            Some(TokenKind::Ident(value)) if self.peek_symbol(')') => {
                Ok(Some(AttributeValue::Ident(value)))
            }
            Some(first) => {
                let mut raw = token_text(&first);
                while !self.peek_symbol(')') {
                    let kind = self
                        .next_kind()
                        .ok_or_else(|| "attribute is missing closing `)`".to_string())?;
                    raw.push_str(&token_text(&kind));
                }
                Ok(Some(AttributeValue::Raw(raw)))
            }
            None => Err("attribute is missing closing `)`".to_string()),
        }
    }

    fn parse_library_name(&mut self) -> Result<String, String> {
        let first = self.expect_ident()?;
        self.parse_qualified_name_tail(first)
    }

    fn parse_qualified_name(&mut self) -> Result<String, String> {
        let first = self.expect_ident()?;
        self.parse_qualified_name_tail(first)
    }

    fn parse_qualified_name_tail(&mut self, first: String) -> Result<String, String> {
        let mut parts = vec![first];
        while self.eat_symbol('.') {
            parts.push(self.expect_ident()?);
        }
        Ok(parts.join("."))
    }

    fn expect_ident_value(&mut self, expected: &str) -> Result<(), String> {
        let actual = self.expect_ident()?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!("expected `{expected}`, got `{actual}`"))
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.next_kind() {
            Some(TokenKind::Ident(value)) => Ok(value),
            Some(other) => Err(format!("expected identifier, got {other:?}")),
            None => Err("expected identifier, got end of file".to_string()),
        }
    }

    fn expect_i64(&mut self) -> Result<i64, String> {
        match self.next_kind() {
            Some(TokenKind::IntLiteral(value)) => Ok(value),
            Some(other) => Err(format!("expected integer literal, got {other:?}")),
            None => Err("expected integer literal, got end of file".to_string()),
        }
    }

    fn expect_u64(&mut self) -> Result<u64, String> {
        let value = self.expect_i64()?;
        u64::try_from(value).map_err(|_| format!("expected non-negative integer, got {value}"))
    }

    fn expect_symbol(&mut self, expected: char) -> Result<(), String> {
        if self.eat_symbol(expected) {
            Ok(())
        } else {
            Err(format!("expected `{expected}`"))
        }
    }

    fn eat_ident_value(&mut self, expected: &str) -> bool {
        match self.peek_kind() {
            Some(TokenKind::Ident(actual)) if actual == expected => {
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }

    fn eat_symbol(&mut self, expected: char) -> bool {
        match self.peek_kind() {
            Some(TokenKind::Symbol(actual)) if *actual == expected => {
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_field_separator(&mut self) -> Result<(), String> {
        if self.eat_symbol(';') || self.eat_symbol(',') || self.peek_symbol('}') {
            Ok(())
        } else {
            Err("expected field separator `;` or `,`".to_string())
        }
    }

    fn peek_ident_value(&self, expected: &str) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Ident(actual)) if actual == expected)
    }

    fn peek_symbol(&self, expected: char) -> bool {
        matches!(self.peek_kind(), Some(TokenKind::Symbol(actual)) if *actual == expected)
    }

    fn peek_symbol_at(&self, offset: usize, expected: char) -> bool {
        matches!(
            self.tokens.get(self.cursor + offset).map(|token| &token.kind),
            Some(TokenKind::Symbol(actual)) if *actual == expected
        )
    }

    fn is_eof(&self) -> bool {
        self.cursor >= self.tokens.len()
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.cursor).map(|token| &token.kind)
    }

    fn next_kind(&mut self) -> Option<TokenKind> {
        let token = self.tokens.get(self.cursor).cloned();
        if token.is_some() {
            self.cursor += 1;
        }
        token.map(|token| token.kind)
    }
}

fn ordinal_attribute(attrs: &[Attribute]) -> Result<Option<u64>, String> {
    let mut ordinal = None;
    for attr in attrs {
        if attr.name != "ordinal" {
            continue;
        }
        if ordinal.is_some() {
            return Err("method has duplicate @ordinal attributes".to_string());
        }
        ordinal = match &attr.value {
            Some(AttributeValue::Int(value)) => Some(
                u64::try_from(*value)
                    .map_err(|_| "@ordinal requires a non-negative integer literal".to_string())?,
            ),
            _ => return Err("@ordinal requires an integer literal".to_string()),
        };
    }
    Ok(ordinal)
}

fn token_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(value) => value.clone(),
        TokenKind::IntLiteral(value) => value.to_string(),
        TokenKind::StringLiteral(value) => format!("{value:?}"),
        TokenKind::Symbol(value) => value.to_string(),
    }
}
