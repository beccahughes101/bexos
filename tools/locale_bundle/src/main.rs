use bexos_locale_compiler::{Source, compile};
fn main() {
    if let Err(e) = run() {
        eprintln!("locale_bundle: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("missing output")?;
    let default = args.next().ok_or("missing default locale")?;
    let mut inputs = Vec::new();
    for path in args {
        let p = std::path::Path::new(&path);
        let locale = p
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or("expected locales/<tag>/<file>.ftl")?
            .to_owned();
        let text = std::fs::read_to_string(p).map_err(|e| format!("{path}: {e}"))?;
        inputs.push((locale, path, text));
    }
    let sources = inputs
        .iter()
        .map(|(locale, path, text)| Source { locale, path, text })
        .collect::<Vec<_>>();
    let bytes = compile(&default, &sources)?;
    std::fs::write(output, bytes).map_err(|e| e.to_string())
}
