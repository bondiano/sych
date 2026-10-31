//! TypeScript/React shapes from `oxc`: effect setup and cleanup, listeners,
//! timers, subscriptions, stale async responses, mutations and invalidation, controlled
//! inputs, form submit and async state flags. Names come from source text by span, so the
//! shapes hold across `React.useEffect` and `useEffect` alike.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, AwaitExpression, CallExpression, Expression, Function,
    ImportDeclaration, JSXAttributeItem, JSXOpeningElement, NewExpression, ReturnStatement,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;
use sych_core::SyntaxKind;

use super::Raw;

const BUILTIN_HOOKS: &[&str] = &[
    "useState",
    "useEffect",
    "useLayoutEffect",
    "useInsertionEffect",
    "useMemo",
    "useCallback",
    "useRef",
    "useContext",
    "useReducer",
    "useId",
    "useTransition",
    "useDeferredValue",
    "useSyncExternalStore",
    "useImperativeHandle",
    "useDebugValue",
    "useOptimistic",
    "useActionState",
    "useFormStatus",
];
const STALE_GUARDS: &[&str] = &[
    "cancelled",
    "canceled",
    "ignore",
    "isMounted",
    "mounted",
    "active",
    "stale",
    "aborted",
];
const STATE_FLAGS: &[&str] = &["loading", "pending", "error", "optimistic", "submitting"];

/// What one effect callback does, counted at any depth inside it.
#[derive(Default)]
struct EffectScan<'s> {
    src: &'s str,
    depth: u32,
    cleanup: bool,
    calls: Vec<String>,
    awaits: u32,
    abort: bool,
}

fn name_of(src: &str, span: Span) -> String {
    let text = &src[span.start as usize..span.end as usize];
    text.rsplit(['.', '?'])
        .next()
        .unwrap_or(text)
        .trim()
        .to_owned()
}

impl<'a> Visit<'a> for EffectScan<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.calls.push(name_of(self.src, it.callee.span()));
        walk::walk_call_expression(self, it);
    }
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        if name_of(self.src, it.callee.span()) == "AbortController" {
            self.abort = true;
        }

        walk::walk_new_expression(self, it);
    }
    fn visit_await_expression(&mut self, it: &AwaitExpression<'a>) {
        self.awaits += 1;
        walk::walk_await_expression(self, it);
    }
    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        // Only the effect callback's own `return` (depth 1) is its cleanup.
        if self.depth == 1
            && matches!(
                it.argument,
                Some(
                    Expression::ArrowFunctionExpression(_)
                        | Expression::FunctionExpression(_)
                        | Expression::Identifier(_)
                        | Expression::CallExpression(_)
                )
            )
        {
            self.cleanup = true;
        }

        walk::walk_return_statement(self, it);
    }
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.depth += 1;
        // `useEffect(() => store.subscribe(f))` returns its cleanup from an expression body.
        if self.depth == 1 && it.expression {
            let body = &self.src[it.body.span.start as usize..it.body.span.end as usize];
            if body.contains("ubscribe") || body.contains("isten") {
                self.cleanup = true;
            }
        }

        walk::walk_arrow_function_expression(self, it);
        self.depth -= 1;
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        self.depth += 1;
        walk::walk_function(self, it, flags);
        self.depth -= 1;
    }
}

struct Visitor<'s> {
    src: &'s str,
    /// `(call name, registry symbol)` of registered effect wrappers.
    wrappers: &'s [(String, String)],
    out: Vec<Raw>,
    lines: Vec<usize>,
    symbols: Vec<String>,
}

