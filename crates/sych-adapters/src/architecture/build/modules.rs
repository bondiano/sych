//! Where a symbol is, by language: the package and the module path its namespaces name, and
//! what kind of item it is.

use sych_core::Language;
use sych_core::architecture::ItemKind;

use crate::architecture::scip::{Kind, Symbol};

/// The package node's name, whether it is test code, and the module path below it.
pub(super) struct Place {
    pub package: String,
    pub test: bool,
    pub modules: Vec<String>,
}

/// Rust: `lib/a/b/` is the library's module `a::b`, `bin/tool/a/` the binary `tool`'s; test and
/// bench targets are test code. Go: the namespace is the import path, below the module's.
/// TypeScript: directories and the file, `index` standing for its directory. Python: the
/// dotted module name.
pub(super) fn place(language: Language, symbol: &Symbol<'_>) -> Option<Place> {
    let namespaces = symbol.namespaces();
    let package = symbol.package.to_string();
    let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    Some(match language {
        Language::Rust => match namespaces.as_slice() {
            ["lib" | "crate", rest @ ..] => Place {
                package,
                test: false,
                modules: owned(rest),
            },
            [
                kind @ ("bin" | "test" | "bench" | "example"),
                target,
                rest @ ..,
            ] => Place {
                package: format!("{target} ({kind})"),
                test: matches!(*kind, "test" | "bench"),
                modules: owned(rest),
            },
            _ => return None,
        },
        Language::Go => {
            let path = namespaces.first()?;
            let rest = if *path == package {
                ""
            } else {
                path.strip_prefix(package.as_str())?.strip_prefix('/')?
            };
            Place {
                package,
                test: false,
                modules: rest
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }
        }
        Language::Typescript => {
            let mut modules: Vec<String> = namespaces
                .iter()
                .map(|n| strip_extension(n).to_owned())
                .collect();
            if modules.last().is_some_and(|m| m == "index") {
                modules.pop();
            }

            Place {
                package,
                test: false,
                modules,
            }
        }
        Language::Python => {
            let dotted = namespaces.first()?;
            // The package named after the project is the project's node itself.
            let mut modules: Vec<String> = dotted.split('.').map(str::to_owned).collect();
            if modules.first() == Some(&package) {
                modules.remove(0);
            }

            Place {
                package,
                test: false,
                modules,
            }
        }
    })
}

fn strip_extension(name: &str) -> &str {
    let name = name.strip_suffix(".d.ts").unwrap_or(name);
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(name, |(stem, _)| stem)
}

