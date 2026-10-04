//! Rendering of command results as json, table or text.

use comfy_table::Table;
use comfy_table::presets::UTF8_HORIZONTAL_ONLY;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Json,
    Table,
    Text,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Json => "json",
            OutputFormat::Table => "table",
            OutputFormat::Text => "text",
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "table" => Ok(OutputFormat::Table),
            "text" => Ok(OutputFormat::Text),
            other => Err(format!(
                "invalid output '{other}': expected json, table or text"
            )),
        }
    }
}

/// One table column: header and JSON pointer into each item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub header: &'static str,
    pub pointer: &'static str,
}

const fn col(header: &'static str, pointer: &'static str) -> Column {
    Column { header, pointer }
}

pub const FEATURE_COLUMNS: &[Column] = &[
    col("KEY", "/key"),
    col("TYPE", "/featureType"),
    col("ENABLED", "/enabled"),
    col("LIFECYCLE", "/lifecycleStage"),
    col("OWNER", "/owner"),
    col("ID", "/id"),
];
pub const APPROVAL_COLUMNS: &[Column] = &[
    col("ID", "/id"),
    col("FEATURE", "/featureId"),
    col("CHANGE", "/changeType"),
    col("STATUS", "/status"),
    col("REQUESTED BY", "/requestedBy"),
    col("CREATED", "/createdAt"),
];
pub const TEAM_COLUMNS: &[Column] = &[
    col("ACTIVE", "/active"),
    col("NAME", "/name"),
    col("ID", "/id"),
];
pub const PROFILE_COLUMNS: &[Column] = &[
    col("ACTIVE", "/active"),
    col("NAME", "/name"),
    col("SESSION", "/session"),
    col("TEAM", "/team"),
    col("URL", "/url"),
];
pub const CONFIG_COLUMNS: &[Column] = &[
    col("NAME", "/name"),
    col("VALUE", "/value"),
    col("SOURCE", "/source"),
];

/// How a result is shown in table and text formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Key/value rows of a JSON object.
    Object,
    /// Always pretty JSON (exports).
    Document,
    /// `{"message": "..."}` shown as plain text.
    Message,
    /// `{"items": [...], "meta": {...}}` shown with these columns.
    List(&'static [Column]),
    /// Lists (`{"items": [...]}` or a bare array) get columns from their first
    /// item; anything else is shown like [`Kind::Object`].
    Auto,
    /// A string printed as it is in every format (shell completions).
    Raw,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub value: Value,
    pub kind: Kind,
    pub exit_code: i32,
    /// Printed on stderr after the command.
    pub warnings: Vec<String>,
}

impl Outcome {
    pub fn new(value: Value, kind: Kind) -> Self {
        Self {
            value,
            kind,
            exit_code: 0,
            warnings: Vec::new(),
        }
    }

    pub fn message(text: impl Into<String>) -> Self {
        Self::new(json!({ "message": text.into() }), Kind::Message)
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(Value::String(text.into()), Kind::Raw)
    }
}

pub fn render(value: &Value, kind: Kind, format: OutputFormat) -> String {
    match (kind, format) {
        (Kind::Raw, _) => value.as_str().unwrap_or_default().trim_end().to_string(),
        (_, OutputFormat::Json) | (Kind::Document, _) => pretty(value),
        (Kind::Message, _) => value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        (Kind::List(columns), OutputFormat::Table) => list_table(value, columns),
        (Kind::List(columns), OutputFormat::Text) => list_text(value, columns),
        (Kind::Object, OutputFormat::Table) => object_table(value),
        (Kind::Object, OutputFormat::Text) => object_text(value),
        (Kind::Auto, format) => auto(value, format),
    }
}

/// Columns shown first when an item has them.
const LEADING_KEYS: [&str; 4] = ["id", "key", "name", "slug"];
const MAX_AUTO_COLUMNS: usize = 6;

