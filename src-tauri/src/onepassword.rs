//! 1Password for the secret cards: when an agent asks for a key or password, the card can take it
//! from 1Password through the `op` CLI (with its desktop app integration, so Touch ID unlocks it).
//! The window only ever sees item titles and field names; the value is read here with
//! `op read` and goes straight to the waiting request, like a typed one.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::runtime::State;

/// Room for the Touch ID prompt.
const WAIT: Duration = Duration::from_secs(90);
const MAX_ITEMS: usize = 40;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Items whose title has the text, newest first.
    Search { query: String },
    /// An item's fields, by name, without their values.
    Fields { id: String },
    /// Reads the field and answers the secret request with it.
    Answer {
        request_id: String,
        reference: String,
    },
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub title: String,
    pub vault: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub label: String,
    pub reference: String,
}

async fn op(args: &[&str]) -> Result<Vec<u8>> {
    let mut command = crate::detect::command("op");
    command
        .args(args)
        .env("OP_FORMAT", "json")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(WAIT, command.output())
        .await
        .map_err(|_| Error::agent("1Password did not answer in time."))?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::agent("Install the 1Password CLI (brew install 1password-cli) and turn on \"Integrate with 1Password CLI\" in the 1Password app.")
            } else {
                Error::agent("1Password could not be reached.")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::agent(
            if stderr.contains("not signed in")
                || stderr.contains("no accounts")
                || stderr.contains("authorization")
            {
                "Sign in to 1Password and turn on \"Integrate with 1Password CLI\" in its settings."
            } else {
                "1Password refused the request."
            },
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn items(list: &Value, query: &str) -> Vec<Item> {
    let needle = query.trim().to_lowercase();
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let title = item["title"].as_str()?.to_owned();
            if !needle.is_empty() && !title.to_lowercase().contains(&needle) {
                return None;
            }
            Some(Item {
                id: item["id"].as_str()?.to_owned(),
                title,
                vault: item["vault"]["name"].as_str().unwrap_or("").to_owned(),
            })
        })
        .take(MAX_ITEMS)
        .collect()
}

/// Field names and their `op://` references; values never leave this function.
pub(crate) fn fields(item: &Value) -> Vec<Field> {
    item["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|field| {
            let reference = field["reference"].as_str()?;
            if !reference.starts_with("op://") || field["value"].is_null() {
                return None;
            }
            let label = field["label"]
                .as_str()
                .filter(|label| !label.is_empty())
                .or(field["id"].as_str())?;
            Some(Field {
                label: label.to_owned(),
                reference: reference.to_owned(),
            })
        })
        .collect()
}

pub async fn onepassword_action(state: State<'_, Arc<AppState>>, action: Action) -> Result<Value> {
    state.ensure_running()?;
    match action {
        Action::Search { query } => {
            let list: Value = serde_json::from_slice(&op(&["item", "list"]).await?)
                .map_err(|_| Error::agent("1Password answered something unreadable."))?;
            Ok(serde_json::to_value(items(&list, &query))?)
        }
        Action::Fields { id } => {
            if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(Error::agent("That 1Password item is not valid."));
            }
            let item: Value = serde_json::from_slice(&op(&["item", "get", &id]).await?)
                .map_err(|_| Error::agent("1Password answered something unreadable."))?;
            Ok(serde_json::to_value(fields(&item))?)
        }
        Action::Answer {
            request_id,
            reference,
        } => {
            if !reference.starts_with("op://") || reference.len() > 512 || reference.contains('\0')
            {
                return Err(Error::agent("That 1Password item is not valid."));
            }
            let value = op(&["read", "--no-newline", &reference]).await?;
            let value = String::from_utf8(value)
                .map_err(|_| Error::agent("That 1Password field is not text."))?;
            crate::secrets::respond(&request_id, Some(value))?;
            Ok(Value::Null)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_filter_by_title_and_fields_hide_values() {
        let list = json!([
            {"id": "a1", "title": "GitHub token", "vault": {"name": "Dev"}},
            {"id": "b2", "title": "Banco", "vault": {"name": "Pessoal"}}
        ]);
        let found = items(&list, "git");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].vault, "Dev");
        let item = json!({"fields": [
            {"id": "password", "label": "password", "value": "s3cret", "reference": "op://Dev/GitHub token/password"},
            {"id": "notesPlain", "label": "notes", "reference": "op://Dev/GitHub token/notes"},
            {"id": "x", "label": "", "value": "v", "reference": "https://no"}
        ]});
        let shown = fields(&item);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].label, "password");
        assert!(!serde_json::to_string(&shown).unwrap().contains("s3cret"));
    }
}
