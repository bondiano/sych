//! Linter selection, unsafe oxlint configs and the actionlint format.

use super::*;

#[test]
fn each_linter_selects_its_files() {
    let picks = |path: &str| -> Vec<&str> {
        LINTERS
            .iter()
            .filter(|l| (l.selects)(path))
            .map(|l| l.tool)
            .collect()
    };
    assert_eq!(picks("app/a.py"), ["ruff"]);
    assert_eq!(picks("web/a.js"), ["oxlint"]);
    assert_eq!(picks("web/a.tsx"), ["oxlint"]);
    assert_eq!(picks("db/migrations/1.sql"), ["sqruff"]);
    assert_eq!(picks("src/Order.php"), ["mago"]);
    assert_eq!(picks("R/orders.R"), ["jarl"]);
    assert_eq!(picks("R/orders.r"), ["jarl"]);
    assert_eq!(picks(".github/workflows/ci.yml"), ["actionlint", "zizmor"]);
    assert_eq!(picks("tools/setup/action.yaml"), ["zizmor"]);
    assert!(picks("README.md").is_empty() && picks("config.yml").is_empty());
}

#[test]
fn js_plugins_or_js_configs_disable_the_project_oxlint_config() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::write(root.path().join(".oxlintrc.json"), r#"{"rules": {}}"#).expect("write");
    assert!(!run::oxlint_runs_code(root.path()));
    fs::create_dir_all(root.path().join("pkg")).expect("dir");
    fs::write(
        root.path().join("pkg/.oxlintrc.json"),
        r#"{"jsPlugins": ["./plugin.js"]}"#,
    )
    .expect("write");
    assert!(run::oxlint_runs_code(root.path()));
    let ts = tempfile::tempdir().expect("tempdir");
    fs::write(ts.path().join("oxlint.config.ts"), "export default {}").expect("write");
    assert!(run::oxlint_runs_code(ts.path()));
}

#[test]
fn actionlint_json_becomes_diagnostics() {
    let text = r#"[{"message":"property \"missing\" is not defined","filepath":".github/workflows/ci.yml","line":10,"column":17,"kind":"expression"}]"#;
    let (found, _) = run::actionlint(text).expect("parses");
    assert_eq!(found[0].path, ".github/workflows/ci.yml");
    assert_eq!(found[0].line, 10);
    assert_eq!(found[0].code.as_deref(), Some("actionlint::expression"));
    assert!(run::actionlint("").expect("no findings").0.is_empty());
    assert!(run::actionlint("not json").is_err());
}
