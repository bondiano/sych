//! Syntax facts (spec §5.4, §10.6): observations from parsing changed files, never findings.
//!
//! Patterns are matched on the syntax tree without expanding macros or resolving types, so
//! a fact says "this shape is here", and its absence proves nothing.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LintFacts;
use crate::effects::EffectLedger;
use crate::semantic::facts_evidence_id;
use crate::{
    BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    Language, Producer, SchemaVersion, SourceSide, SourceSnapshot, SourceSnapshotId, Timestamp,
    TrustClass, Validity, canonical_sha256,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxKind {
    // Rust
    AsyncFn,
    /// A `lock()`/`read()`/`write()`/`borrow_mut()` guard still bound at an `.await`.
    GuardAcrossAwait,
    SelectMacro,
    /// `spawn(..)` whose handle is dropped.
    DetachedSpawn,
    /// `std::thread::sleep`, `std::fs::*` or `block_on` inside an `async fn`.
    BlockingInAsync,
    /// `let _ = call(..)` or a statement ending in `.ok()`.
    DiscardedResult,
    Unwrap,
    DropImpl,
    PublicItem,
    CfgGate,
    /// An item or statement under a `#[cfg(..)]` that the analysed build (host target, `test`,
    /// `debug_assertions`, default features) does not compile: its code is not checked.
    InactiveCfg,
    SerdeAttribute,
    UnexpandedMacro,
    /// A value logged with `?value`, `{:?}` or `{value:?}`; `related` holds its type's
    /// definitions in the snapshot, empty when the type is defined elsewhere or not known.
    DebugFormatted,
    TestModule,
    /// `#[allow(..)]`/`#[expect(..)]`: a disabled lint is not a fix.
    LintSuppressed,
    /// `#[ignore]`, `it.skip`, `test.only`, `describe.todo`: a test that does not run.
    TestSkipped,
    /// A Rust `use` or TS `import`: names the types and wrappers the file depends on.
    Import,
    // TypeScript / React
    EffectWithCleanup,
    EffectWithoutCleanup,
    /// `addEventListener` without `removeEventListener` in the same effect.
    UnpairedListener,
    /// `setInterval`/`setTimeout` without the matching clear in the same effect.
    UnpairedTimer,
    /// `subscribe(..)` without `unsubscribe` or a returned cleanup in the same effect.
    UnpairedSubscription,
    /// Async work in an effect without an abort signal or a stale-response flag.
    AsyncEffectUnguarded,
    AsyncEffectGuarded,
    MutationCall,
    InvalidationCall,
    ControlledInput,
    UncontrolledInput,
    /// `value` without `onChange` or `readOnly`.
    ControlledWithoutHandler,
    FormSubmit,
    /// `useState` flags named loading/pending/error/optimistic/submitting, or `useOptimistic`.
    AsyncStateFlag,
    CustomHookCall,
    // Go / Python
    /// A function taking a `context.Context`.
    ContextParam,
    /// A function returning an `error`.
    ReturnsError,
    /// `defer` of a call; `detail` is the call.
    Defer,
    /// A channel send, receive, `close` or `select`.
    ChannelOp,
    /// `Begin`/`BeginTx`/`Commit`/`Rollback`, Python `commit`/`rollback`/`begin`.
    TxBoundary,
    /// A SQL call; `subject` is the receiver's declared type (`*sql.Tx`, `*sql.DB`, a project
    /// type) when the function or package shows it, else none.
    DbCall,
    /// A file write: Python `open(.., "w")`, `.write(..)`, Go `os.WriteFile`.
    WriteCall,
    /// An outgoing HTTP request.
    NetworkCall,
    /// A call of a wrapper listed in the effect registry; `detail` is the registry symbol and
    /// `subject` the type of a transaction passed to it, when known.
    RegisteredEffectCall,
    /// Python `with`/`async with` or `try`/`finally`: the span owns a resource or cleanup.
    ResourceScope,
    /// An async task kept by the caller: Python `create_task`/`ensure_future` assigned.
    AsyncTask,
    /// An `except` that only passes, continues or returns.
    SwallowedException,
    /// Reflection, `getattr`/`setattr`/`eval`/`import_module`: the callee is not known
    /// statically.
    DynamicDispatch,
    /// A Go call through an interface the package declares; `subject` is the interface, and
    /// the implementation is chosen at runtime.
    InterfaceCall,
    /// A module attribute replaced at runtime (`mod.attr = ..`, `setattr(mod, ..)`).
    MonkeyPatch,
    /// A decorator or `register`/`connect` call that registers code at import time.
    RuntimeRegistration,
}

impl SyntaxKind {
    /// Shapes that can refute "behaviour is preserved"; the broker ranks them as refuting.
    pub fn is_risk(self) -> bool {
        matches!(
            self,
            Self::GuardAcrossAwait
                | Self::DetachedSpawn
                | Self::BlockingInAsync
                | Self::DiscardedResult
                | Self::EffectWithoutCleanup
                | Self::UnpairedListener
                | Self::UnpairedTimer
                | Self::UnpairedSubscription
                | Self::AsyncEffectUnguarded
                | Self::ControlledWithoutHandler
                | Self::SwallowedException
                | Self::LintSuppressed
                | Self::TestSkipped
        )
    }

    /// Shapes whose meaning the changed text alone does not show: the broker names them on
    /// the span so the model reads the contract they carry.
    pub fn is_contract(self) -> bool {
        matches!(
            self,
            Self::SelectMacro
                | Self::SerdeAttribute
                | Self::CfgGate
                | Self::UnexpandedMacro
                | Self::TxBoundary
                | Self::ResourceScope
                | Self::RegisteredEffectCall
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyntaxFact {
    pub kind: SyntaxKind,
    pub code_ref: CodeRef,
    /// Enclosing function or component, when known.
    pub symbol: Option<String>,
    /// Short repository-derived description; data, not an instruction.
    pub detail: String,
    /// Whether the span intersects a changed line of this side.
    pub in_change: bool,
    /// Where a project wrapper (custom hook) is defined in the snapshot.
    #[serde(default)]
    pub related: Vec<CodeRef>,
    /// The type or name the fact is about beyond its span, e.g. a SQL call's receiver type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Condition of the nearest enclosing `if` inside the function, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyntaxFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    pub side: SourceSide,
    pub language: Language,
    pub producer: Producer,
    /// Parsed files with their parse status (`ok` or the error).
    pub files: BTreeMap<String, String>,
    pub facts: Vec<SyntaxFact>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// Semantic and syntax facts of a run as one stored, versioned document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunFacts {
    pub schema_version: SchemaVersion,
    pub semantic: Vec<crate::SemanticFacts>,
    pub syntax: Vec<SyntaxFacts>,
    pub ci: Vec<crate::CiFacts>,
    /// Effects of the changed code on both sides and the registry bound to its implementation.
    #[serde(default)]
    pub ledger: EffectLedger,
    /// Pinned linters run on the head's changed files.
    #[serde(default)]
    pub lint: Vec<LintFacts>,
}

impl SyntaxFacts {
    pub fn evidence_id(&self) -> EvidenceId {
        facts_evidence_id("syntax", &self.source_snapshot_id, self.side, self.language)
    }

    /// Derived static evidence; complete only for the parsed files and the listed shapes.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        let parsed_all = self.files.values().all(|status| status == "ok");
        Ok(Evidence {
            schema_version: SchemaVersion,
            id: self.evidence_id(),
            kind: EvidenceKind::Static,
            source_snapshot_id: self.source_snapshot_id.clone(),
            producer: self.producer.clone(),
            binding: EvidenceBinding {
                kind: BindingKind::Source,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: None,
            },
            validity: if parsed_all {
                Validity::Valid
            } else {
                Validity::Malformed
            },
            completeness: Completeness::Partial,
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self.facts.iter().map(|f| f.code_ref.clone()).collect(),
            limitations: self.limitations.clone(),
            observed_at,
        })
    }
}
