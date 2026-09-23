#![no_main]
use bexos_i18n_client::{Client, MappedData, call};
use bexos_locale_catalog::Formatter;
use bexos_userspace::{Channel, Memory, Startup, log};
use locale_fidl::{FidlDecode, HandleRef};
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let mut startup = Startup::receive(control).unwrap();
    bexos_libc::install_startup(&startup);
    let descriptor = startup.locale.take().expect("locale startup descriptor");
    let (_, rights) = Memory::object_info(descriptor.data).unwrap();
    assert_eq!(rights & 4, 0);
    assert!(Memory::map(descriptor.data, descriptor.data_len, 2 | 4).is_err());
    let provider = Channel(
        startup
            .service_grants
            .iter()
            .find(|g| g.protocol == "LocaleProvider")
            .unwrap()
            .endpoint,
    );
    let mut client = Client::from_descriptor(descriptor).unwrap();
    client.watch(provider).unwrap();
    let response = call(
        provider,
        2,
        &locale_fidl::LocaleProviderGetCldrDataVmoRequest {},
    )
    .unwrap();
    let handles = response
        .handles
        .iter()
        .map(|h| HandleRef { raw: *h })
        .collect::<Vec<_>>();
    let q = locale_fidl::LocaleProviderGetCldrDataVmoResponse::decode(&response.bytes, &handles)
        .unwrap();
    assert_eq!(q.status, locale_fidl::Status::Ok);
    assert_eq!(q.data.len(), 1);
    let duplicate = q.data[0].raw;
    assert_eq!(q.data_len, client.descriptor.data_len);
    let mapped = MappedData::map(duplicate, q.data_len).unwrap();
    // Mapping owns its address-space reference independently of the handle.
    Memory::close(duplicate).unwrap();
    assert!(!mapped.bytes().is_empty());
    let generation = client.generation;
    let f = client.formatting().unwrap();
    let number = f
        .number(
            "1234.5",
            &vec![("minimumFractionDigits".into(), 2u32.into())],
        )
        .unwrap();
    let plural = f.plural("ru", "2").unwrap();
    log(&format!(
        "locale-native: number={number} plural={plural} readonly=true generation={}\n",
        generation
    ));
    drop(f);
    drop(mapped);
    drop(client);
    Startup::ready(control).unwrap();
    bexos_userspace::exit();
}
