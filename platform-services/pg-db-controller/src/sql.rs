use crate::error::{Error, Result};

pub fn is_valid_ident(ident: &str) -> bool {
    !ident.is_empty()
        && ident.len() <= 63
        && ident.bytes().enumerate().all(|(i, b)| {
            matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'_') || (i > 0 && b.is_ascii_digit())
        })
}

pub fn quote_ident(ident: &str) -> Result<String> {
    if !is_valid_ident(ident) {
        return Err(Error::InvalidIdentifier(ident.to_string()));
    }
    Ok(format!("\"{ident}\""))
}

pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
