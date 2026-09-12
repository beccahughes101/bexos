use bexos_fontd::binding::Client;
use bexos_fontd::{
    index::{Index, Query, Scope, digest, font_id, normalize_family},
    parser::{self, Error},
    resolver::{DisabledResolver, MissingFontResolver},
    runtime::parser_status,
};
use bexos_userspace::service_binding::ServiceBinding;
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use fonts_fidl::{FontFormat, FontStatus, FontStyle};

#[test]
fn pinned_variable_fonts_parse_and_expose_expected_scripts() {
    let inter = parser::parse(include_bytes!(env!("INTER"))).unwrap();
    assert_eq!(inter.len(), 1);
    assert_eq!(normalize_family(&inter[0].family).as_deref(), Some("inter"));
    assert_eq!(inter[0].weight, 400);
    assert_eq!(inter[0].style, FontStyle::Normal);
    assert_ne!(inter[0].scripts & 1, 0);

    let arabic = parser::parse(include_bytes!(env!("NOTO_ARABIC"))).unwrap();
    assert_ne!(arabic[0].scripts & 2, 0);
    let devanagari = parser::parse(include_bytes!(env!("NOTO_DEVANAGARI"))).unwrap();
    assert_ne!(devanagari[0].scripts & 4, 0);
}

#[test]
fn static_system_index_matches_every_pinned_artifact() {
    for configured in bexos_fontd::system::fonts() {
        let parsed = parser::parse(configured.bytes).unwrap();
        assert_eq!(digest(configured.bytes), configured.digest);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            normalize_family(&parsed[0].family),
            normalize_family(configured.family)
        );
        assert_eq!(parsed[0].weight, 400);
        assert_eq!(parsed[0].style, FontStyle::Normal);
        assert_eq!(parsed[0].format, FontFormat::Truetype);
        assert_eq!(parsed[0].scripts & configured.scripts, configured.scripts);
    }
}

#[test]
fn malformed_and_woff2_inputs_are_rejected() {
    assert_eq!(parser::parse(b"wOF2........"), Err(Error::Unsupported));
    assert_eq!(
        parser_status(Error::Unsupported),
        FontStatus::UnsupportedFormat
    );
    assert!(matches!(
        parser::parse(&[0; 12]),
        Err(Error::Unsupported | Error::Bounds)
    ));
    assert_eq!(
        parser::parse(&vec![0; parser::MAX_FONT_BYTES + 1]),
        Err(Error::Bounds)
    );

    let mut too_many_faces = vec![0; 12 + (parser::MAX_FACES + 1) * 4];
    too_many_faces[..4].copy_from_slice(b"ttcf");
    too_many_faces[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    too_many_faces[8..12].copy_from_slice(&((parser::MAX_FACES + 1) as u32).to_be_bytes());
    assert_eq!(parser::parse(&too_many_faces), Err(Error::Bounds));

    let mut out_of_bounds_table = include_bytes!(env!("INTER")).to_vec();
    let font_len = out_of_bounds_table.len() as u32;
    out_of_bounds_table[20..24].copy_from_slice(&font_len.to_be_bytes());
    out_of_bounds_table[24..28].copy_from_slice(&16u32.to_be_bytes());
    assert_eq!(parser::parse(&out_of_bounds_table), Err(Error::Bounds));
}

#[test]
fn missing_font_resolver_is_explicitly_local_only() {
    let query = Query {
        family: "Not Installed".into(),
        weight: 400,
        style: FontStyle::Normal,
        format: FontFormat::Truetype,
    };
    assert_eq!(DisabledResolver.resolve(&query), Err(FontStatus::NotFound));
}

#[test]
fn duplicate_tags_and_partial_table_overlaps_are_rejected() {
    let original = include_bytes!(env!("INTER"));
    let mut duplicate = original.to_vec();
    let first_tag: [u8; 4] = duplicate[12..16].try_into().unwrap();
    duplicate[28..32].copy_from_slice(&first_tag);
    assert_eq!(parser::parse(&duplicate), Err(Error::Invalid));

    let mut overlap = original.to_vec();
    let first_offset = u32::from_be_bytes(overlap[20..24].try_into().unwrap());
    overlap[36..40].copy_from_slice(&(first_offset + 1).to_be_bytes());
    assert!(matches!(
        parser::parse(&overlap),
        Err(Error::Invalid | Error::Bounds)
    ));
}

#[test]
fn a_bounded_ttc_face_is_supported() {
    let font = include_bytes!(env!("INTER"));
    let mut collection = Vec::with_capacity(font.len() + 16);
    collection.extend_from_slice(b"ttcf\0\x01\0\0\0\0\0\x01\0\0\0\x10");
    collection.extend_from_slice(font);
    let count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
    for index in 0..count {
        let record = 16 + 12 + index * 16;
        let offset = u32::from_be_bytes(collection[record + 8..record + 12].try_into().unwrap());
        collection[record + 8..record + 12].copy_from_slice(&(offset + 16).to_be_bytes());
    }
    let faces = parser::parse(&collection).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].format, FontFormat::Collection);
    assert_eq!(faces[0].index, 0);
}

