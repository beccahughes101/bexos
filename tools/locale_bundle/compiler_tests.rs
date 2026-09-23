use bexos_locale_catalog::{Arguments, Catalog, Error, Formatter, Resolver, Value};
use bexos_locale_compiler::{Source, compile};
struct TestFormatter;
impl Formatter for TestFormatter {
    fn number(&self, n: &str, _: &Arguments) -> Result<String, Error> {
        Ok(n.into())
    }
    fn datetime(&self, _: &Value, _: &Arguments) -> Result<String, Error> {
        Err(Error::Unsupported)
    }
    fn plural(&self, _: &str, n: &str) -> Result<&'static str, Error> {
        Ok(if n == "1" { "one" } else { "other" })
    }
}
#[test]
fn compiled_terms_attributes_and_fallback() {
    let en = "-brand = BexOS\nhello = Hello { $name }, { -brand }!\n    .title = Title\nitems = { $count ->\n [one] One item\n *[other] { $count } items\n}\n";
    let es = "hello = Hola { $name }!\n";
    let sources = [
        Source {
            locale: "en-US",
            path: "en.ftl",
            text: en,
        },
        Source {
            locale: "es",
            path: "es.ftl",
            text: es,
        },
    ];
    let data = compile("en-US", &sources).unwrap();
    assert_eq!(
        data,
        compile(
            "en-US",
            &[sources[1].clone_source(), sources[0].clone_source()]
        )
        .unwrap()
    );
    let r = Resolver {
        catalog: Catalog::parse(&data).unwrap(),
        languages: vec!["es-MX".into()],
        formatter: TestFormatter,
    };
    assert_eq!(
        r.resolve("hello", &vec![("name".into(), Value::text("Ada"))])
            .unwrap(),
        "Hola Ada!"
    );
    assert_eq!(r.resolve("hello.title", &vec![]).unwrap(), "Title");
    assert_eq!(
        r.resolve("items", &vec![("count".into(), Value::number(3))])
            .unwrap(),
        "3 items"
    );
    assert_eq!(r.text("absent", &vec![]), "⟪absent⟫");
    for end in 0..data.len() {
        assert!(Catalog::parse(&data[..end]).is_err());
    }
}
trait CloneSource {
    fn clone_source(&self) -> Source<'_>;
}
impl CloneSource for Source<'_> {
    fn clone_source(&self) -> Source<'_> {
        Source {
            locale: self.locale,
            path: self.path,
            text: self.text,
        }
    }
}
#[test]
fn rejects_invalid_sources() {
    for text in [
        "broken = {",
        "a = one\na = two",
        "a = { missing }",
        "a = { b }\nb = { a }",
        "a = { UNKNOWN($n) }",
    ] {
        assert!(
            compile(
                "en-US",
                &[Source {
                    locale: "en-US",
                    path: "test.ftl",
                    text
                }]
            )
            .is_err(),
            "{text}"
        );
    }
}

#[test]
fn nested_selectors_exact_numbers_and_plural_fallback() {
    let sources = [
        Source {
            locale: "en-US",
            path: "en.ftl",
            text: r#"
items = { $n ->
    [one] baseline one
   *[other] baseline other
}
nested = { $gender ->
    [female] { $n ->
        [1] she has one
       *[other] she has { $n }
    }
   *[other] they have { $n }
}
exact = { $n ->
    [9007199254740992] wrong
    [9007199254740993] exact
   *[other] other
}
-brand = { $case ->
    [lower] bexos
   *[other] BexOS
}
brand = { -brand(case: "lower") }
"#,
        },
        Source {
            locale: "es",
            path: "es.ftl",
            text: "items = { $n ->\n *[other] español\n}\n",
        },
    ];
    let bytes = compile("en-US", &sources).unwrap();
    let r = Resolver {
        catalog: Catalog::parse(&bytes).unwrap(),
        languages: vec!["es-MX".into()],
        formatter: TestFormatter,
    };
    assert_eq!(
        r.resolve("items", &vec![("n".into(), 1u32.into())])
            .unwrap(),
        "baseline one"
    );
    assert_eq!(
        r.resolve("items", &vec![("n".into(), 2u32.into())])
            .unwrap(),
        "español"
    );
    assert_eq!(
        r.resolve(
            "nested",
            &vec![
                ("n".into(), 2u32.into()),
                ("gender".into(), "female".into())
            ]
        )
        .unwrap(),
        "she has 2"
    );
    assert_eq!(
        r.resolve("exact", &vec![("n".into(), 9007199254740993u64.into())])
            .unwrap(),
        "exact"
    );
    assert_eq!(r.resolve("brand", &vec![]).unwrap(), "bexos");
    assert_eq!(r.resolve("nested", &vec![]), Err(Error::MissingArgument));
}

