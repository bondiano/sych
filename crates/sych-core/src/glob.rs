//! Path globs of rule `applies_to.paths`: `**` spans segments, `*` and `?` stay in one.

/// Whether `/`-separated `path` matches `pattern`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(seg, tail)| segment(&chars(first), &chars(seg)) && segments(rest, tail)),
    }
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn segment(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| segment(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && segment(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && segment(rest, &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn matches_rule_paths() {
        assert!(glob_match("backend/orders/**", "backend/orders/create.go"));
        assert!(glob_match("backend/orders/**", "backend/orders/a/b.go"));
        assert!(!glob_match("backend/orders/**", "backend/ordersx/a.go"));
        assert!(glob_match("**/*.sql", "db/migrations/001.sql"));
        assert!(glob_match("**/*.sql", "001.sql"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/a/main.rs"));
        assert!(glob_match("src/?.rs", "src/a.rs"));
        assert!(!glob_match("frontend/**", "backend/x.ts"));
        assert!(glob_match("docs/?.md", "docs/ё.md"));
    }
}
