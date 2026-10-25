//! Provider API keys: in the operating system's keychain, read and written
//! only here, never returned to the page, never logged, never in an
//! export.
//!
//! A key reaches Corelens by the clipboard: the engineer copies it, presses
//! *Paste key*, and Rust reads the clipboard, checks the key's shape,
//! keeps it in the keychain and empties the clipboard. A password field
//! would have carried the key through the page; this never does.

use zeroize::Zeroizing;

use crate::keychain::SERVICE;

fn account(provider: &str) -> Result<&'static str, String> {
    match provider {
        "anthropic" => Ok("anthropic-api-key"),
        "openai" => Ok("openai-api-key"),
        _ => Err(format!("{provider} takes no key.")),
    }
}

fn entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account(provider)?).map_err(|e| format!("The system keychain is not available: {e}"))
}

/// The key, when one is kept. Every key read is remembered by the log's
/// scrub, so no line can carry it.
pub fn read(provider: &str) -> Result<Option<Zeroizing<String>>, String> {
    match entry(provider)?.get_password() {
        Ok(k) => {
            crate::applog::remember_secret(&k);
            Ok(Some(Zeroizing::new(k)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("The system keychain could not be read: {e}")),
    }
}

pub fn present(provider: &str) -> bool {
    read(provider).map(|k| k.is_some()).unwrap_or(false)
}

pub fn forget(provider: &str) -> Result<(), String> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("The key could not be removed from the system keychain: {e}")),
    }
}

/// Whether `text` looks like a key for `provider`, and the key trimmed.
pub fn check(provider: &str, text: &str) -> Result<Zeroizing<String>, String> {
    let key = text.trim();
    if key.is_empty() {
        return Err("The clipboard is empty: copy the API key first, then press Paste key.".into());
    }
    if key.chars().any(char::is_whitespace) || key.len() < 20 || key.len() > 400 {
        return Err("What is on the clipboard does not look like an API key (one line, no spaces). Copy the key itself.".into());
    }
    match provider {
        "anthropic" if !key.starts_with("sk-ant-") => Err("That is not an Anthropic API key (they start sk-ant-). Copy it from the Claude Console.".into()),
        "openai" | "anthropic" => Ok(Zeroizing::new(key.to_string())),
        _ => Err(format!("{provider} takes no key.")),
    }
}

/// Reads the key from the clipboard, keeps it, empties the clipboard.
pub fn import_from_clipboard(provider: &str) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("The clipboard could not be read: {e}"))?;
    let text = Zeroizing::new(clip.get_text().map_err(|_| "The clipboard holds no text: copy the API key first.".to_string())?);
    let key = check(provider, &text)?;
    crate::applog::remember_secret(&key);
    entry(provider)?.set_password(&key).map_err(|e| format!("The key could not be kept in the system keychain: {e}"))?;
    // The key is in the keychain now; it does not stay on the clipboard.
    let _ = clip.clear();
    crate::applog::info("ai", &format!("{provider} key stored in the system keychain"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_checked_for_its_shape_before_it_is_kept() {
        assert!(check("anthropic", "  sk-ant-api03-FIXTUREfixtureFIXTUREfixture  ").is_ok());
        assert!(check("anthropic", "sk-proj-FIXTUREfixtureFIXTURE").unwrap_err().contains("sk-ant-"));
        assert!(check("openai", "sk-proj-FIXTUREfixtureFIXTURE").is_ok());
        assert!(check("openai", "").unwrap_err().contains("empty"));
        assert!(check("openai", "a whole paragraph of text that is not a key").is_err());
        assert!(check("ollama", "sk-proj-FIXTUREfixtureFIXTURE").is_err());
    }
}
