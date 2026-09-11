use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{
    AttributeValue, Bits, Decl, Enum, File, PrimitiveType, Protocol, TypeKind, TypeRef, Union,
};

pub fn validate(file: &File) -> Result<(), String> {
    validate_with_deps(file, &[])
}

pub fn validate_with_deps(file: &File, deps: &[File]) -> Result<(), String> {
    validate_library_name(&file.library)?;
    let mut type_names = builtin_type_names();
    let dependency_types = dependency_type_names(file, deps)?;
    let mut decl_names = BTreeSet::new();
    let mut protocol_names = BTreeSet::new();

    for decl in &file.declarations {
        if !decl_names.insert(decl.name().to_string()) {
            return Err(format!("duplicate declaration `{}`", decl.name()));
        }
        match decl {
            Decl::Protocol(protocol) => {
                if !protocol_names.insert(protocol.name.clone()) {
                    return Err(format!("duplicate protocol `{}`", protocol.name));
                }
            }
            _ => {
                type_names.insert(decl.name().to_string());
            }
        }
    }

    for decl in &file.declarations {
        match decl {
            Decl::Alias(alias) => validate_type(&alias.ty, &type_names, &dependency_types)?,
            Decl::Bits(bits) => validate_bits(bits)?,
            Decl::Enum(en) => validate_enum(en)?,
            Decl::Protocol(protocol) => {
                validate_protocol(protocol, &type_names, &dependency_types, &protocol_names)?
            }
            Decl::Struct(st) => validate_fields(
                st.fields.iter().map(|field| (&field.name, &field.ty)),
                &type_names,
                &dependency_types,
            )?,
            Decl::Table(table) => {
                let mut ordinals = BTreeSet::new();
                validate_fields(
                    table.fields.iter().map(|field| (&field.name, &field.ty)),
                    &type_names,
                    &dependency_types,
                )?;
                for field in &table.fields {
                    if field.ordinal == 0 {
                        return Err(format!("table `{}` has zero field ordinal", table.name));
                    }
                    if !ordinals.insert(field.ordinal) {
                        return Err(format!(
                            "table `{}` has duplicate field ordinal {}",
                            table.name, field.ordinal
                        ));
                    }
                }
            }
            Decl::Union(union) => validate_union(union, &type_names, &dependency_types)?,
        }
    }

    validate_protocol_composition(file)?;

    validate_rust_name_collisions(file)?;

    Ok(())
}

fn dependency_type_names(file: &File, deps: &[File]) -> Result<BTreeSet<String>, String> {
    let imported = file
        .usings
        .iter()
        .map(|using| using.library.as_str())
        .collect::<BTreeSet<_>>();
    let mut out = BTreeSet::new();
    for dep in deps {
        if dep.library == file.library {
            return Err(format!(
                "dependency library `{}` cannot be the root library",
                dep.library
            ));
        }
        if !imported.contains(dep.library.as_str()) {
            return Err(format!(
                "dependency library `{}` is not imported with using",
                dep.library
            ));
        }
        for decl in &dep.declarations {
            out.insert(format!("{}.{}", dep.library, decl.name()));
        }
    }
    Ok(out)
}

fn validate_protocol_composition(file: &File) -> Result<(), String> {
    let protocols = file
        .declarations
        .iter()
        .filter_map(|decl| match decl {
            Decl::Protocol(protocol) => Some(protocol),
            _ => None,
        })
        .collect::<Vec<_>>();
    for protocol in &protocols {
        let mut visiting = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut ordinals = BTreeMap::new();
        collect_composed_methods(
            protocol,
            &protocols,
            &mut visiting,
            &mut names,
            &mut ordinals,
        )?;
    }
    Ok(())
}

fn collect_composed_methods<'a>(
    protocol: &'a Protocol,
    protocols: &[&'a Protocol],
    visiting: &mut BTreeSet<String>,
    names: &mut BTreeSet<String>,
    ordinals: &mut BTreeMap<u64, String>,
) -> Result<(), String> {
    if !visiting.insert(protocol.name.clone()) {
        return Err(format!(
            "protocol composition cycle includes `{}`",
            protocol.name
        ));
    }
    for parent_name in &protocol.composes {
        let short = parent_name.rsplit('.').next().unwrap_or(parent_name);
        let parent = protocols
            .iter()
            .copied()
            .find(|candidate| candidate.name == short)
            .ok_or_else(|| {
                format!(
                    "protocol `{}` composes unknown protocol `{parent_name}`",
                    protocol.name
                )
            })?;
        collect_composed_methods(parent, protocols, visiting, names, ordinals)?;
    }
    for method in &protocol.methods {
        if !names.insert(method.name.clone()) {
            return Err(format!(
                "protocol `{}` inherits duplicate method `{}`",
                protocol.name, method.name
            ));
        }
        if let Some(ordinal) = method.ordinal {
            if let Some(existing) = ordinals.insert(ordinal, method.name.clone()) {
                return Err(format!(
                    "protocol `{}` inherits duplicate ordinal for methods `{existing}` and `{}`",
                    protocol.name, method.name
                ));
            }
        }
    }
    visiting.remove(&protocol.name);
    Ok(())
}