#[test]
fn digest_ids_are_stable_and_collection_indexed() {
    let bytes = include_bytes!(env!("INTER"));
    let first = digest(bytes);
    assert_eq!(first, digest(bytes));
    assert_eq!(font_id(&first, 0), font_id(&first, 0));
    assert_ne!(font_id(&first, 0), font_id(&first, 1));
}

#[test]
fn repeated_index_install_is_idempotent() {
    let bytes = include_bytes!(env!("INTER"));
    let metadata = parser::parse(bytes).unwrap();
    let hash = digest(bytes);
    let mut index = Index::default();
    let first = index.insert(Scope::User(7), hash, &metadata).unwrap();
    let second = index.insert(Scope::User(7), hash, &metadata).unwrap();
    assert_eq!(first, second);
    assert_eq!(index.faces().len(), metadata.len());
}

#[test]
fn matching_normalizes_family_and_prefers_the_callers_font() {
    let bytes = include_bytes!(env!("INTER"));
    let metadata = parser::parse(bytes).unwrap();
    let hash = digest(bytes);
    let mut index = Index::default();
    index.insert(Scope::System, hash, &metadata).unwrap();
    index.insert(Scope::User(7), hash, &metadata).unwrap();
    let query = Query {
        family: "  INTER  ".into(),
        weight: 400,
        style: FontStyle::Normal,
        format: FontFormat::Truetype,
    };
    assert_eq!(index.resolve(7, &query).unwrap().scope, Scope::User(7));
    assert_eq!(index.resolve(8, &query).unwrap().scope, Scope::System);
}

#[test]
fn matching_obeys_style_weight_format_and_cross_user_isolation() {
    let bytes = include_bytes!(env!("INTER"));
    let base = parser::parse(bytes).unwrap()[0].clone();
    let mut index = Index::default();

    let mut normal_400 = base.clone();
    normal_400.weight = 400;
    normal_400.style = FontStyle::Normal;
    normal_400.format = FontFormat::Truetype;
    index
        .insert(Scope::User(7), [1; 32], &[normal_400])
        .unwrap();

    let mut italic_700 = base.clone();
    italic_700.weight = 700;
    italic_700.style = FontStyle::Italic;
    italic_700.format = FontFormat::Truetype;
    index
        .insert(Scope::User(7), [2; 32], &[italic_700])
        .unwrap();

    let mut normal_600_otf = base.clone();
    normal_600_otf.weight = 600;
    normal_600_otf.style = FontStyle::Normal;
    normal_600_otf.format = FontFormat::Opentype;
    index
        .insert(Scope::User(7), [3; 32], &[normal_600_otf])
        .unwrap();

    let italic = index
        .resolve(
            7,
            &Query {
                family: "Inter".into(),
                weight: 400,
                style: FontStyle::Italic,
                format: FontFormat::Truetype,
            },
        )
        .unwrap();
    assert_eq!(italic.digest, [2; 32]);

    let lower_weight = index
        .resolve(
            7,
            &Query {
                family: "Inter".into(),
                weight: 500,
                style: FontStyle::Normal,
                format: FontFormat::Opentype,
            },
        )
        .unwrap();
    assert_eq!(lower_weight.digest, [1; 32]);

    let mut format_index = Index::default();
    let mut ttf = base.clone();
    ttf.format = FontFormat::Truetype;
    let mut otf = base.clone();
    otf.format = FontFormat::Opentype;
    format_index.insert(Scope::System, [4; 32], &[ttf]).unwrap();
    format_index.insert(Scope::System, [5; 32], &[otf]).unwrap();
    assert_eq!(
        format_index
            .resolve(
                0,
                &Query {
                    family: "Inter".into(),
                    weight: 400,
                    style: FontStyle::Normal,
                    format: FontFormat::Opentype,
                },
            )
            .unwrap()
            .digest,
        [5; 32]
    );

    assert!(
        index
            .resolve(
                8,
                &Query {
                    family: "Inter".into(),
                    weight: 400,
                    style: FontStyle::Normal,
                    format: FontFormat::Truetype,
                },
            )
            .is_none()
    );
}

