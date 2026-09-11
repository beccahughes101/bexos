use std::collections::HashMap;

use anyhow::anyhow;
use starlark::{
    environment::{FrozenModule, Globals, Module},
    eval::{Evaluator, FileLoader},
    syntax::{AstModule, Dialect},
    values::{Value, bytes::StarlarkBytes, dict::DictRef, list::ListRef, tuple::TupleRef},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Load {
    pub label: String,
    pub path: String,
    pub source: String,
}

struct DeclaredLoader {
    modules: HashMap<String, FrozenModule>,
}

impl FileLoader for DeclaredLoader {
    fn load(&self, path: &str) -> starlark::Result<FrozenModule> {
        self.modules
            .get(path)
            .cloned()
            .ok_or_else(|| starlark::Error::new_other(anyhow!("undeclared Starlark load `{path}`")))
    }
}

pub fn compile(
    source_path: &str,
    source: &str,
    entry: &str,
    loads: &[Load],
) -> Result<String, String> {
    let globals = Globals::extended_internal();
    let mut modules = HashMap::new();
    for load in loads {
        if modules.contains_key(&load.label) {
            return Err(format!("duplicate declared Starlark load `{}`", load.label));
        }
        let loader = DeclaredLoader {
            modules: modules.clone(),
        };
        let module = evaluate_module(&load.path, &load.source, &globals, &loader)
            .map_err(|error| format!("evaluate declared load `{}`: {error:#}", load.label))?;
        modules.insert(load.label.clone(), module);
    }

    let loader = DeclaredLoader { modules };
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(source_path, source.to_owned(), &Dialect::Standard)
            .map_err(|error| anyhow!("parse {source_path}: {error}"))?;
        let mut evaluator = Evaluator::new(&module);
        evaluator.set_loader(&loader);
        evaluator
            .eval_module(ast, &globals)
            .map_err(|error| anyhow!("evaluate {source_path}: {error}"))?;
        let function = module
            .get(entry)
            .ok_or_else(|| anyhow!("entry `{entry}` is not exported by {source_path}"))?;
        let value = evaluator
            .eval_function(function, &[], &[])
            .map_err(|error| anyhow!("evaluate entry `{entry}`: {error}"))?;
        render_message(value, 0)
    })
    .map_err(|error: anyhow::Error| error.to_string())
}

fn evaluate_module(
    path: &str,
    source: &str,
    globals: &Globals,
    loader: &DeclaredLoader,
) -> anyhow::Result<FrozenModule> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_owned(), &Dialect::Standard)
            .map_err(|error| anyhow!("parse {path}: {error}"))?;
        {
            let mut evaluator = Evaluator::new(&module);
            evaluator.set_loader(loader);
            evaluator
                .eval_module(ast, globals)
                .map_err(|error| anyhow!("evaluate {path}: {error}"))?;
        }
        module
            .freeze()
            .map_err(|error| anyhow!(format!("{error:?}")))
    })
}

fn render_message(value: Value, indent: usize) -> anyhow::Result<String> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "entry must return a dictionary, received `{}`",
            value.get_type()
        )
    })?;
    render_dict(dict, indent)
}

