//! "Trazer logins deste Mac" on Settings → Conectores while another computer is active
//! (ADR-137): the sign-ins this Mac already has go to that computer, so none is made twice. A
//! server cannot open a browser for a service's sign-in, but it can hold the tokens one made.
//! The tokens leave this Mac's own host only through the window (`connector_export_logins` is a
//! window-only command) and travel on the connection to the other computer.
use crate::backend::Backend;
use crate::i18n::t;
use serde_json::{Value, json};

/// What the page shows afterwards: `{ "statuses": [...], "gained": n }` or `{ "error": "..." }`.
pub fn bring(backend: &Backend) -> Value {
    match carry(backend) {
        Ok(done) => done,
        Err(error) => json!({ "error": error }),
    }
}

fn wait(reply: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match reply.recv_blocking() {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(crate::computer_move::message_of(&error)),
        Err(_) => Err(t("A conexão com o computador caiu.").to_owned()),
    }
}

fn signed_in(statuses: &Value) -> usize {
    statuses.as_array().map_or(0, |list| {
        list.iter()
            .filter(|status| status["state"] == "connected")
            .count()
    })
}

fn status() -> Value {
    json!({ "action": { "type": "status" } })
}

fn carry(backend: &Backend) -> Result<Value, String> {
    let before = wait(backend.request("connector_action", status()))?;
    let logins = wait(backend.request_local("connector_export_logins", json!({})))?;
    if logins.as_array().is_none_or(|list| list.is_empty()) {
        return Err(t("Este Mac não tem logins de conectores para trazer.").to_owned());
    }
    let after = wait(backend.request(
        "connector_action",
        json!({ "action": { "type": "importLogins", "logins": logins } }),
    ))?;
    let gained = signed_in(&after).saturating_sub(signed_in(&before));
    Ok(json!({ "statuses": after, "gained": gained }))
}