#[test]
fn fallback_is_bounded_and_script_specific() {
    let mut index = Index::default();
    for bytes in [
        include_bytes!(env!("INTER")).as_slice(),
        include_bytes!(env!("NOTO_SANS")).as_slice(),
        include_bytes!(env!("NOTO_ARABIC")).as_slice(),
        include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
    ] {
        index
            .insert(Scope::System, digest(bytes), &parser::parse(bytes).unwrap())
            .unwrap();
    }
    assert!(!index.fallbacks(42, "Latn").unwrap().is_empty());
    assert!(!index.fallbacks(42, "Arab").unwrap().is_empty());
    assert!(!index.fallbacks(42, "Deva").unwrap().is_empty());
    assert!(index.fallbacks(42, "Zzzz").is_none());
}

#[test]
fn fallback_places_caller_fonts_before_the_system_baseline() {
    let inter = include_bytes!(env!("INTER"));
    let noto = include_bytes!(env!("NOTO_SANS"));
    let metadata = parser::parse(inter).unwrap();
    let user_digest = [9; 32];
    let mut index = Index::default();
    index
        .insert(Scope::System, digest(inter), &metadata)
        .unwrap();
    index
        .insert(Scope::System, digest(noto), &parser::parse(noto).unwrap())
        .unwrap();
    index
        .insert(Scope::User(7), user_digest, &metadata)
        .unwrap();

    let fonts = index.fallbacks(7, "Latn").unwrap();
    assert_eq!(fonts.first().unwrap().scope, Scope::User(7));
    assert!(fonts.len() <= 8);
    assert!(fonts.iter().skip(1).all(|font| font.scope == Scope::System));
}

#[test]
fn install_binding_requires_authenticated_uid_and_dedicated_capability() {
    let public =
        ServiceBinding::parse("bexos.fonts.FontProvider|FontProvider|Public|1,2||app.test|7|fg")
            .unwrap();
    let client = Client::from_binding(9, &public).unwrap();
    assert_eq!(client.uid, 7);
    assert!(!client.install);

    let install = ServiceBinding::parse(
        "bexos.fonts.FontProvider|FontProvider|InstallUserFont|3||app.test|7|fg",
    )
    .unwrap();
    assert!(Client::from_binding(10, &install).unwrap().install);
    let unauthenticated = ServiceBinding::parse(
        "bexos.fonts.FontProvider|FontProvider|InstallUserFont|3||app.test||fg",
    )
    .unwrap();
    assert!(Client::from_binding(11, &unauthenticated).is_none());
}

#[test]
fn transplant_retains_indexes_clients_endpoints_and_canonical_vmos() {
    let bytes = include_bytes!(env!("INTER"));
    let hash = digest(bytes);
    let mut source = bexos_fontd::runtime::Runtime::empty();
    source.control = Channel(10);
    source.migration = Some(Channel(11));
    source.vfsd = Channel(12);
    source.usersd = Channel(13);
    source.watcher = Channel(14);
    source.clients.push(Client {
        channel: 15,
        uid: 7,
        methods: vec![1, 2, 3],
        install: true,
    });
    source.loaded_users.insert(7);
    source
        .index
        .insert(Scope::System, hash, &parser::parse(bytes).unwrap())
        .unwrap();
    source.blobs.insert(
        hash,
        bexos_fontd::runtime::Blob {
            handle: 16,
            len: bytes.len() as u64,
        },
    );

    let mut adopted = bexos_fontd::runtime::Runtime::empty();
    for key in source.keys() {
        let record = source.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }
    adopted.validate().unwrap();

    assert_eq!(adopted.index.faces(), source.index.faces());
    assert_eq!(adopted.clients, source.clients);
    assert!(adopted.loaded_users.contains(&7));
    let handles = adopted
        .resources()
        .into_iter()
        .map(|resource| match resource {
            Resource::Handle(handle) => handle,
            _ => 0,
        })
        .collect::<Vec<_>>();
    assert_eq!(handles, vec![10, 12, 13, 11, 14, 15, 16]);
}