impl Visitor<'_> {
    fn line(&self, offset: u32) -> u32 {
        let at = self
            .lines
            .partition_point(|&start| start <= offset as usize);
        u32::try_from(at).unwrap_or(u32::MAX)
    }

    fn push(&mut self, kind: SyntaxKind, span: Span, detail: String) {
        self.out.push(Raw {
            kind,
            start: self.line(span.start),
            end: self.line(span.end.saturating_sub(1).max(span.start)),
            symbol: self.symbols.last().cloned(),
            detail,
            subject: None,
            guard: None,
        });
    }

    fn effect(&mut self, it: &CallExpression<'_>, hook: &str) {
        let Some(callback) = it.arguments.first().and_then(|a| a.as_expression()) else {
            return;
        };
        let mut scan = EffectScan {
            src: self.src,
            ..EffectScan::default()
        };
        scan.visit_expression(callback);
        let text = &self.src[callback.span().start as usize..callback.span().end as usize];
        let has = |name: &str| scan.calls.iter().any(|c| c == name);
        let kind = if scan.cleanup {
            SyntaxKind::EffectWithCleanup
        } else {
            SyntaxKind::EffectWithoutCleanup
        };
        self.push(
            kind,
            it.span,
            format!("{hook} with {} calls", scan.calls.len()),
        );
        if has("addEventListener") && !has("removeEventListener") {
            self.push(
                SyntaxKind::UnpairedListener,
                it.span,
                format!("{hook}: addEventListener without removeEventListener"),
            );
        }

        for (start, clear) in [
            ("setInterval", "clearInterval"),
            ("setTimeout", "clearTimeout"),
        ] {
            if has(start) && !has(clear) {
                self.push(
                    SyntaxKind::UnpairedTimer,
                    it.span,
                    format!("{hook}: {start} without {clear}"),
                );
            }
        }
        if has("subscribe") && !has("unsubscribe") && !scan.cleanup {
            self.push(
                SyntaxKind::UnpairedSubscription,
                it.span,
                format!("{hook}: subscribe without unsubscribe or cleanup"),
            );
        }

        let is_async = scan.awaits > 0 || has("then") || has("fetch");
        if is_async {
            let flag = STALE_GUARDS.iter().any(|g| text.contains(g));
            let guarded = scan.abort || text.contains("signal") || flag;
            let sets_state = scan.calls.iter().any(|c| {
                c.len() > 3 && c.starts_with("set") && c[3..].starts_with(char::is_uppercase)
            });
            let detail = format!(
                "{hook}: async work{}{}",
                if sets_state { ", sets state" } else { "" },
                if scan.abort {
                    ", AbortController"
                } else if flag {
                    ", stale flag"
                } else {
                    ""
                }
            );
            let kind = if guarded {
                SyntaxKind::AsyncEffectGuarded
            } else {
                SyntaxKind::AsyncEffectUnguarded
            };
            self.push(kind, it.span, detail);
        }
    }
}

