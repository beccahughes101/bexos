use std::path::PathBuf;
use std::process::ExitCode;

use bexos_trace_analysis::TraceAnalysis;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(output) => {
            if !output.is_empty() {
                print!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<String>) -> Result<String, String> {
    if args.len() < 2 {
        return Err(
            "usage: trace_analysis <trace.pftrace|trace.fxt> query <sql> | event <name> | category <name> | scalar <sql>"
                .into(),
        );
    }
    let trace = TraceAnalysis::from_file(PathBuf::from(&args[0]))?;
    match args[1].as_str() {
        "query" if args.len() == 3 => trace.query(&args[2]),
        "scalar" if args.len() == 3 => Ok(format!("{}\n", trace.scalar_i64(&args[2])?)),
        "event" if args.len() == 3 => {
            trace.assert_event_present(&args[2])?;
            Ok(String::new())
        }
        "category" if args.len() == 3 => {
            trace.assert_category_present(&args[2])?;
            Ok(String::new())
        }
        _ => Err("invalid trace_analysis command".into()),
    }
}