fn auto(value: &Value, format: OutputFormat) -> String {
    let items = match value {
        Value::Array(items) => items.as_slice(),
        Value::Object(map) if map.get("items").is_some_and(Value::is_array) => items(value),
        _ => {
            return render(value, Kind::Object, format);
        }
    };
    let Some(Value::Object(first)) = items.first() else {
        return match format {
            OutputFormat::Table => "(no items)".to_string(),
            _ => String::new(),
        };
    };
    let shown = |item: &Value| match item {
        Value::Object(_) => false,
        Value::Array(values) => values.iter().all(|v| !v.is_object() && !v.is_array()),
        _ => true,
    };
    let mut keys: Vec<String> = LEADING_KEYS
        .iter()
        .filter(|key| first.get(**key).is_some_and(shown))
        .map(|key| key.to_string())
        .collect();
    for (key, item) in first {
        if keys.len() >= MAX_AUTO_COLUMNS {
            break;
        }
        if !keys.contains(key) && shown(item) {
            keys.push(key.clone());
        }
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| keys.iter().map(|key| cell(item.get(key))).collect())
        .collect();
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.load_preset(UTF8_HORIZONTAL_ONLY);
            table.set_header(keys.iter().map(|key| header_of(key)));
            for row in rows {
                table.add_row(row);
            }
            let mut out = table.to_string();
            if let Some(total) = value.pointer("/meta/total").and_then(Value::as_i64)
                && (items.len() as i64) < total
            {
                out.push_str(&format!(
                    "\n{} of {total} shown; use --all or --offset for more",
                    items.len()
                ));
            }
            out
        }
        _ => rows
            .iter()
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// `displayName` -> `DISPLAY NAME`.
fn header_of(key: &str) -> String {
    let mut header = String::new();
    for (index, ch) in key.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            header.push(' ');
        }
        if ch == '_' {
            header.push(' ');
        } else {
            header.extend(ch.to_uppercase());
        }
    }
    header
}