impl<'a> Visit<'a> for Visitor<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        self.push(SyntaxKind::Import, it.span, it.source.value.to_string());
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let name = name_of(self.src, it.callee.span());
        let callee = &self.src[it.callee.span().start as usize..it.callee.span().end as usize];
        if matches!(name.as_str(), "skip" | "only" | "todo")
            && ["it.", "test.", "describe."]
                .iter()
                .any(|p| callee.starts_with(p))
        {
            self.push(SyntaxKind::TestSkipped, it.span, callee.to_owned());
        }

        match name.as_str() {
            "useEffect" | "useLayoutEffect" | "useInsertionEffect" => self.effect(it, &name),
            "useOptimistic" => {
                self.push(SyntaxKind::AsyncStateFlag, it.span, "useOptimistic".into());
            }
            "useMutation" | "mutate" | "mutateAsync" => {
                self.push(SyntaxKind::MutationCall, it.span, name.clone());
            }
            "invalidateQueries" | "invalidate" | "refetch" | "revalidate" | "refetchQueries"
            | "setQueryData" => {
                self.push(SyntaxKind::InvalidationCall, it.span, name.clone());
            }
            n if n.len() > 3
                && n.starts_with("use")
                && n[3..].starts_with(char::is_uppercase)
                && !BUILTIN_HOOKS.contains(&n) =>
            {
                self.push(SyntaxKind::CustomHookCall, it.span, name.clone());
            }
            _ => {}
        }
        if let Some((_, symbol)) = self.wrappers.iter().find(|(call, _)| *call == name) {
            self.push(SyntaxKind::RegisteredEffectCall, it.span, symbol.clone());
        }

        walk::walk_call_expression(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        let id = &self.src[it.id.span().start as usize..it.id.span().end as usize];
        if let Some(init) = &it.init {
            let callee = match init {
                Expression::CallExpression(call) => Some(name_of(self.src, call.callee.span())),
                _ => None,
            };
            if callee.as_deref() == Some("useState") {
                let lower = id.to_lowercase();
                if STATE_FLAGS.iter().any(|f| lower.contains(f)) {
                    self.push(
                        SyntaxKind::AsyncStateFlag,
                        it.span,
                        format!("useState {id}"),
                    );
                }
            }

            if matches!(
                init,
                Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
            ) {
                self.symbols.push(id.to_owned());
                walk::walk_variable_declarator(self, it);
                self.symbols.pop();
                return;
            }
        }

        walk::walk_variable_declarator(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let name = it
            .id
            .as_ref()
            .map(|id| self.src[id.span.start as usize..id.span.end as usize].to_owned());
        if let Some(name) = &name {
            self.symbols.push(name.clone());
        }

        walk::walk_function(self, it, flags);
        if name.is_some() {
            self.symbols.pop();
        }
    }

    fn visit_jsx_opening_element(&mut self, it: &JSXOpeningElement<'a>) {
        let element = &self.src[it.name.span().start as usize..it.name.span().end as usize];
        let attrs: Vec<&str> = it
            .attributes
            .iter()
            .filter_map(|a| match a {
                JSXAttributeItem::Attribute(attr) => {
                    let s = attr.name.span();
                    Some(&self.src[s.start as usize..s.end as usize])
                }
                JSXAttributeItem::SpreadAttribute(_) => None,
            })
            .collect();
        let has = |n: &str| attrs.contains(&n);
        if matches!(element, "input" | "textarea" | "select") {
            if has("value") || has("checked") {
                self.push(
                    SyntaxKind::ControlledInput,
                    it.span,
                    format!("<{element}> controlled"),
                );
                if !has("onChange") && !has("readOnly") && !has("disabled") {
                    self.push(
                        SyntaxKind::ControlledWithoutHandler,
                        it.span,
                        format!("<{element}> value without onChange"),
                    );
                }
            }

            if has("defaultValue") || has("defaultChecked") {
                self.push(
                    SyntaxKind::UncontrolledInput,
                    it.span,
                    format!("<{element}> uncontrolled"),
                );
            }
        }

        if element == "form" && (has("onSubmit") || has("action")) {
            let handler = if has("onSubmit") {
                "onSubmit"
            } else {
                "action"
            };
            self.push(SyntaxKind::FormSubmit, it.span, format!("<form {handler}>"));
        }

        walk::walk_jsx_opening_element(self, it);
    }
}