fn validate_library_name(name: &str) -> Result<(), String> {
    if name.split('.').all(valid_ident) {
        Ok(())
    } else {
        Err(format!("invalid library name `{name}`"))
    }
}

fn validate_protocol(
    protocol: &Protocol,
    type_names: &BTreeSet<String>,
    dependency_types: &BTreeSet<String>,
    protocol_names: &BTreeSet<String>,
) -> Result<(), String> {
    let mut method_names = BTreeSet::new();
    let mut ordinals = BTreeMap::new();

    for parent in &protocol.composes {
        let short = parent.rsplit('.').next().unwrap_or(parent);
        if short == protocol.name {
            return Err(format!(
                "protocol `{}` cannot compose itself",
                protocol.name
            ));
        }
        if !protocol_names.contains(short) {
            return Err(format!(
                "protocol `{}` composes unknown protocol `{parent}`",
                protocol.name
            ));
        }
    }

    for method in &protocol.methods {
        if !method_names.insert(method.name.clone()) {
            return Err(format!(
                "protocol `{}` has duplicate method `{}`",
                protocol.name, method.name
            ));
        }
        if let Some(existing) = method
            .ordinal
            .and_then(|ordinal| ordinals.insert(ordinal, method.name.clone()))
        {
            return Err(format!(
                "protocol `{}` has duplicate ordinal for methods `{}` and `{}`",
                protocol.name, existing, method.name
            ));
        }
        if method.ordinal == Some(0) {
            return Err(format!(
                "protocol `{}` method `{}` has zero ordinal",
                protocol.name, method.name
            ));
        }
        for attr in &method.attrs {
            if attr.name == "permission" {
                match &attr.value {
                    Some(AttributeValue::String(value)) if !value.is_empty() => {}
                    _ => return Err("@permission requires at least one string literal".to_string()),
                }
            }
        }
        if let Some(request) = &method.request {
            validate_payload(request, type_names, dependency_types)?;
        }
        if let Some(response) = &method.response {
            validate_payload(response, type_names, dependency_types)?;
        }
    }

    Ok(())
}

