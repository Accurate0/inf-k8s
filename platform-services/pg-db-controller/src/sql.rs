use crate::error::{Error, Result};

pub fn quote_ident(ident: &str) -> Result<String> {
    let valid = !ident.is_empty()
        && ident.len() <= 63
        && ident.bytes().enumerate().all(|(i, b)| {
            matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'_') || (i > 0 && b.is_ascii_digit())
        });
    if !valid {
        return Err(Error::InvalidIdentifier(ident.to_string()));
    }
    Ok(format!("\"{ident}\""))
}

pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
