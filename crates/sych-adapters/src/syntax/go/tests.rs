//! Shapes `facts` finds in Go sources.

use super::*;

const STORE: &str = "package orders\n\nimport \"database/sql\"\n\ntype Store struct {\n\tdb *sql.DB\n\tevents Publisher\n}\n\ntype Publisher interface {\n\tPublish(e Event) error\n}\n";

fn found(source: &str, wrappers: &[(String, String)]) -> Vec<Raw> {
    let package = PackageTypes::new([STORE, source]);
    facts(source, &package, wrappers).expect("parses")
}

fn of(raws: &[Raw], kind: SyntaxKind) -> Vec<&Raw> {
    raws.iter().filter(|r| r.kind == kind).collect()
}

#[test]
fn sql_calls_are_told_apart_by_receiver_type_not_sql_text() {
    let tx = "package orders\n\nimport (\n\t\"context\"\n\t\"database/sql\"\n)\n\n\
        func (s *Store) Save(ctx context.Context, o Order) error {\n\
        \ttx, err := s.db.BeginTx(ctx, nil)\n\
        \tif err != nil {\n\t\treturn err\n\t}\n\
        \tdefer tx.Rollback()\n\
        \tif _, err := tx.ExecContext(ctx, \"INSERT INTO orders VALUES ($1)\", o.ID); err != nil {\n\t\treturn err\n\t}\n\
        \treturn tx.Commit()\n}\n";
    let raws = found(tx, &[]);
    let db = of(&raws, SyntaxKind::DbCall);
    assert_eq!(db.len(), 1);
    assert_eq!(db[0].subject.as_deref(), Some("*sql.Tx"));
    assert_eq!(db[0].symbol.as_deref(), Some("Store.Save"));
    assert_eq!(
        of(&raws, SyntaxKind::TxBoundary).len(),
        3,
        "begin, rollback, commit"
    );
    assert_eq!(of(&raws, SyntaxKind::Defer)[0].detail, "tx.Rollback()");
    assert!(!of(&raws, SyntaxKind::ContextParam).is_empty());
    assert!(!of(&raws, SyntaxKind::ReturnsError).is_empty());

    let db_only = tx.replace("tx.ExecContext", "s.db.ExecContext");
    let raws = found(&db_only, &[]);
    let db = of(&raws, SyntaxKind::DbCall);
    assert_eq!(db[0].subject.as_deref(), Some("*sql.DB"));
    assert!(db[0].detail.contains("INSERT") || db[0].detail.contains("ExecContext"));
}

#[test]
fn unknown_receiver_stays_unresolved() {
    let src = "package orders\n\nfunc save(r Repo) {\n\tr.conn().Exec(\"x\")\n}\n";
    let raws = found(src, &[]);
    let db = of(&raws, SyntaxKind::DbCall);
    assert_eq!(db[0].subject, None);
    assert!(db[0].detail.contains("unresolved"));
}

#[test]
fn goroutine_channels_guards_and_registered_wrapper() {
    let wrappers = [(
        "AppendEvent".to_owned(),
        "example.internal/orders.Store.AppendEvent".to_owned(),
    )];
    let src = "package orders\n\nfunc (s *Store) Create(tx *sql.Tx, done chan bool) {\n\
        \tif s.db != nil {\n\t\ts.AppendEvent(tx, \"created\")\n\t}\n\
        \tgo func() {\n\t\ts.AppendEvent(nil, \"created\")\n\t\tdone <- true\n\t}()\n\
        \t<-done\n\tclose(done)\n}\n";
    let raws = found(src, &wrappers);
    let calls = of(&raws, SyntaxKind::RegisteredEffectCall);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].subject.as_deref(), Some("*sql.Tx"));
    assert_eq!(calls[0].guard.as_deref(), Some("s.db != nil"));
    assert_eq!(calls[1].subject, None);
    let spawn = of(&raws, SyntaxKind::DetachedSpawn);
    assert_eq!((spawn[0].start, spawn[0].end), (7, 10));
    assert_eq!(of(&raws, SyntaxKind::ChannelOp).len(), 3);
}

#[test]
fn interface_dispatch_reflection_and_build_tags() {
    let src = "//go:build linux && !race\n\npackage orders\n\nimport \"reflect\"\n\n\
        func (s *Store) Notify(e Event) {\n\t_ = s.events.Publish(e)\n\treflect.ValueOf(e).MethodByName(\"Run\")\n}\n";
    let raws = found(src, &[]);
    assert_eq!(
        of(&raws, SyntaxKind::CfgGate)[0].detail,
        "//go:build linux && !race"
    );
    let interface = of(&raws, SyntaxKind::InterfaceCall);
    assert!(interface[0].detail.contains("interface Publisher"));
    assert_eq!(
        of(&raws, SyntaxKind::DynamicDispatch)[0].detail,
        "reflect.ValueOf"
    );
    assert_eq!(of(&raws, SyntaxKind::DiscardedResult).len(), 1);
    assert_eq!(of(&raws, SyntaxKind::Import)[0].detail, "reflect");
}

#[test]
fn syntax_error_is_refused() {
    assert!(facts("package x\nfunc (", &PackageTypes::default(), &[]).is_err());
}