/// Display form of one value: `-` for missing or null, arrays joined.
pub fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| cell(Some(item)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(other) => other.to_string(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn items(value: &Value) -> &[Value] {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn list_table(value: &Value, columns: &[Column]) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(columns.iter().map(|column| column.header));
    for item in items(value) {
        table.add_row(
            columns
                .iter()
                .map(|column| cell(item.pointer(column.pointer))),
        );
    }
    let mut out = table.to_string();
    let shown = items(value).len() as i64;
    if let Some(total) = value.pointer("/meta/total").and_then(Value::as_i64)
        && shown < total
    {
        out.push_str(&format!(
            "\n{shown} of {total} shown; use --all or --offset for more"
        ));
    }
    out
}

fn list_text(value: &Value, columns: &[Column]) -> String {
    items(value)
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|column| cell(item.pointer(column.pointer)))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn object_rows(value: &Value) -> Vec<(String, String)> {
    match value.as_object() {
        Some(map) => map
            .iter()
            .map(|(key, item)| {
                let shown = match item {
                    Value::Object(_) => item.to_string(),
                    other => cell(Some(other)),
                };
                (key.clone(), shown)
            })
            .collect(),
        None => vec![("value".to_string(), cell(Some(value)))],
    }
}

fn object_table(value: &Value) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(["KEY", "VALUE"]);
    for (key, shown) in object_rows(value) {
        table.add_row([key, shown]);
    }
    table.to_string()
}

fn object_text(value: &Value) -> String {
    object_rows(value)
        .into_iter()
        .map(|(key, shown)| format!("{key}: {shown}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one_feature(total: i64) -> Value {
        json!({
            "items": [{
                "key": "checkout", "featureType": "SIMPLE", "enabled": true,
                "lifecycleStage": "ACTIVE", "owner": null, "id": "f1"
            }],
            "meta": { "offset": 0, "limit": 50, "total": total }
        })
    }

    #[test]
    fn json_is_pretty_printed() {
        assert_eq!(
            render(&json!({ "a": 1 }), Kind::Object, OutputFormat::Json),
            "{\n  \"a\": 1\n}"
        );
    }

    #[test]
    fn list_table_shows_headers_cells_and_dash_for_null() {
        let out = render(
            &one_feature(1),
            Kind::List(FEATURE_COLUMNS),
            OutputFormat::Table,
        );
        assert!(out.contains("KEY"));
        assert!(out.contains("LIFECYCLE"));
        assert!(out.contains("checkout"));
        assert!(!out.contains("null"));
        assert!(!out.contains("shown"));
    }

    #[test]
    fn list_table_says_when_more_items_exist() {
        let out = render(
            &one_feature(120),
            Kind::List(FEATURE_COLUMNS),
            OutputFormat::Table,
        );
        assert!(out.ends_with("1 of 120 shown; use --all or --offset for more"));
    }

    #[test]
    fn list_text_is_tab_separated_without_header() {
        let out = render(
            &one_feature(1),
            Kind::List(FEATURE_COLUMNS),
            OutputFormat::Text,
        );
        assert_eq!(out, "checkout\tSIMPLE\ttrue\tACTIVE\t-\tf1");
    }

    #[test]
    fn object_text_lists_sorted_key_value_lines() {
        let value = json!({ "value": true, "flagKey": "checkout", "tags": ["a", "b"] });
        assert_eq!(
            render(&value, Kind::Object, OutputFormat::Text),
            "flagKey: checkout\ntags: a, b\nvalue: true"
        );
    }

    #[test]
    fn object_table_has_key_and_value_columns() {
        let out = render(
            &json!({ "status": "ok" }),
            Kind::Object,
            OutputFormat::Table,
        );
        assert!(
            out.contains("KEY")
                && out.contains("VALUE")
                && out.contains("status")
                && out.contains("ok")
        );
    }

    #[test]
    fn message_is_plain_text_outside_json() {
        let outcome = Outcome::message("Logged in");
        assert_eq!(
            render(&outcome.value, outcome.kind, OutputFormat::Table),
            "Logged in"
        );
        assert_eq!(
            render(&outcome.value, outcome.kind, OutputFormat::Text),
            "Logged in"
        );
        assert!(render(&outcome.value, outcome.kind, OutputFormat::Json).contains("\"message\""));
    }

    #[test]
    fn document_is_json_in_every_format() {
        let value = json!({ "team": { "id": "t" } });
        assert_eq!(
            render(&value, Kind::Document, OutputFormat::Table),
            render(&value, Kind::Document, OutputFormat::Json)
        );
    }

    #[test]
    fn output_format_parses_ignoring_case() {
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!(" table ".parse::<OutputFormat>(), Ok(OutputFormat::Table));
        assert!(
            "yaml"
                .parse::<OutputFormat>()
                .unwrap_err()
                .contains("expected json, table or text")
        );
    }

    #[test]
    fn auto_lists_pick_scalar_columns_with_ids_and_names_first() {
        let value = json!({ "items": [
            { "description": "Beta", "nested": { "x": 1 }, "name": "beta", "id": "c1", "tags": ["a"] },
            { "description": null, "name": "gamma", "id": "c2", "extra": true }
        ], "meta": { "offset": 0, "limit": 50, "total": 2 } });
        let out = render(&value, Kind::Auto, OutputFormat::Text);
        assert_eq!(out, "c1\tbeta\tBeta\ta\nc2\tgamma\t-\t-");
        let table = render(&value, Kind::Auto, OutputFormat::Table);
        assert!(
            table.contains("ID")
                && table.contains("NAME")
                && table.contains("DESCRIPTION")
                && table.contains("TAGS")
        );
        assert!(!table.contains("NESTED") && !table.contains("EXTRA"));
    }

    #[test]
    fn auto_handles_bare_arrays_objects_and_empty_lists() {
        let array = json!([{ "slug": "okta", "displayName": "Okta" }]);
        assert_eq!(render(&array, Kind::Auto, OutputFormat::Text), "okta\tOkta");
        let object = json!({ "status": "ok" });
        assert_eq!(
            render(&object, Kind::Auto, OutputFormat::Text),
            "status: ok"
        );
        assert_eq!(
            render(&json!({ "items": [] }), Kind::Auto, OutputFormat::Text),
            ""
        );
        assert_eq!(
            render(&json!([]), Kind::Auto, OutputFormat::Table),
            "(no items)"
        );
    }
}
