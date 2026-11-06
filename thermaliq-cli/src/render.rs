//! Human output: plain text tables, one line per fact. `--json` bypasses all of this.

use serde_json::Value;
use std::fmt::Write;

/// A cell: `-` for null, bare strings, compact JSON for anything else.
pub fn cell(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(cell).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// Columns padded to their widest cell; the last column is not padded.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..headers.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([headers[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                s.push_str(c);
            } else {
                let _ = write!(s, "{c:<w$}  ", w = widths[i]);
            }
        }
        s.trim_end().to_owned() + "\n"
    };
    let mut out = line(headers.to_vec());
    for r in rows {
        out += &line(r.iter().map(String::as_str).collect());
    }
    out
}

fn rows_of(v: &Value, cols: &[&str]) -> Vec<Vec<String>> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|r| cols.iter().map(|c| cell(&r[*c])).collect())
        .collect()
}

/// `table` over a JSON array of objects: header `i` shows field `fields[i]`.
pub fn array_table(v: &Value, headers: &[&str], fields: &[&str]) -> String {
    table(headers, &rows_of(v, fields))
}

pub fn status(v: &Value) -> String {
    let state = if v["latched"] == true {
        "LATCHED (every fan held at 100%)"
    } else if v["degraded"] == true {
        "DEGRADED"
    } else if v["engine_running"] != true {
        "ENGINE NOT RUNNING"
    } else {
        "ok"
    };
    let mut s = format!(
        "state:         {state}\nengine:        {}\nprofile:       {}\nagent:         {}\nticks:         {}\nsnapshot age:  {} s\n",
        if v["engine_running"] == true { "running" } else { "stopped" },
        cell(&v["active_profile"]),
        cell(&v["agent"]),
        cell(&v["ticks"]),
        cell(&v["snapshot_age"]),
    );
    for (key, label) in [
        ("degraded_reasons", "degraded"),
        ("waiting_on", "waiting on"),
        ("unavailable_sources", "unavailable"),
    ] {
        for item in v[key].as_array().into_iter().flatten() {
            let _ = writeln!(s, "{label}: {}", cell(item));
        }
    }
    s
}

pub fn profiles(v: &Value) -> String {
    let rows = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            vec![
                if p["active"] == true { "*" } else { "" }.to_owned(),
                cell(&p["name"]),
                cell(&p["description"]),
            ]
        })
        .collect::<Vec<_>>();
    table(&["", "NAME", "DESCRIPTION"], &rows)
}

pub fn config(v: &Value) -> String {
    v.as_object()
        .into_iter()
        .flatten()
        .map(|(k, val)| format!("{k} = {}\n", cell(val)))
        .collect()
}

pub fn identity(v: &Value) -> String {
    let mut s = format!("board: {}\n", cell(&v["board"]));
    for p in v["problems"].as_array().into_iter().flatten() {
        let _ = writeln!(s, "problem: {}", cell(p));
    }
    s += "\nroles:\n";
    for (role, ids) in v["roles"].as_object().into_iter().flatten() {
        let _ = writeln!(s, "  {role} = {}", cell(ids));
    }
    let _ = writeln!(s, "not mapped: {}", cell(&v["unmapped_roles"]));
    s += "\ninventory:\n";
    for key in ["new", "missing", "changed", "board_changed"] {
        let item = &v["inventory"][key];
        if !item.is_null() && item.as_array().is_none_or(|a| !a.is_empty()) {
            let _ = writeln!(s, "  {key}: {}", cell(item));
        }
    }
    s += "\nproposals:\n";
    s += &array_table(
        &v["proposals"],
        &["ROLE", "ID", "WHY"],
        &["role", "id", "why"],
    );
    s += "\nfans:\n";
    s + &array_table(
        &v["fans"],
        &["ID", "LABEL", "KIND", "RPM", "CONTROLLABLE"],
        &["id", "label", "kind", "rpm", "controllable"],
    )
}

/// "12 s ago", "3 min ago", "2 h ago".
pub fn ago(now: i64, at: i64) -> String {
    let s = (now - at).max(0);
    match s {
        0..=119 => format!("{s} s ago"),
        120..=7199 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

pub fn decisions(v: &Value, now: i64) -> String {
    let mut s = format!(
        "hosted account: {}\nhosted error:   {}\nlocal model:    {}\nforeground app: {}\nslow layer:     {}\n",
        cell(&v["hosted_account"]),
        cell(&v["hosted_error"]),
        cell(&v["local"]),
        if v["foreground"] == true { "visible" } else { "not visible (nothing is decided)" },
        if v["crashed"] == true { "CRASHED" } else { "running" },
    );
    s += "\nquestions:\n";
    s += &array_table(
        &v["questions"],
        &["EXE", "SUGGESTED", "BY", "CONFIDENCE"],
        &["exe", "suggested_workload", "suggested_by", "confidence"],
    );
    s += "\ndecisions:\n";
    let rows: Vec<Vec<String>> = v["decisions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let c = |k: &str| cell(&d[k]);
            vec![
                d["at"].as_i64().map_or_else(|| "-".into(), |a| ago(now, a)),
                c("exe"),
                c("workload"),
                c("source"),
                c("confidence"),
                if d["asked"] == true { "asked" } else { "" }.to_owned(),
                c("profile"),
                c("note"),
            ]
        })
        .collect();
    s + &table(
        &[
            "WHEN", "EXE", "WORKLOAD", "SOURCE", "CONF", "", "PROFILE", "NOTE",
        ],
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_pads_every_column_but_the_last() {
        let t = table(
            &["A", "BB"],
            &[
                vec!["xxx".into(), "1".into()],
                vec!["y".into(), "22".into()],
            ],
        );
        assert_eq!(t, "A    BB\nxxx  1\ny    22\n");
    }

    #[test]
    fn a_cell_shows_null_as_a_dash_and_lists_joined() {
        assert_eq!(cell(&json!(null)), "-");
        assert_eq!(cell(&json!(["a", "b"])), "a, b");
        assert_eq!(cell(&json!(51.5)), "51.5");
    }

    #[test]
    fn ago_picks_a_unit() {
        assert_eq!(ago(100, 90), "10 s ago");
        assert_eq!(ago(1000, 400), "10 min ago");
        assert_eq!(ago(100_000, 90_000), "2 h ago");
        assert_eq!(
            ago(5, 10),
            "0 s ago",
            "a clock step back never goes negative"
        );
    }
}
