//! Whether a `#[cfg(..)]` predicate holds in the analysed build: the host Linux target,
//! `test`, `debug_assertions` and the crate's default features, as the semantic profile
//! configures rust-analyzer. Code under any other predicate is not checked.

use syn::punctuated::Punctuated;
use syn::{Expr, Lit, Meta, Token};

/// `false` for a predicate the analysed build does not satisfy or that is not known here.
pub(super) fn active(meta: &Meta, features: &[String]) -> bool {
    match meta {
        Meta::Path(path) => path.get_ident().is_some_and(|i| {
            matches!(i.to_string().as_str(), "unix" | "test" | "debug_assertions")
        }),
        Meta::NameValue(nv) => {
            let Expr::Lit(lit) = &nv.value else {
                return false;
            };
            let Lit::Str(value) = &lit.lit else {
                return false;
            };
            let value = value.value();
            match nv.path.get_ident().map(ToString::to_string).as_deref() {
                Some("feature") => features.contains(&value),
                Some("target_os") => value == "linux",
                Some("target_family") => value == "unix",
                _ => false,
            }
        }
        Meta::List(list) => {
            let Ok(args) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            else {
                return false;
            };
            match list.path.get_ident().map(ToString::to_string).as_deref() {
                Some("all") => args.iter().all(|m| active(m, features)),
                Some("any") => args.iter().any(|m| active(m, features)),
                // `not` of an unknown predicate is unknown too, so not active.
                Some("not") => args.len() == 1 && known(&args[0]) && !active(&args[0], features),
                _ => false,
            }
        }
    }
}

/// Whether `active` decides `meta` from the build rather than not knowing it.
fn known(meta: &Meta) -> bool {
    match meta {
        Meta::Path(path) => path.get_ident().is_some_and(|i| {
            matches!(
                i.to_string().as_str(),
                "unix" | "windows" | "test" | "debug_assertions"
            )
        }),
        Meta::NameValue(nv) => nv.path.get_ident().is_some_and(|i| {
            matches!(
                i.to_string().as_str(),
                "feature" | "target_os" | "target_family"
            )
        }),
        Meta::List(list) => list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .is_ok_and(|args| args.iter().all(known)),
    }
}

/// Features a default build enables: `default` and what it turns on, transitively;
/// `dep:` and `crate/feature` entries are dependency switches, not features of this crate.
pub(super) fn default_features(manifest: &str) -> Vec<String> {
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(table) = doc.get("features").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut on: Vec<String> = Vec::new();
    let mut todo = vec!["default".to_owned()];
    while let Some(name) = todo.pop() {
        let Some(list) = table.get(&name).and_then(toml::Value::as_array) else {
            continue;
        };
        for item in list.iter().filter_map(toml::Value::as_str) {
            if !item.contains(':') && !item.contains('/') && !on.iter().any(|f| f == item) {
                on.push(item.to_owned());
                todo.push(item.to_owned());
            }
        }
    }
    on
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds(predicate: &str, features: &[&str]) -> bool {
        let meta: Meta = syn::parse_str(predicate).expect("predicate");
        let features: Vec<String> = features.iter().map(|f| (*f).to_owned()).collect();
        active(&meta, &features)
    }

    #[test]
    fn host_test_and_default_features_hold_the_rest_does_not() {
        assert!(holds("unix", &[]));
        assert!(holds("test", &[]));
        assert!(holds("target_os = \"linux\"", &[]));
        assert!(holds("feature = \"metrics\"", &["metrics"]));
        assert!(holds("not(windows)", &[]));
        assert!(holds("all(unix, any(test, feature = \"x\"))", &[]));
        assert!(!holds("windows", &[]));
        assert!(!holds("target_os = \"macos\"", &[]));
        assert!(!holds("feature = \"metrics\"", &[]));
        assert!(!holds("loom", &[]), "an unknown predicate is not analysed");
        assert!(!holds("not(loom)", &[]), "nor is its negation");
    }

    #[test]
    fn default_features_follow_what_default_enables() {
        let manifest = "[package]\nname = \"a\"\n\n[features]\ndefault = [\"std\", \"dep:serde\"]\nstd = [\"alloc\", \"serde/std\"]\nalloc = []\nmetrics = []\n";
        let mut on = default_features(manifest);
        on.sort();
        assert_eq!(on, ["alloc", "std"]);
        assert!(default_features("[package]\nname = \"a\"\n").is_empty());
    }
}
