use super::*;
fn start(c: &mut UnixDebugClient, o: TraceOptions) -> Result {
    c.trace_start_with_format(
        bexos_trace::parse_category_list(&o.categories).ok_or("invalid trace categories")?,
        if o.buffer_mode == "circular" { 2 } else { 1 },
        o.buffer_size_kb,
        if matches!(o.format.as_str(), "perfetto" | "pftrace") {
            1
        } else {
            2
        },
    )?;
    Ok(())
}
pub(super) fn run(c: &mut UnixDebugClient, cmd: TraceCommand, f: Format) -> Result {
    match cmd {
        TraceCommand::Start(o) => start(c, o)?,
        TraceCommand::Stop { output } => write(&output, &c.trace_stop()?)?,
        TraceCommand::Record {
            options,
            output,
            duration_ms,
        } => {
            start(c, options)?;
            std::thread::sleep(std::time::Duration::from_millis(duration_ms));
            write(&output, &c.trace_stop()?)?;
        }
        TraceCommand::Status => {
            let s = c.trace_status()?;
            output::records(
                f,
                &[
                    ("state", "STATE"),
                    ("categories", "CATEGORIES"),
                    ("buffer_size_kb", "BUFFER KiB"),
                    ("format", "FORMAT"),
                    ("producers", "PRODUCERS"),
                    ("events", "EVENTS"),
                    ("dropped", "DROPPED"),
                ],
                vec![
                    json!({"state":match s.state{1=>"idle",2=>"recording",3=>"stopped",_=>"unknown"},"categories":bexos_trace::category_list(s.categories),"buffer_mode":s.buffer_mode,"buffer_size_kb":s.buffer_size_kb,"format":match s.output_format{1=>"perfetto",2=>"fxt",_=>"unknown"},"producers":s.producer_count,"events":s.event_count,"dropped":s.dropped_count}),
                ],
            )?;
        }
    }
    Ok(())
}
