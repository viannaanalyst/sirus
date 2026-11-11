//! Credential masking for what the app keeps and shows (ADR-077): activity details and
//! command output are masked before they are stored. Known token shapes keep their prefix
//! and last four characters (`sk-••••abcd`); credential-named flags, assignments, headers
//! and URL passwords lose their value. Mirrors `src/lib/redact.ts`.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

pub const DOTS: &str = "••••";

/// `••••` plus the last four characters, or only `••••` under 16 characters.
pub fn mask_value(value: &str) -> String {
    let count = value.chars().count();
    if count < 16 {
        return DOTS.into();
    }
    let tail: String = value.chars().skip(count - 4).collect();
    format!("{DOTS}{tail}")
}

fn words(name: &str) -> Vec<String> {
    let mut out = vec![];
    let mut current = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let boundary = c.is_ascii_uppercase()
            && index > 0
            && (chars[index - 1].is_ascii_lowercase()
                || chars[index - 1].is_ascii_digit()
                || (chars[index - 1].is_ascii_uppercase()
                    && chars.get(index + 1).is_some_and(char::is_ascii_lowercase)));
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// True when a key, flag or variable names a credential rather than benign metadata
/// (`max_tokens` and `token_count` are counters, `GITHUB_TOKEN` and `apiKey` are not).
pub fn sensitive_name(name: &str) -> bool {
    let words = words(name);
    let joined: String = words.concat();
    if matches!(
        joined.as_str(),
        "apikey"
            | "accesskey"
            | "accesskeyid"
            | "authtoken"
            | "authorization"
            | "clientsecret"
            | "privatekey"
            | "secretkey"
            | "sessiontoken"
            | "refreshtoken"
            | "idtoken"
            | "token"
            | "pat"
    ) {
        return true;
    }
    let Some(last) = words.last().map(String::as_str) else {
        return false;
    };
    let qualifier = words.len().checked_sub(2).map(|i| words[i].as_str());
    match last {
        "password" | "passwd" | "pwd" | "pass" | "passphrase" | "secret" | "secrets"
        | "credential" | "credentials" | "authorization" | "cookie" => true,
        "token" => qualifier.is_none_or(|q| {
            !matches!(
                q,
                "max" | "prompt" | "completion" | "total" | "input" | "output" | "csrf" | "next"
            )
        }),
        "key" => words[..words.len() - 1].iter().any(|w| {
            matches!(
                w.as_str(),
                "api" | "private" | "secret" | "access" | "signing" | "encryption" | "master"
            )
        }),
        _ => false,
    }
}

fn shannon(value: &str) -> f64 {
    let mut counts = [0u32; 128];
    let mut total = 0f64;
    for byte in value.bytes().filter(u8::is_ascii) {
        counts[byte as usize] += 1;
        total += 1.0;
    }
    counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let p = f64::from(*count) / total;
            -p * p.log2()
        })
        .sum()
}

/// A long random-looking value: letters and digits mixed, no spaces or paths, high entropy.
pub fn looks_random(value: &str) -> bool {
    value.len() >= 24
        && !value.contains("://")
        && !value.starts_with(['/', '.', '~', '$'])
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=_-.".contains(c))
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && shannon(value) >= 3.5
}

/// Values that are references, not secrets: `$TOKEN`, `${{ secrets.X }}`, `<token>`, masked ones.
fn placeholder(value: &str) -> bool {
    let bare = value.trim_matches(['"', '\'']);
    bare.is_empty()
        || bare.starts_with('$')
        || bare.starts_with('<')
        || bare.starts_with('-')
        || bare.contains(DOTS)
        || matches!(
            bare.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "none" | "***" | "bearer" | "basic" | "digest" | "token"
        )
}

/// Masks a possibly quoted value, keeping its quotes.
fn mask_quoted(value: &str) -> String {
    let quote = value
        .chars()
        .next()
        .filter(|c| (*c == '"' || *c == '\'') && value.len() >= 2 && value.ends_with(*c));
    match quote {
        Some(q) => format!("{q}{}{q}", mask_value(&value[1..value.len() - 1])),
        None => mask_value(value),
    }
}

