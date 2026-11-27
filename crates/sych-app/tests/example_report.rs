//! Keeps `docs/examples/local-review/report.html` in sync with `review.json` and the embedded
//! single-file HTML report template. Refresh with `SYCH_BLESS=1 cargo test -p sych-app --test example_report`.

use std::env;
use std::fs;
use std::path::PathBuf;

use sych_app::render::Report;
use sych_app::report_view;
use sych_core::{Redactor, ReportExcerpt};
use tempfile::TempDir;

const EXCERPT: &str = "\
package orders

func (s *Store) Save(ctx Context, order Order) error {
\tif err := s.insertOrder(ctx, order); err != nil {
\t\treturn err
\t}
\tgo s.publishOrderCreated(order)
\treturn nil
}";

const REVIEWED_DIFF: &str = "\
diff --git a/backend/orders/store.go b/backend/orders/store.go
--- a/backend/orders/store.go
+++ b/backend/orders/store.go
@@ -4,6 +4,5 @@ func (s *Store) Save(ctx Context, order Order) error {
-\treturn s.withTx(ctx, func(tx *Tx) error {
-\t\tif err := tx.InsertOrder(order); err != nil {
-\t\t\treturn err
-\t\t}
-\t\treturn tx.AppendEvent(\"order-created\", order)
-\t})
+\tif err := s.insertOrder(ctx, order); err != nil {
+\t\treturn err
+\t}
+\tgo s.publishOrderCreated(order)
+\treturn nil
";

const SUGGESTED_PATCH: &str = "\
diff --git a/backend/orders/store.go b/backend/orders/store.go
--- a/backend/orders/store.go
+++ b/backend/orders/store.go
@@ -4,5 +4,6 @@ func (s *Store) Save(ctx Context, order Order) error {
-\tif err := s.insertOrder(ctx, order); err != nil {
-\t\treturn err
-\t}
-\tgo s.publishOrderCreated(order)
-\treturn nil
+\treturn s.withTx(ctx, func(tx *Tx) error {
+\t\tif err := tx.InsertOrder(order); err != nil {
+\t\t\treturn err
+\t\t}
+\t\treturn tx.AppendEvent(\"order-created\", order)
+\t})
";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn local_review_html_example_stays_in_sync() {
    let root = workspace_root();
    let dir = root.join("docs/examples/local-review");
    let raw = fs::read_to_string(dir.join("review.json")).expect("read review.json");
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("parse review.json");
    if let Some(usage) = value["review_result"]["usage_summary"].as_object_mut() {
        usage.entry("reasoning_tokens").or_insert_with(|| 0.into());
        usage
            .entry("unknown_usage_attempts")
            .or_insert_with(|| 0.into());
        usage
            .entry("routes")
            .or_insert_with(|| serde_json::json!([]));
        if let Some(cost) = usage.get_mut("cost").and_then(|c| c.as_object_mut()) {
            cost.entry("reference_estimate").or_insert_with(|| 0.into());
        }
    }

    let report: Report = serde_json::from_value(value).expect("deserialize review.json");
    let empty = TempDir::new().expect("tempdir");
    let redactor = Redactor::new(&[], &[], &[]).expect("redactor");
    let (mut view, _) = report_view::build(&report, empty.path(), &redactor);
    let finding = &mut view.findings[0];
    finding.anchors[0].excerpt = Some(ReportExcerpt {
        first_line: 1,
        text: EXCERPT.to_owned(),
    });
    finding.anchors[0].diff = Some(REVIEWED_DIFF.to_owned());
    finding.suggested_patch = Some(SUGGESTED_PATCH.to_owned());
    view.fixes_patch = Some(SUGGESTED_PATCH.to_owned());

    let html = report_view::html::render(&view).expect("render HTML example");
    let out = dir.join("report.html");
    if env::var_os("SYCH_BLESS").is_some() {
        fs::write(&out, &html).expect("write report.html");
        return;
    }

    let committed = fs::read_to_string(&out).unwrap_or_default();
    assert!(
        committed == html,
        "docs/examples/local-review/report.html is stale; re-run with SYCH_BLESS=1"
    );
}
