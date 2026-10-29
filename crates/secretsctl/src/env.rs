use std::collections::BTreeMap;

pub fn is_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub fn exports(vars: &BTreeMap<String, String>) -> String {
    vars.iter()
        .map(|(k, v)| format!("export {k}={}\n", quote(v)))
        .collect()
}

pub fn collect(
    secrets: impl IntoIterator<Item = (String, serde_json::Map<String, serde_json::Value>)>,
    prefix: &str,
) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for (path, data) in secrets {
        for (key, value) in data {
            let name = format!("{prefix}{key}");
            if !is_name(&name) {
                eprintln!("skipping {path}:{key}: not a valid environment variable name");
                continue;
            }
            let value = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            vars.insert(name, value);
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_name("DATABASE_URL"));
        assert!(is_name("_x1"));
        assert!(!is_name("1ABC"));
        assert!(!is_name("client-id"));
        assert!(!is_name(""));
    }

    #[test]
    fn quoting_survives_the_shell() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME `x` \"y\""), "'$HOME `x` \"y\"'");
    }

    #[test]
    fn later_paths_win_and_bad_names_are_skipped() {
        let first: serde_json::Map<_, _> =
            serde_json::from_str(r#"{"A":"1","B":"1","bad-key":"x"}"#).unwrap();
        let second: serde_json::Map<_, _> = serde_json::from_str(r#"{"B":"2","N":3}"#).unwrap();
        let vars = collect([("a".into(), first), ("b".into(), second)], "");
        assert_eq!(vars["A"], "1");
        assert_eq!(vars["B"], "2");
        assert_eq!(vars["N"], "3");
        assert!(!vars.contains_key("bad-key"));
    }
}