static URL_PASSWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:)([^\s@/]+)@").unwrap());
static PREFIXED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(sk-(?:proj-|ant-(?:api\d\d-)?|svcacct-)?|gh[pousr]_|github_pat_|xox[abposr]-|glpat-|npm_|[sr]k_(?:live|test)_|AIza|cf(?:ast|at|ut|k)_|hf_|ya29\.)([A-Za-z0-9_\-]{12,})").unwrap()
});
static JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(eyJ)([A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}(?:\.[A-Za-z0-9_\-]*)?)").unwrap()
});
/// A private key block, or (when a bounded output kept only its tail) the body before the END line.
static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)|(?:[A-Za-z0-9+/=]{16,}\s+)+-----END [A-Z0-9 ]*PRIVATE KEY-----").unwrap()
});
static AWS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(AKIA|ASIA)([0-9A-Z]{16})\b").unwrap());
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(\b(?:proxy-)?authorization\b["']?\s*[:=]\s*["']?(?:(?:bearer|basic|token|digest)\s+)?|\bbearer\s+)([A-Za-z0-9._~+/=\-]{6,})"#).unwrap()
});
static FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\s|^)(--?[A-Za-z][A-Za-z0-9_-]*)(=|\s+)("[^"]*"|'[^']*'|[^\s"'-][^\s"']*)"#)
        .unwrap()
});
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(["']?)([A-Za-z_][A-Za-z0-9_.\-]*)(["']?\s*(?::|=)\s*)("[^"\n]*"|'[^'\n]*'|[^\s"',;&|)}\]]+)"#).unwrap()
});

/// Masks credentials in free text. Cheap when nothing matches.
pub fn mask(text: &str) -> Cow<'_, str> {
    if text.is_empty() {
        return Cow::Borrowed(text);
    }
    let text = URL_PASSWORD.replace_all(text, |c: &Captures| {
        format!("{}{}@", &c[1], mask_value(&c[2]))
    });
    let text = PREFIXED
        .replace_all(&text, |c: &Captures| {
            format!("{}{}", &c[1], mask_value(&c[2]))
        })
        .into_owned();
    let text = JWT
        .replace_all(&text, |c: &Captures| {
            format!("{}{}", &c[1], mask_value(&c[2]))
        })
        .into_owned();
    let text = PEM
        .replace_all(&text, "-----PRIVATE KEY ••••-----")
        .into_owned();
    let text = AWS
        .replace_all(&text, |c: &Captures| {
            format!("{}{}", &c[1], mask_value(&c[2]))
        })
        .into_owned();
    let text = AUTHORIZATION
        .replace_all(&text, |c: &Captures| {
            if placeholder(&c[2]) {
                c[0].to_string()
            } else {
                format!("{}{}", &c[1], mask_value(&c[2]))
            }
        })
        .into_owned();
    let text = FLAG
        .replace_all(&text, |c: &Captures| {
            let name = c[2].trim_start_matches('-');
            if sensitive_name(name) && !placeholder(&c[4]) {
                format!("{}{}{}{}", &c[1], &c[2], &c[3], mask_quoted(&c[4]))
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    let masked = ASSIGNMENT
        .replace_all(&text, |c: &Captures| {
            let value = &c[4];
            let separator = &c[3];
            let bare = value.trim_matches(['"', '\'']);
            let env_style = separator.trim() == "="
                && c[2]
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_');
            let hide =
                !placeholder(value) && (sensitive_name(&c[2]) || (env_style && looks_random(bare)));
            if hide {
                format!("{}{}{}{}", &c[1], &c[2], separator, mask_quoted(value))
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    Cow::Owned(masked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_token_shapes_keep_prefix_and_last_four() {
        assert_eq!(
            mask("key sk-proj-abcdefghijklmnop1234 ok"),
            "key sk-proj-••••1234 ok"
        );
        assert_eq!(mask("ghp_0123456789abcdefghijABCD"), "ghp_••••ABCD");
        assert_eq!(mask("xoxb-1234567890-abcdefgh"), "xoxb-••••efgh");
        assert_eq!(mask("AKIAABCDEFGHIJKLMNOP"), "AKIA••••MNOP");
        assert_eq!(
            mask("github_pat_11ABCDEFG0123456789_xyz"),
            "github_pat_••••_xyz"
        );
        assert_eq!(
            mask("sk-ant-api03-aaaaaaaaaaaaaaaa9999"),
            "sk-ant-api03-••••9999"
        );
        assert_eq!(mask("cfast_0123456789abcdefghij"), "cfast_••••ghij");
        assert_eq!(
            mask("t=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghij"),
            "t=eyJ••••ghij"
        );
        assert_eq!(
            mask(
                "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\nb"
            ),
            "a\n-----PRIVATE KEY ••••-----\nb"
        );
        assert!(
            !mask("-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEF").contains("MIIEvQ")
        );
        assert!(
            !mask(
                "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nKcwggSjAgEAAoIBAQC7\n-----END PRIVATE KEY-----"
            )
            .contains("KcwggSj")
        );
        // Words that only contain a prefix stay.
        assert_eq!(
            mask("task-runner and desk-lamp"),
            "task-runner and desk-lamp"
        );
    }

    #[test]
    fn flags_headers_assignments_and_urls_lose_their_values() {
        assert_eq!(
            mask("mysql --password hunter2 -u root"),
            "mysql --password •••• -u root"
        );
        assert_eq!(mask("login --token=abc123"), "login --token=••••");
        assert_eq!(mask("run --verbose --dry-run"), "run --verbose --dry-run");
        assert_eq!(
            mask("curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig'"),
            "curl -H 'Authorization: Bearer ••••.sig'"
        );
        assert_eq!(
            mask("GITHUB_TOKEN=abc123 npm publish"),
            "GITHUB_TOKEN=•••• npm publish"
        );
        assert_eq!(
            mask("export DB_PASSWORD=\"correct horse\""),
            "export DB_PASSWORD=\"••••\""
        );
        assert_eq!(
            mask(r#"{"apiKey": "abcdefghijklmnop"}"#),
            r#"{"apiKey": "••••mnop"}"#
        );
        assert_eq!(
            mask("git clone https://user:s3cretpass@github.com/x/y"),
            "git clone https://user:••••@github.com/x/y"
        );
        assert_eq!(
            mask("SESSION_SALT=Zx8kP2qLm9Rt4Vw7Yb3Nc6Hd1Jf5Gs0A"),
            "SESSION_SALT=••••Gs0A"
        );
    }

    #[test]
    fn counters_references_and_ordinary_text_are_kept() {
        for text in [
            "max_tokens=4096",
            "prompt_tokens: 120",
            "TOKEN=$GITHUB_TOKEN",
            "password: ${{ secrets.DB }}",
            "NODE_ENV=production",
            "PATH=/usr/local/bin:/usr/bin",
            "BUILD_ID=2026-10-07",
            "see https://example.com/docs",
            "npm test -- --watch",
        ] {
            assert_eq!(mask(text), text, "{text}");
        }
        assert!(matches!(mask(""), Cow::Borrowed(_)));
    }

    #[test]
    fn names_are_classified_by_their_last_word() {
        for name in [
            "GITHUB_TOKEN",
            "apiKey",
            "db-password",
            "client_secret",
            "AWS_SECRET_ACCESS_KEY",
            "Authorization",
            "token",
            "x-api-key",
        ] {
            assert!(sensitive_name(name), "{name}");
        }
        for name in [
            "max_tokens",
            "tokenizer",
            "keyboard",
            "primary_key",
            "monkey",
            "passenger",
            "user",
        ] {
            assert!(!sensitive_name(name), "{name}");
        }
        assert_eq!(mask_value("short"), "••••");
        assert_eq!(mask_value("abcdefghijklmnop"), "••••mnop");
    }
}
