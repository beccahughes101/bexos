pub mod ast;
pub mod backends;
pub mod capability;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod validate;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn compile_rust(input: &Path, output: &Path) -> Result<(), String> {
    compile_rust_inputs(&[input.to_path_buf()], output)
}

pub fn compile_rust_inputs(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    compile_rust_inputs_with_deps(inputs, &[], output)
}

pub fn compile_rust_inputs_with_deps(
    inputs: &[PathBuf],
    deps: &[PathBuf],
    output: &Path,
) -> Result<(), String> {
    let mut sources = Vec::new();
    for input in inputs {
        sources.push(
            fs::read_to_string(input)
                .map_err(|err| format!("failed to read {}: {err}", input.display()))?,
        );
    }
    let mut dep_sources = Vec::new();
    for input in deps {
        dep_sources.push(
            fs::read_to_string(input)
                .map_err(|err| format!("failed to read {}: {err}", input.display()))?,
        );
    }
    let rust = compile_rust_sources_with_deps(&sources, &dep_sources)?;
    fs::write(output, rust).map_err(|err| format!("failed to write {}: {err}", output.display()))
}

pub fn compile_rust_source(source: &str) -> Result<String, String> {
    compile_rust_sources(&[source.to_string()])
}

pub fn compile_rust_sources(sources: &[String]) -> Result<String, String> {
    compile_rust_sources_with_deps(sources, &[])
}

pub fn compile_rust_sources_with_deps(
    sources: &[String],
    deps: &[String],
) -> Result<String, String> {
    let ast = parse_sources(sources)?;
    let dep_asts = deps
        .iter()
        .map(|source| parser::parse_file(source))
        .collect::<Result<Vec<_>, _>>()?;
    validate::validate_with_deps(&ast, &dep_asts)?;
    let library = ir::Library::from_ast(ast);
    let bindings = capability::split_library(&library);
    Ok(backends::rust::generate(&library, &bindings))
}

fn parse_sources(sources: &[String]) -> Result<ast::File, String> {
    let mut files = sources
        .iter()
        .map(|source| parser::parse_file(source))
        .collect::<Result<Vec<_>, _>>()?;
    let Some(first) = files.first().cloned() else {
        return Err("expected at least one input .fidl path".to_string());
    };

    let mut usings = Vec::new();
    let mut seen_usings = BTreeSet::new();
    let mut declarations = Vec::new();
    for file in files.drain(..) {
        if file.library != first.library {
            return Err(format!(
                "all input files must use library `{}`, got `{}`",
                first.library, file.library
            ));
        }
        for using in file.usings {
            if seen_usings.insert(using.library.clone()) {
                usings.push(using);
            }
        }
        declarations.extend(file.declarations);
    }

    Ok(ast::File {
        library: first.library,
        usings,
        declarations,
    })
}
