use crate::args::Format;
use serde_json::Value;
use std::io::{self, Write};

fn cell(v: &Value) -> String {
    let raw = match v {
        Value::Null => "—".into(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    };
    raw.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
pub fn render(format: Format, columns: &[(&str, &str)], rows: &[Value]) -> String {
    if matches!(format, Format::Json) {
        return format!(
            "{}\n",
            serde_json::to_string_pretty(rows).expect("JSON values serialize")
        );
    }
    let data: Vec<Vec<String>> = rows
        .iter()
        .map(|row| columns.iter().map(|(key, _)| cell(&row[*key])).collect())
        .collect();
    if matches!(format, Format::Tsv) {
        return data.iter().map(|r| format!("{}\n", r.join("\t"))).collect();
    }
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, (_, h))| {
            data.iter()
                .map(|r| r[i].chars().count())
                .max()
                .unwrap_or(0)
                .max(h.len())
        })
        .collect();
    let line = |r: &[String]| {
        r.iter()
            .enumerate()
            .map(|(i, s)| {
                if i + 1 == r.len() {
                    s.clone()
                } else {
                    format!(
                        "{s}{}",
                        " ".repeat(widths[i].saturating_sub(s.chars().count()) + 2)
                    )
                }
            })
            .collect::<String>()
    };
    let mut out = line(
        &columns
            .iter()
            .map(|(_, h)| h.to_string())
            .collect::<Vec<_>>(),
    );
    out.push('\n');
    for r in data {
        out.push_str(&line(&r));
        out.push('\n');
    }
    if rows.is_empty() {
        out.push_str("(no results)\n");
    }
    out
}
pub fn records(format: Format, columns: &[(&str, &str)], rows: Vec<Value>) -> io::Result<()> {
    io::stdout()
        .lock()
        .write_all(render(format, columns, &rows).as_bytes())
}