pub fn facts(source: &str, path: &str, wrappers: &[(String, String)]) -> Result<Vec<Raw>, String> {
    let source_type = SourceType::from_path(Path::new(path)).map_err(|e| e.to_string())?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.panicked {
        return Err("parse failed".into());
    }

    let mut lines = vec![0];
    lines.extend(source.match_indices('\n').map(|(i, _)| i + 1));
    let mut visitor = Visitor {
        src: source,
        wrappers,
        out: Vec::new(),
        lines,
        symbols: Vec::new(),
    };
    visitor.visit_program(&parsed.program);
    if !parsed.diagnostics.is_empty() {
        return Err(format!(
            "{} syntax errors; facts may be incomplete",
            parsed.diagnostics.len()
        ));
    }

    Ok(visitor.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(SyntaxKind, u32)> {
        facts(source, "a.tsx", &[])
            .expect("parses")
            .into_iter()
            .map(|r| (r.kind, r.start))
            .collect()
    }

    #[test]
    fn imports_are_found_with_their_source() {
        let found = facts(
            "import { a } from \"./a\";\nimport type {\n  B,\n} from \"./b\";\n",
            "x.ts",
            &[],
        )
        .expect("parses");
        let imports: Vec<_> = found
            .iter()
            .filter(|r| r.kind == SyntaxKind::Import)
            .map(|r| (r.start, r.end, r.detail.as_str()))
            .collect();
        assert_eq!(imports, [(1, 1, "./a"), (2, 4, "./b")]);
    }

    #[test]
    fn effect_cleanup_and_unpaired_resources() {
        let leaky = "function A() {\n  useEffect(() => {\n    window.addEventListener('resize', f);\n    setInterval(tick, 1000);\n  }, []);\n  return null;\n}\n";
        let found = kinds(leaky);
        for k in [
            SyntaxKind::EffectWithoutCleanup,
            SyntaxKind::UnpairedListener,
            SyntaxKind::UnpairedTimer,
        ] {
            assert!(found.contains(&(k, 2)), "{k:?} in {found:?}");
        }
        let clean = "function A() {\n  React.useEffect(() => {\n    window.addEventListener('resize', f);\n    return () => window.removeEventListener('resize', f);\n  }, []);\n}\n";
        let found = kinds(clean);
        assert!(
            found.contains(&(SyntaxKind::EffectWithCleanup, 2)),
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|(k, _)| *k == SyntaxKind::UnpairedListener)
        );
    }

    #[test]
    fn stale_async_response_versus_guarded_fetch() {
        let stale = "function A({ id }) {\n  useEffect(() => {\n    fetch(`/o/${id}`).then(r => r.json()).then(setOrder);\n  }, [id]);\n}\n";
        assert!(kinds(stale).contains(&(SyntaxKind::AsyncEffectUnguarded, 2)));
        let guarded = "function A({ id }) {\n  useEffect(() => {\n    let ignore = false;\n    fetch(`/o/${id}`).then(r => r.json()).then(o => { if (!ignore) setOrder(o); });\n    return () => { ignore = true; };\n  }, [id]);\n}\n";
        let found = kinds(guarded);
        assert!(
            found.contains(&(SyntaxKind::AsyncEffectGuarded, 2)),
            "{found:?}"
        );
        assert!(found.contains(&(SyntaxKind::EffectWithCleanup, 2)));
    }

    #[test]
    fn inputs_forms_mutations_and_custom_hooks() {
        let source = "export const Form = () => {\n  const [isSaving, setIsSaving] = useState(false);\n  const save = useMutation({ onSuccess: () => client.invalidateQueries(['o']) });\n  const user = useCurrentUser();\n  return (<form onSubmit={submit}>\n    <input value={name} />\n    <input defaultValue=\"x\" />\n  </form>);\n};\n";
        let found = facts(source, "a.tsx", &[]).expect("parses");
        let has = |k: SyntaxKind, line: u32| found.iter().any(|r| r.kind == k && r.start == line);
        assert!(
            !has(SyntaxKind::AsyncStateFlag, 2),
            "isSaving is not a listed flag name"
        );
        assert!(has(SyntaxKind::MutationCall, 3) && has(SyntaxKind::InvalidationCall, 3));
        assert!(has(SyntaxKind::CustomHookCall, 4));
        assert!(has(SyntaxKind::FormSubmit, 5));
        assert!(
            has(SyntaxKind::ControlledInput, 6) && has(SyntaxKind::ControlledWithoutHandler, 6)
        );
        assert!(has(SyntaxKind::UncontrolledInput, 7));
        assert!(found.iter().all(|r| r.symbol.as_deref() == Some("Form")));
        let skipped = facts(
            "it.skip('saves', () => {});\ndescribe.only('x', () => {});\n",
            "a.test.ts",
            &[],
        )
        .expect("parses");
        assert_eq!(
            skipped
                .iter()
                .filter(|r| r.kind == SyntaxKind::TestSkipped)
                .count(),
            2
        );
        let flags = facts(
            "function A() { const [isLoading, setIsLoading] = useState(false); }",
            "a.ts",
            &[],
        )
        .expect("parses");
        assert!(flags.iter().any(|r| r.kind == SyntaxKind::AsyncStateFlag));
    }
}