fn render_dict(dict: DictRef, indent: usize) -> anyhow::Result<String> {
    let mut fields = dict
        .iter()
        .map(|(key, value)| {
            let key = key
                .unpack_str()
                .ok_or_else(|| anyhow!("protobuf field names must be strings"))?
                .to_owned();
            Ok((key, value))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    let mut output = String::new();
    for (field, value) in fields {
        if !is_field_name(&field) {
            return Err(anyhow!("invalid protobuf field name `{field}`"));
        }
        if value.is_none() {
            continue;
        }
        render_field(&mut output, &field, value, indent)?;
    }
    Ok(output)
}

fn is_field_name(field: &str) -> bool {
    let mut bytes = field.bytes();
    match bytes.next() {
        Some(byte) if byte == b'_' || byte.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn render_field(
    output: &mut String,
    field: &str,
    value: Value,
    indent: usize,
) -> anyhow::Result<()> {
    if let Some(values) = ListRef::from_value(value) {
        for value in values.iter() {
            render_field(output, field, value, indent)?;
        }
        return Ok(());
    }
    if let Some(values) = TupleRef::from_value(value) {
        for value in values.iter() {
            render_field(output, field, value, indent)?;
        }
        return Ok(());
    }
    output.push_str(&"  ".repeat(indent));
    output.push_str(field);
    if let Some(message) = DictRef::from_value(value) {
        output.push_str(" {\n");
        output.push_str(&render_dict(message, indent + 1)?);
        output.push_str(&"  ".repeat(indent));
        output.push_str("}\n");
        return Ok(());
    }
    output.push_str(": ");
    output.push_str(&render_scalar(value)?);
    output.push('\n');
    Ok(())
}

fn render_scalar(value: Value) -> anyhow::Result<String> {
    if let Some(bytes) = StarlarkBytes::from_value(value) {
        return Ok(quote_bytes(bytes.as_bytes()));
    }
    if let Some(string) = value.unpack_str() {
        if is_enum_token(string) {
            return Ok(string.to_owned());
        }
        return Ok(quote_bytes(string.as_bytes()));
    }
    if let Some(boolean) = value.unpack_bool() {
        return Ok(boolean.to_string());
    }
    if value.unpack_i32().is_some() || value.get_type() == "int" {
        return Ok(value.to_string());
    }
    Err(anyhow!(
        "unsupported protobuf scalar `{}`",
        value.get_type()
    ))
}

fn is_enum_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_uppercase())
}

fn quote_bytes(bytes: &[u8]) -> String {
    let mut output = String::from("\"");
    for byte in bytes {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(*byte as char),
            _ => output.push_str(&format!("\\{:03o}", byte)),
        }
    }
    output.push('\"');
    output
}

#[cfg(test)]
mod tests {
    use super::{Load, compile};

    #[test]
    fn composes_declared_loads_and_serializes_values() {
        let output = compile(
            "product.star",
            "load(\"//base:configs.star\", \"base\")\ndef product():\n  return base()\n",
            "product",
            &[Load {
                label: "//base:configs.star".to_owned(),
                path: "configs.star".to_owned(),
                source: "def base():\n  return {\"enabled\": True, \"value\": 7, \"payload\": b\"\\x00A\", \"child\": {\"kind\": \"BOOTFS\"}, \"omit\": None}\n".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(
            output,
            "child {\n  kind: BOOTFS\n}\nenabled: true\npayload: \"\\000A\"\nvalue: 7\n"
        );
    }

    #[test]
    fn rejects_undeclared_loads() {
        let error = compile(
            "product.star",
            "load(\"//missing:base.star\", \"base\")\ndef product():\n  return base()\n",
            "product",
            &[],
        )
        .unwrap_err();
        assert!(error.contains("undeclared Starlark load"));
    }

    #[test]
    fn rejects_non_dictionary_entry_values() {
        let error = compile(
            "product.star",
            "def product():\n  return [1]\n",
            "product",
            &[],
        )
        .unwrap_err();
        assert!(error.contains("must return a dictionary"));
    }

    #[test]
    fn serializes_repeated_values_and_omits_none() {
        let output = compile(
            "product.star",
            "def product():\n  return {\"item\": [1, 2], \"omit\": None}\n",
            "product",
            &[],
        )
        .unwrap();
        assert_eq!(output, "item: 1\nitem: 2\n");
    }

    #[test]
    fn rejects_invalid_field_names_and_evaluation_failures() {
        let invalid = compile(
            "product.star",
            "def product():\n  return {\"not-a-field\": 1}\n",
            "product",
            &[],
        )
        .unwrap_err();
        assert!(invalid.contains("invalid protobuf field name"));

        let evaluation = compile(
            "product.star",
            "def product():\n  fail(\"bad composition\")\n",
            "product",
            &[],
        )
        .unwrap_err();
        assert!(evaluation.contains("bad composition"));
    }
}
