//! FTL is parsed only here, in the Bazel execution configuration.
use bexos_locale_catalog::{Node, builder::Builder, kind};
use fluent_syntax::{ast::*, parser};
use std::collections::{BTreeMap, BTreeSet};
pub struct Source<'a> {
    pub locale: &'a str,
    pub path: &'a str,
    pub text: &'a str,
}
pub fn compile(default_locale: &str, sources: &[Source<'_>]) -> Result<Vec<u8>, String> {
    if sources.iter().map(|s| s.text.len()).sum::<usize>() > 16 * 1024 * 1024 {
        return Err("sources exceed 16 MiB".into());
    }
    let mut names = BTreeSet::new();
    let mut compiler = Compiler {
        writer: Builder::default(),
        definitions: BTreeMap::new(),
        references: BTreeMap::new(),
        current: (String::new(), String::new()),
    };
    let mut sources = sources.iter().collect::<Vec<_>>();
    sources.sort_by_key(|s| (s.locale, s.path));
    for source in sources {
        if source.locale.len() > 32
            || source.locale.parse::<icu_locale_core::Locale>().is_err()
            || source
                .locale
                .parse::<icu_locale_core::Locale>()
                .is_ok_and(|l| l.to_string() != source.locale)
        {
            return Err(format!("{}: invalid locale", source.path));
        }
        let resource = parser::parse(source.text)
            .map_err(|(_, errors)| format!("{}: {errors:?}", source.path))?;
        for entry in resource.body {
            match entry {
                Entry::Message(m) => {
                    if !names.insert((source.locale, m.id.name.to_string())) {
                        return Err(format!("duplicate {}:{}", source.locale, m.id.name));
                    }
                    if let Some(value) = m.value {
                        compiler.add(source.locale, m.id.name, &value)?;
                    }
                    for attr in m.attributes {
                        compiler.add(
                            source.locale,
                            &format!("{}.{}", m.id.name, attr.id.name),
                            &attr.value,
                        )?;
                    }
                }
                Entry::Term(t) => {
                    if !names.insert((source.locale, format!("-{}", t.id.name))) {
                        return Err(format!("duplicate term {}:{}", source.locale, t.id.name));
                    }
                    compiler.add(source.locale, &format!("-{}", t.id.name), &t.value)?;
                    for attr in t.attributes {
                        compiler.add(
                            source.locale,
                            &format!("-{}.{}", t.id.name, attr.id.name),
                            &attr.value,
                        )?;
                    }
                }
                Entry::Comment(_) | Entry::GroupComment(_) | Entry::ResourceComment(_) => {}
                _ => return Err(format!("{}: invalid FTL entry", source.path)),
            }
        }
    }
    let mut depths = BTreeMap::new();
    for key in compiler.definitions.keys() {
        compiler.check_references(key, &mut BTreeSet::new(), &mut depths)?;
    }
    compiler
        .writer
        .finish(default_locale)
        .map_err(|e| format!("catalog: {e:?}"))
}
struct Compiler {
    writer: Builder,
    definitions: BTreeMap<(String, String), u32>,
    references: BTreeMap<(String, String), Vec<String>>,
    current: (String, String),
}
impl Compiler {
    fn add(&mut self, locale: &str, key: &str, p: &Pattern<&str>) -> Result<(), String> {
        self.current = (locale.into(), key.into());
        if self.definitions.contains_key(&self.current) {
            return Err(format!("duplicate {locale}:{key}"));
        }
        let root = self.pattern(p)?;
        self.definitions.insert(self.current.clone(), root);
        self.writer
            .entry(locale, key, root)
            .map_err(|e| format!("{e:?}"))
    }
    fn check_references(
        &self,
        key: &(String, String),
        visiting: &mut BTreeSet<(String, String)>,
        depths: &mut BTreeMap<(String, String), usize>,
    ) -> Result<usize, String> {
        if let Some(depth) = depths.get(key) {
            return Ok(*depth);
        }
        if visiting.len() >= 64 || !visiting.insert(key.clone()) {
            return Err(format!("cyclic/deep reference {}:{}", key.0, key.1));
        }
        let mut depth = 1;
        for name in self.references.get(key).into_iter().flatten() {
            let target = (key.0.clone(), name.clone());
            if !self.definitions.contains_key(&target) {
                return Err(format!(
                    "unresolved {}:{} referenced by {}",
                    key.0, name, key.1
                ));
            }
            depth = depth.max(1 + self.check_references(&target, visiting, depths)?);
        }
        if depth > 64 {
            return Err("reference depth exceeds 64".into());
        }
        visiting.remove(key);
        depths.insert(key.clone(), depth);
        Ok(depth)
    }
    fn pattern(&mut self, p: &Pattern<&str>) -> Result<u32, String> {
        let mut children = Vec::new();
        for element in &p.elements {
            children.push(match element {
                PatternElement::TextElement { value } => self.writer.string_node(kind::TEXT, value),
                PatternElement::Placeable { expression } => self.expression(expression)?,
            });
        }
        let (a, b) = self.writer.edge_list(&children);
        Ok(self.writer.node(Node {
            kind: kind::PATTERN,
            a,
            b,
            c: 0,
            d: 0,
        }))
    }
    fn expression(&mut self, e: &Expression<&str>) -> Result<u32, String> {
        match e {
            Expression::Inline(e) => self.inline(e),
            Expression::Select { selector, variants } => {
                let a = self.inline(selector)?;
                let mut children = Vec::new();
                let mut default = None;
                let mut keys = BTreeSet::new();
                for (index, v) in variants.iter().enumerate() {
                    if v.default && default.replace(index as u32).is_some() {
                        return Err("multiple defaults".into());
                    }
                    let (k, text) = match v.key {
                        VariantKey::Identifier { name } => (kind::TEXT, name),
                        VariantKey::NumberLiteral { value } => (kind::NUMBER, value),
                    };
                    if !keys.insert((k, text)) {
                        return Err("duplicate variant".into());
                    }
                    let key = self.writer.string_node(k, text);
                    let value = self.pattern(&v.value)?;
                    children.push(self.writer.node(Node {
                        kind: kind::VARIANT,
                        a: key,
                        b: value,
                        c: 0,
                        d: 0,
                    }));
                }
                let (b, c) = self.writer.edge_list(&children);
                Ok(self.writer.node(Node {
                    kind: kind::SELECT,
                    a,
                    b,
                    c,
                    d: default.ok_or("missing default")?,
                }))
            }
        }
    }
    fn inline(&mut self, e: &InlineExpression<&str>) -> Result<u32, String> {
        Ok(match e {
            InlineExpression::StringLiteral { value } => {
                let mut decoded = String::new();
                fluent_syntax::unicode::unescape_unicode(&mut decoded, value)
                    .map_err(|e| e.to_string())?;
                self.writer.string_node(kind::TEXT, &decoded)
            }
            InlineExpression::NumberLiteral { value } => {
                self.writer.string_node(kind::NUMBER, value)
            }
            InlineExpression::VariableReference { id } => {
                self.writer.string_node(kind::VARIABLE, id.name)
            }
            InlineExpression::MessageReference { id, attribute } => self.reference(
                kind::MESSAGE,
                &format!(
                    "{}{}",
                    id.name,
                    attribute
                        .as_ref()
                        .map(|a| format!(".{}", a.name))
                        .unwrap_or_default()
                ),
                None,
            )?,
            InlineExpression::TermReference {
                id,
                attribute,
                arguments,
            } => self.reference(
                kind::TERM,
                &format!(
                    "-{}{}",
                    id.name,
                    attribute
                        .as_ref()
                        .map(|a| format!(".{}", a.name))
                        .unwrap_or_default()
                ),
                arguments.as_ref(),
            )?,
            InlineExpression::FunctionReference { id, arguments } => {
                if !matches!(id.name, "NUMBER" | "DATETIME") {
                    return Err(format!("unsupported function {}", id.name));
                }
                if arguments.positional.len() != 1 {
                    return Err("format functions require one positional argument".into());
                }
                for arg in &arguments.named {
                    let allowed = if id.name == "NUMBER" {
                        matches!(
                            arg.name.name,
                            "minimumFractionDigits"
                                | "maximumFractionDigits"
                                | "minimumIntegerDigits"
                                | "useGrouping"
                        )
                    } else {
                        matches!(
                            arg.name.name,
                            "dateStyle" | "timeStyle" | "hourCycle" | "calendar"
                        )
                    };
                    if !allowed {
                        return Err(format!("unsupported {} option {}", id.name, arg.name.name));
                    }
                }
                self.reference(kind::FUNCTION, id.name, Some(arguments))?
            }
            InlineExpression::Placeable { expression } => self.expression(expression)?,
        })
    }
    fn reference(
        &mut self,
        k: u32,
        name: &str,
        args: Option<&CallArguments<&str>>,
    ) -> Result<u32, String> {
        if k != kind::FUNCTION {
            self.references
                .entry(self.current.clone())
                .or_default()
                .push(name.into());
        }
        let mut children = Vec::new();
        let mut names = BTreeSet::new();
        if let Some(args) = args {
            for e in &args.positional {
                let c = self.inline(e)?;
                children.push(self.writer.node(Node {
                    kind: kind::ARGUMENT,
                    a: 0,
                    b: 0,
                    c,
                    d: 0,
                }));
            }
            for arg in &args.named {
                if !names.insert(arg.name.name) {
                    return Err("duplicate named argument".into());
                }
                let (a, b) = self.writer.text(arg.name.name);
                let c = self.inline(&arg.value)?;
                children.push(self.writer.node(Node {
                    kind: kind::ARGUMENT,
                    a,
                    b,
                    c,
                    d: 0,
                }));
            }
        }
        let (a, b) = self.writer.text(name);
        let (c, d) = self.writer.edge_list(&children);
        Ok(self.writer.node(Node {
            kind: k,
            a,
            b,
            c,
            d,
        }))
    }
}
