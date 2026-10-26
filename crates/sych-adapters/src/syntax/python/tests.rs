//! Shapes `facts` finds in Python sources.

use super::*;

fn found(source: &str) -> Vec<Raw> {
    facts(
        source,
        &[("publish_order".into(), "orders.outbox.publish_order".into())],
    )
    .expect("parses")
}

fn of(raws: &[Raw], kind: SyntaxKind) -> Vec<&Raw> {
    raws.iter().filter(|r| r.kind == kind).collect()
}

#[test]
fn write_inside_with_and_after_it_have_different_scopes() {
    let inside = "import json\n\nclass Exporter:\n    def save(self, path, data):\n        with open(path, \"w\") as f:\n            f.write(json.dumps(data))\n";
    let raws = found(inside);
    let scope = of(&raws, SyntaxKind::ResourceScope);
    assert_eq!((scope[0].start, scope[0].end), (5, 6));
    let writes = of(&raws, SyntaxKind::WriteCall);
    assert_eq!(writes.len(), 2, "open for write and f.write");
    assert!(
        writes
            .iter()
            .all(|w| w.symbol.as_deref() == Some("Exporter.save"))
    );
    assert_eq!(of(&raws, SyntaxKind::Import)[0].detail, "json");

    let moved = "import json\n\nclass Exporter:\n    def save(self, path, data):\n        f = open(path, \"w\")\n        f.write(json.dumps(data))\n";
    let raws = found(moved);
    assert!(of(&raws, SyntaxKind::ResourceScope).is_empty());
    assert_eq!(of(&raws, SyntaxKind::WriteCall)[1].start, 6);
}

#[test]
fn finally_tasks_transactions_and_swallowed_errors() {
    let src = "import asyncio\n\nasync def run(session, conn):\n    try:\n        session.add(order)\n        session.commit()\n    except ValueError:\n        pass\n    finally:\n        conn.close()\n    asyncio.create_task(notify())\n    task = asyncio.create_task(notify())\n    if order.paid:\n        publish_order(order)\n";
    let raws = found(src);
    assert_eq!(
        of(&raws, SyntaxKind::ResourceScope)[0].detail,
        "try/finally"
    );
    assert_eq!(of(&raws, SyntaxKind::DbCall).len(), 1);
    assert_eq!(of(&raws, SyntaxKind::TxBoundary).len(), 1);
    assert_eq!(of(&raws, SyntaxKind::SwallowedException).len(), 1);
    assert_eq!(of(&raws, SyntaxKind::DetachedSpawn)[0].start, 11);
    assert_eq!(of(&raws, SyntaxKind::AsyncTask)[0].start, 12);
    let wrapper = of(&raws, SyntaxKind::RegisteredEffectCall);
    assert_eq!(wrapper[0].detail, "orders.outbox.publish_order");
    assert_eq!(wrapper[0].guard.as_deref(), Some("order.paid"));
}

#[test]
fn dynamic_shapes_are_named() {
    let src = "import importlib\nimport requests\nfrom app import *\n\n@app.route(\"/x\")\ndef view():\n    handler = getattr(module, name)\n    plugin = importlib.import_module(name)\n    requests.post(url, json=body)\n\nrequests.get = fake_get\nsetattr(requests, \"post\", fake)\nsignals.connect(view)\n\n@property\ndef plain(self):\n    pass\n";
    let raws = found(src);
    assert_eq!(of(&raws, SyntaxKind::DynamicDispatch).len(), 2);
    assert_eq!(of(&raws, SyntaxKind::MonkeyPatch).len(), 2);
    let registered = of(&raws, SyntaxKind::RuntimeRegistration);
    assert_eq!(
        registered.len(),
        2,
        "route decorator and connect, not property"
    );
    assert_eq!(of(&raws, SyntaxKind::NetworkCall).len(), 1);
    assert!(
        of(&raws, SyntaxKind::Import)
            .iter()
            .any(|i| i.detail == "app (*)")
    );
}

#[test]
fn syntax_error_is_refused() {
    assert!(facts("def f(:\n", &[]).is_err());
}