/// What the symbol is: `scip.proto`'s `SymbolInformation.Kind` when the indexer says, else its
/// last descriptor. `in_type`: what it is written for is a type. `None` for what is no item:
/// a module, a parameter, a type parameter or a module's own entry.
pub(super) fn kind(
    symbol: &Symbol<'_>,
    information: Option<u64>,
    in_type: bool,
) -> Option<ItemKind> {
    if symbol.inner().is_empty() || symbol.is_impl_block() {
        return (symbol.is_impl_block()).then_some(ItemKind::Impl);
    }

    let by_information = information.and_then(|kind| match kind {
        // Class, Enum, Interface, Struct, Trait, Type, TypeAlias, Union, Object.
        7 | 11 | 21 | 33 | 49 | 53 | 54 | 55 | 59 => Some(ItemKind::Type),
        // EnumMember.
        12 => Some(ItemKind::Variant),
        // Field, Property, StaticField, StaticProperty; Constant, Variable, StaticVariable,
        // StaticDataMember.
        15 | 41 | 79 | 81 | 8 | 61 | 77 | 82 => Some(if in_type {
            ItemKind::Field
        } else {
            ItemKind::Constant
        }),
        // AbstractMethod, Constructor, Function, Getter, Method, Setter, StaticMethod,
        // TraitMethod, ProtocolMethod, SingletonMethod.
        9 | 17 | 18 | 26 | 45 | 66 | 68 | 70 | 76 | 80 => Some(ItemKind::Function),
        // Macro.
        25 => Some(ItemKind::Other),
        _ => None,
    });
    by_information.or(match symbol.kind() {
        Kind::Type => Some(ItemKind::Type),
        Kind::Method => Some(ItemKind::Function),
        Kind::Term if in_type => Some(ItemKind::Field),
        Kind::Term => Some(ItemKind::Constant),
        Kind::Macro => Some(ItemKind::Other),
        Kind::Namespace | Kind::Meta | Kind::Parameter | Kind::TypeParameter => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(language: Language, text: &str) -> Option<(String, bool, Vec<String>)> {
        let symbol = Symbol::parse(text).expect("a symbol").expect("not a local");
        place(language, &symbol).map(|p| (p.package, p.test, p.modules))
    }

    fn path(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn each_language_places_its_symbols() {
        assert_eq!(
            at(
                Language::Rust,
                "rust-analyzer cargo fx 0.1.0 lib/store/Store#"
            ),
            Some(("fx".into(), false, path(&["store"])))
        );
        assert_eq!(
            at(
                Language::Rust,
                "rust-analyzer cargo app 0.1.0 bin/tool/cli/run()."
            ),
            Some(("tool (bin)".into(), false, path(&["cli"])))
        );
        assert_eq!(
            at(Language::Rust, "rust-analyzer cargo app 0.1.0 test/it/"),
            Some(("it (test)".into(), true, path(&[])))
        );
        assert_eq!(
            at(
                Language::Go,
                "scip-go gomod example.com/fx . `example.com/fx/store`/New()."
            ),
            Some(("example.com/fx".into(), false, path(&["store"])))
        );
        assert_eq!(
            at(
                Language::Go,
                "scip-go gomod example.com/fx . `example.com/fx`/Main()."
            ),
            Some(("example.com/fx".into(), false, path(&[])))
        );
        assert_eq!(
            at(
                Language::Go,
                "scip-go gomod github.com/x/y v1 `github.com/x/y/z`/F()."
            ),
            Some(("github.com/x/y".into(), false, path(&["z"])))
        );
        assert_eq!(
            at(
                Language::Typescript,
                "scip-typescript npm fx 1.0.0 src/`store.ts`/Store#"
            ),
            Some(("fx".into(), false, path(&["src", "store"])))
        );
        assert_eq!(
            at(
                Language::Typescript,
                "scip-typescript npm fx 1.0.0 src/`index.ts`/run()."
            ),
            Some(("fx".into(), false, path(&["src"])))
        );
        assert_eq!(
            at(
                Language::Python,
                "scip-python python fx 0 `pkg.store`/Store#"
            ),
            Some(("fx".into(), false, path(&["pkg", "store"])))
        );
    }

    #[test]
    fn what_a_symbol_is() {
        let kind_of = |text: &str, information: Option<u64>, in_type: bool| {
            let symbol = Symbol::parse(text).expect("a symbol").expect("not a local");
            kind(&symbol, information, in_type)
        };
        let ra = "rust-analyzer cargo a 0.1.0 lib/";
        assert_eq!(
            kind_of(&format!("{ra}x/Kind#Ready#"), Some(12), true),
            Some(ItemKind::Variant)
        );
        assert_eq!(
            kind_of(&format!("{ra}x/Kind#Ready#n."), None, true),
            Some(ItemKind::Field)
        );
        assert_eq!(
            kind_of(&format!("{ra}x/LIMIT."), None, false),
            Some(ItemKind::Constant)
        );
        assert_eq!(
            kind_of(&format!("{ra}x/impl#[Q]"), None, false),
            Some(ItemKind::Impl)
        );
        assert_eq!(
            kind_of(&format!("{ra}x/impl#[Q]new()."), Some(80), false),
            Some(ItemKind::Function)
        );
        assert_eq!(kind_of(&format!("{ra}x/"), Some(29), false), None);
        assert_eq!(kind_of(&format!("{ra}x/f().(a)"), None, false), None);
    }
}
