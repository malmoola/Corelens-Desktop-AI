//! Nothing leaves for a client until it has been through here.
//!
//! Every tool's result, whatever the tool, and every error message: a
//! client hands what it receives to a model in somebody else's cloud, so a
//! secret that reaches it has left the building. Four layers, then a
//! check; a result that still looks as if it holds a secret is withheld,
//! not sent.
//!
//! 1. The backup scrub (`corelens_collect::scrub`): every vendor's
//!    password, secret, community, key and PSK lines.
//! 2. Every secret the unlocked vault holds, by value, wherever it occurs —
//!    in a banner, a description, a hostname somebody typed it into.
//! 3. Shapes no keyword announces: private-key blocks, crypt hashes,
//!    Anthropic keys.
//! 4. The check: the scrub's own `looks_clean`, and no vault secret left.

use std::sync::OnceLock;

use regex::Regex;

pub const MARK: &str = "<removed-by-corelens>";

/// The vault's secrets, for this call only. Zeroed when dropped.
#[derive(Default)]
pub struct Secrets(Vec<zeroize::Zeroizing<String>>);

impl Secrets {
    pub fn new(mut values: Vec<String>) -> Secrets {
        values.retain(|v| !v.trim().is_empty());
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        values.dedup();
        Secrets(values.into_iter().map(zeroize::Zeroizing::new).collect())
    }
}

/// A result that could not be made safe, and so is not sent.
#[derive(Debug, PartialEq, Eq)]
pub struct Withheld;

fn shapes() -> &'static [(Regex, &'static str)] {
    static S: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    S.get_or_init(|| {
        let r = |p: &str| Regex::new(p).expect("a fixed regex");
        vec![
            (r(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)"), MARK),
            (r(r"\$(?:1|2[aby]?|5|6|7|8|9|14|y)\$[./A-Za-z0-9$=+-]{4,}"), MARK),
            (r(r"sk-ant-[A-Za-z0-9_-]{8,}"), MARK),
        ]
    })
}

fn word_bounded(text: &str, secret: &str) -> String {
    // A secret too short to replace everywhere without wrecking the text is
    // replaced where it stands alone.
    let re = Regex::new(&format!(r"(^|[^A-Za-z0-9]){}($|[^A-Za-z0-9])", regex::escape(secret))).expect("an escaped literal");
    re.replace_all(text, |c: &regex::Captures| format!("{}{MARK}{}", &c[1], &c[2])).into_owned()
}

/// The text with every secret removed, or [`Withheld`].
pub fn gate(text: &str, secrets: &Secrets) -> Result<String, Withheld> {
    let mut out = corelens_collect::scrub::scrub(text);
    for secret in &secrets.0 {
        let s = secret.as_str();
        out = if s.chars().count() >= 4 { out.replace(s, MARK) } else { word_bounded(&out, s) };
    }
    for (re, rep) in shapes() {
        out = re.replace_all(&out, *rep).into_owned();
    }
    let survivor = secrets.0.iter().any(|s| s.chars().count() >= 4 && out.contains(s.as_str()));
    if survivor || !corelens_collect::scrub::looks_clean(&out) {
        return Err(Withheld);
    }
    Ok(out)
}

/// What the client is told when a result was withheld.
pub const WITHHELD: &str = "This result was withheld: after removing passwords, keys and community strings it still looked \
as if it contained one, so it was not sent. The engineer can see it in Corelens. Ask for something narrower \
(a pipe filter, a line range, a different command).";

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        Secrets::new(vec!["fixture-pass-0001".into(), "fixture-enable-0002".into(), "fixture-community-3".into(), "xyz".into()])
    }

    #[test]
    fn configuration_secrets_are_removed_by_keyword() {
        let config = "hostname SW1\nenable secret 9 $9$abcdefghijklmnop\nusername admin privilege 15 secret 5 $1$salt$fakehashfakehash\nsnmp-server community fixture-ro-community RO\nline vty 0 4\n password 7 0822455D0A16\n";
        let out = gate(config, &Secrets::default()).unwrap();
        assert!(out.contains("hostname SW1"));
        for gone in ["$9$abc", "$1$salt", "fixture-ro-community", "0822455D0A16"] {
            assert!(!out.contains(gone), "{gone} survived:\n{out}");
        }
    }

    #[test]
    fn a_vault_secret_is_removed_wherever_it_stands() {
        let text = "banner motd ^ call 555 if fixture-pass-0001 fails ^\ninterface Gi1/0/2\n description fixture-community-3 printer\n hostxyz1 xyz\n";
        let out = gate(text, &secrets()).unwrap();
        assert!(!out.contains("fixture-pass-0001") && !out.contains("fixture-community-3"), "{out}");
        assert!(out.contains("hostxyz1"), "a short secret is not cut out of a longer word: {out}");
        assert!(out.contains(&format!(" {MARK}\n")), "a short secret standing alone is: {out}");
    }

    #[test]
    fn shapes_without_a_keyword_are_removed() {
        let text = "crypto key\n-----BEGIN RSA PRIVATE KEY-----\nMIIEfixturefixturefixture\n-----END RSA PRIVATE KEY-----\nnote $6$saltsalt$hashhashhash and sk-ant-api03-FIXTUREFIXTURE end\n";
        let out = gate(text, &Secrets::default()).unwrap();
        assert!(!out.contains("MIIE") && !out.contains("$6$") && !out.contains("sk-ant-api03"), "{out}");
        assert!(out.contains("end"));
    }

    #[test]
    fn what_cannot_be_made_safe_is_withheld() {
        // A hash fragment too short for the shape rewrite still trips the
        // check, and the whole result is held back rather than half-cleaned.
        assert_eq!(gate("note $9$ab", &Secrets::default()), Err(Withheld));
        assert_eq!(gate("routes\n$9$x\n", &Secrets::default()), Err(Withheld));
        assert!(gate("plain text", &Secrets::default()).is_ok());
    }
}