#[test]
fn malformed_tables_and_evaluation_are_bounded() {
    use bexos_locale_catalog::{Node, builder::Builder, kind};
    let mut b = Builder::default();
    let (a, count) = b.edge_list(&[0]);
    let root = b.node(Node {
        kind: kind::PATTERN,
        a,
        b: count,
        c: 0,
        d: 0,
    });
    b.entry("en-US", "cycle", root).unwrap();
    let bytes = b.finish("en-US").unwrap();
    let r = Resolver {
        catalog: Catalog::parse(&bytes).unwrap(),
        languages: vec!["en-US".into()],
        formatter: TestFormatter,
    };
    assert_eq!(r.resolve("cycle", &vec![]), Err(Error::EvaluationLimit));
    for index in [8, 12, 16, 20, 24, 28] {
        let mut broken = bytes.clone();
        broken[index..index + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Catalog::parse(&broken).is_err());
    }
    assert!(
        compile(
            "en-US",
            &[Source {
                locale: "en-US",
                path: "test",
                text: "a =\n .one = one\na =\n .two = two\n"
            }]
        )
        .is_err()
    );
}
#[test]
fn an_explicit_baseline_keeps_its_priority() {
    let b = compile(
        "en-US",
        &[
            Source {
                locale: "en-US",
                path: "en",
                text: "hello = Hello",
            },
            Source {
                locale: "es",
                path: "es",
                text: "hello = Hola",
            },
        ],
    )
    .unwrap();
    let r = Resolver {
        catalog: Catalog::parse(&b).unwrap(),
        languages: vec!["en-US".into(), "es".into()],
        formatter: TestFormatter,
    };
    assert_eq!(r.text("hello", &vec![]), "Hello");
}

#[test]
fn real_cldr_plural_resolution_and_regional_interpolation() {
    use bexos_locale_formatting::Formatting;
    use bexos_locale_settings::Settings;
    let ru = "n = { $count ->\n [one] один\n [few] несколько\n [many] много\n *[other] другое\n}\nvalue = { NUMBER($count, minimumFractionDigits: 2) }\n";
    let ar = "n = { $count ->\n [zero] صفر\n [one] واحد\n [two] اثنان\n [few] قليل\n [many] كثير\n *[other] آخر\n}\n";
    let bytes = compile(
        "en-US",
        &[
            Source {
                locale: "en-US",
                path: "en",
                text: "n = baseline",
            },
            Source {
                locale: "ru",
                path: "ru",
                text: ru,
            },
            Source {
                locale: "ar",
                path: "ar",
                text: ar,
            },
        ],
    )
    .unwrap();
    for (language, count, expected) in [
        ("ru", 1, "один"),
        ("ru", 2, "несколько"),
        ("ru", 5, "много"),
        ("ar", 0, "صفر"),
        ("ar", 1, "واحد"),
        ("ar", 2, "اثنان"),
        ("ar", 3, "قليل"),
        ("ar", 11, "كثير"),
        ("ar", 100, "آخر"),
    ] {
        let formatter = Formatting::new(
            include_bytes!(env!("TEST_CLDR")),
            Settings {
                region: "de-DE".into(),
                ..Settings::default()
            },
        )
        .unwrap();
        let r = Resolver {
            catalog: Catalog::parse(&bytes).unwrap(),
            languages: vec![language.into()],
            formatter,
        };
        assert_eq!(
            r.resolve("n", &vec![("count".into(), Value::number(count))])
                .unwrap(),
            expected
        );
        if language == "ru" {
            assert_eq!(
                r.resolve("value", &vec![("count".into(), Value::number(1234.5))])
                    .unwrap(),
                "1.234,50"
            );
        }
    }
}