fn validate_rust_name_collisions(file: &File) -> Result<(), String> {
    let mut type_names = BTreeMap::new();
    for decl in &file.declarations {
        let rust_name = to_pascal_case(decl.name());
        if let Some(existing) = type_names.insert(rust_name, decl.name().to_string()) {
            return Err(format!(
                "declarations `{existing}` and `{}` collide after Rust name conversion",
                decl.name()
            ));
        }
        if let Decl::Protocol(protocol) = decl {
            let mut method_names = BTreeMap::new();
            for method in &protocol.methods {
                let rust_name = to_snake_case(&method.name);
                if let Some(existing) = method_names.insert(rust_name, method.name.clone()) {
                    return Err(format!(
                        "protocol `{}` methods `{existing}` and `{}` collide after Rust name conversion",
                        protocol.name, method.name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_payload(
    payload: &crate::ast::Payload,
    type_names: &BTreeSet<String>,
    dependency_types: &BTreeSet<String>,
) -> Result<(), String> {
    match payload {
        crate::ast::Payload::Struct(payload) => validate_fields(
            payload.fields.iter().map(|field| (&field.name, &field.ty)),
            type_names,
            dependency_types,
        ),
        crate::ast::Payload::Type(ty) => validate_type(ty, type_names, dependency_types),
    }
}

fn validate_fields<'a>(
    fields: impl Iterator<Item = (&'a String, &'a TypeRef)>,
    type_names: &BTreeSet<String>,
    dependency_types: &BTreeSet<String>,
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for (name, ty) in fields {
        if !valid_ident(name) {
            return Err(format!("invalid field name `{name}`"));
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicate field `{name}`"));
        }
        validate_type(ty, type_names, dependency_types)?;
    }
    Ok(())
}

fn validate_type(
    ty: &TypeRef,
    type_names: &BTreeSet<String>,
    dependency_types: &BTreeSet<String>,
) -> Result<(), String> {
    match &ty.kind {
        TypeKind::Primitive(_) => {
            if ty.nullable {
                return Err("primitive types cannot be nullable in FIDL v1".to_string());
            }
        }
        TypeKind::Identifier(name) => {
            let short = name.rsplit('.').next().unwrap_or(name);
            if name.contains('.') && dependency_types.contains(name) {
                return Ok(());
            }
            if name.contains('.') && !dependency_types.contains(name) {
                return Err(format!("unknown imported type `{name}`"));
            }
            if !type_names.contains(short) && !type_names.contains(name) {
                return Err(format!("unknown type `{name}`"));
            }
        }
        TypeKind::String(_) => {}
        TypeKind::Vector(element, _) | TypeKind::Array(element, _) => {
            validate_type(element, type_names, dependency_types)?
        }
        TypeKind::ClientEnd(protocol) | TypeKind::ServerEnd(protocol) => {
            if protocol.is_empty() {
                return Err("endpoint type requires a protocol name".to_string());
            }
        }
        TypeKind::Handle(_) => {}
    }
    Ok(())
}

fn validate_union(
    union: &Union,
    type_names: &BTreeSet<String>,
    dependency_types: &BTreeSet<String>,
) -> Result<(), String> {
    if !union.strict {
        return Err(format!("union `{}` must be strict in FIDL v1", union.name));
    }
    let mut names = BTreeSet::new();
    let mut ordinals = BTreeSet::new();
    for member in &union.members {
        if member.ordinal == 0 {
            return Err(format!("union `{}` has zero member ordinal", union.name));
        }
        if !ordinals.insert(member.ordinal) {
            return Err(format!(
                "union `{}` has duplicate member ordinal {}",
                union.name, member.ordinal
            ));
        }
        if !valid_ident(&member.name) {
            return Err(format!("invalid union member name `{}`", member.name));
        }
        if !names.insert(member.name.clone()) {
            return Err(format!(
                "union `{}` has duplicate member `{}`",
                union.name, member.name
            ));
        }
        validate_type(&member.ty, type_names, dependency_types)?;
    }
    Ok(())
}

fn validate_enum(en: &Enum) -> Result<(), String> {
    validate_int_backing_type(en.ty.as_ref(), "enum")?;
    let mut names = BTreeSet::new();
    let mut values = BTreeSet::new();
    for member in &en.members {
        if !names.insert(member.name.clone()) {
            return Err(format!(
                "enum `{}` has duplicate member `{}`",
                en.name, member.name
            ));
        }
        if let Some(value) = member.value {
            if !values.insert(value) {
                return Err(format!("enum `{}` has duplicate value {}", en.name, value));
            }
        }
    }
    Ok(())
}

fn validate_bits(bits: &Bits) -> Result<(), String> {
    validate_int_backing_type(bits.ty.as_ref(), "bits")?;
    let mut names = BTreeSet::new();
    let mut values = BTreeSet::new();
    for member in &bits.members {
        if !names.insert(member.name.clone()) {
            return Err(format!(
                "bits `{}` has duplicate member `{}`",
                bits.name, member.name
            ));
        }
        if let Some(value) = member.value {
            let Ok(unsigned) = u64::try_from(value) else {
                return Err(format!(
                    "bits `{}` member `{}` is not a single bit",
                    bits.name, member.name
                ));
            };
            if unsigned == 0 || unsigned.count_ones() != 1 {
                return Err(format!(
                    "bits `{}` member `{}` is not a single bit",
                    bits.name, member.name
                ));
            }
            if !values.insert(value) {
                return Err(format!(
                    "bits `{}` has duplicate value {}",
                    bits.name, value
                ));
            }
        }
    }
    Ok(())
}

fn validate_int_backing_type(ty: Option<&TypeRef>, kind: &str) -> Result<(), String> {
    let Some(ty) = ty else {
        return Ok(());
    };
    match ty.kind {
        TypeKind::Primitive(
            PrimitiveType::Int8
            | PrimitiveType::Int16
            | PrimitiveType::Int32
            | PrimitiveType::Int64
            | PrimitiveType::Uint8
            | PrimitiveType::Uint16
            | PrimitiveType::Uint32
            | PrimitiveType::Uint64,
        ) if !ty.nullable => Ok(()),
        _ => Err(format!(
            "{kind} backing type must be a non-nullable integer"
        )),
    }
}

fn builtin_type_names() -> BTreeSet<String> {
    BTreeSet::from([
        "bool".to_string(),
        "int8".to_string(),
        "int16".to_string(),
        "int32".to_string(),
        "int64".to_string(),
        "uint8".to_string(),
        "uint16".to_string(),
        "uint32".to_string(),
        "uint64".to_string(),
        "float32".to_string(),
        "float64".to_string(),
        "string".to_string(),
        "handle".to_string(),
    ])
}

fn valid_ident(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn to_pascal_case(value: &str) -> String {
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
    if out.is_empty() {
        "field_".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("field_{out}")
    } else {
        out
    }
}
