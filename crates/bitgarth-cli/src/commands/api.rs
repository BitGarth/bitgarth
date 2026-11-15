use std::io::{IsTerminal as _, Read as _};

use reqwest::Method;
use serde_json::{Map, Value};

use crate::args::ApiArgs;
use crate::context::{CliError, Context};

pub(super) fn run(context: &Context, args: ApiArgs) -> Result<(), CliError> {
    let method = Method::from_bytes(args.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| CliError::Usage(format!("unsupported method {}", args.method)))?;
    let trimmed = args.path.trim().trim_start_matches('/');
    let trimmed = trimmed.strip_prefix("api/v1/").unwrap_or(trimmed);
    let (path, query) = trimmed.split_once('?').unwrap_or((trimmed, ""));
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let query: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let query: Vec<(&str, String)> = query
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect();
    let body = request_body(&args)?;

    let session = context.session()?;
    let response = session.call(method, &segments, &query, body.as_ref())?;
    match &args.query {
        Some(filter) => {
            for value in select(&response, filter)? {
                match value {
                    Value::String(text) if std::io::stdout().is_terminal() => {
                        println!("{}", crate::ui::terminal_text(text))
                    }
                    Value::String(text) => println!("{text}"),
                    other => println!("{other}"),
                }
            }
            Ok(())
        }
        None => context.print_json(&response),
    }
}

fn request_body(args: &ApiArgs) -> Result<Option<Value>, CliError> {
    if let Some(input) = &args.input {
        let mut text = String::new();
        if input == "-" {
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|_| CliError::Usage("could not read the body from stdin".to_owned()))?;
        } else {
            text = std::fs::read_to_string(input)
                .map_err(|error| CliError::Usage(format!("could not read {input}: {error}")))?;
        }
        return serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| CliError::Usage(format!("the body is not valid JSON: {error}")));
    }
    if args.fields.is_empty() && args.raw_fields.is_empty() {
        return Ok(None);
    }
    let mut body = Map::new();
    for field in &args.fields {
        let (key, value) = split_field(field)?;
        body.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    for field in &args.raw_fields {
        let (key, value) = split_field(field)?;
        let typed = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
        body.insert(key.to_owned(), typed);
    }
    Ok(Some(Value::Object(body)))
}

fn split_field(field: &str) -> Result<(&str, &str), CliError> {
    field
        .split_once('=')
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| CliError::Usage(format!("field \"{field}\" must look like key=value")))
}

/// A small jq-style selector: `.a.b`, `.list[2]`, `.list[].name`.
fn select<'a>(value: &'a Value, filter: &str) -> Result<Vec<&'a Value>, CliError> {
    let invalid = || {
        CliError::Usage(format!(
            "unsupported --query \"{filter}\"; use paths like .wallets[0].label or .wallets[].id"
        ))
    };
    let mut current = vec![value];
    let path = filter.trim().strip_prefix('.').ok_or_else(invalid)?;
    for part in path.split('.').filter(|part| !part.is_empty()) {
        let (name, indexes) = part
            .split_once('[')
            .map_or((part, ""), |(name, rest)| (name, rest));
        if !name.is_empty() {
            current = current
                .into_iter()
                .filter_map(|value| value.get(name))
                .collect();
        }
        for index in indexes.split('[').filter(|index| !index.is_empty()) {
            let index = index.strip_suffix(']').ok_or_else(invalid)?;
            current = if index.is_empty() {
                current
                    .into_iter()
                    .filter_map(Value::as_array)
                    .flatten()
                    .collect()
            } else {
                let index: usize = index.parse().map_err(|_| invalid())?;
                current
                    .into_iter()
                    .filter_map(|value| value.get(index))
                    .collect()
            };
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::select;
    use serde_json::json;

    #[test]
    fn selects_fields_indexes_and_iterations() {
        let value = json!({"wallets": [{"label": "A", "accounts": [1, 2]}, {"label": "B", "accounts": []}]});
        assert_eq!(
            select(&value, ".wallets[1].label").ok(),
            Some(vec![&json!("B")])
        );
        assert_eq!(
            select(&value, ".wallets[].label").ok(),
            Some(vec![&json!("A"), &json!("B")])
        );
        assert_eq!(
            select(&value, ".wallets[0].accounts[]")
                .ok()
                .map(|v| v.len()),
            Some(2)
        );
        assert_eq!(select(&value, ".").ok(), Some(vec![&value]));
        assert!(select(&value, "wallets").is_err());
        assert!(select(&value, ".wallets[x]").is_err());
    }
}
