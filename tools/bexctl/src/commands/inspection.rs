use super::*;

pub(super) fn processes(c: &mut UnixDebugClient, wide: bool, format: Format) -> Result {
    let rows = c
        .list_processes()?
        .into_iter()
        .map(|p| {
            let group = match p.resource_group_id {
                Some(id) if !p.resource_group_name.is_empty() => {
                    format!("{} ({id})", p.resource_group_name)
                }
                Some(id) => format!("unknown ({id})"),
                None => "unknown".into(),
            };
            json!({
                "pid": p.pid,
                "state": p.state,
                "name": p.name,
                "package": p.package_id,
                "group": group,
                "main_thread_id": if p.main_thread_id == 0 { None } else { Some(p.main_thread_id) },
                "resource_group_id": p.resource_group_id,
                "resource_group_name": if p.resource_group_name.is_empty() { None } else { Some(p.resource_group_name) },
                "parent_resource_group_id": p.parent_resource_group_id,
                "parent_resource_group_name": if p.parent_resource_group_name.is_empty() { None } else { Some(p.parent_resource_group_name) },
            })
        })
        .collect();
    let mut columns = vec![
        ("pid", "PID"),
        ("state", "STATE"),
        ("name", "PROCESS"),
        ("package", "PACKAGE"),
        ("group", "RESOURCE GROUP"),
    ];
    if matches!(format, Format::Tsv) {
        columns = vec![
            ("pid", "PID"),
            ("state", "STATE"),
            ("name", "PROCESS"),
            ("package", "PACKAGE"),
            ("resource_group_id", "GROUP ID"),
            ("resource_group_name", "RESOURCE GROUP"),
            ("main_thread_id", "THREAD"),
            ("parent_resource_group_id", "PARENT ID"),
            ("parent_resource_group_name", "PARENT"),
        ];
    } else if wide {
        columns.extend([
            ("main_thread_id", "THREAD"),
            ("parent_resource_group_id", "PARENT ID"),
            ("parent_resource_group_name", "PARENT"),
        ]);
    }
    output::records(format, &columns, rows)?;
    Ok(())
}
