# Architecture review

Besides what a model reads in a change, Sych checks the structure of the code itself: which modules, types and
functions know which, computed from the names a compiler-grade index resolves, never from text search or a
hand-written map. A change that ties two modules into a circle, leaks a type's field or widens a module's interface
gets a finding even with `--models fake`; a model sees the same facts in its packets and may explain them, but cannot
change what the checks computed.

The model, the graph and the six checks are ported from
[deslop-please](https://github.com/deterministic-machines/deslop-please) (commit `f678d10`, MIT; the notice is in
`crates/sych-core/src/architecture/UPSTREAM-LICENSE`). Upstream reads Rust; here every language below builds the same
model. easement stays as it is: it checks the constraints an `easement.yml` declares, while architecture review checks
the shape of the code without any.

## What is checked

| Check | Reports |
|---|---|
| `module-cycle` | sibling modules that use each other in a circle, so there is no order to read them in |
| `tangle` | things that know each other in a circle across more than one module, or more of them than `tangle_members` in one |
| `field-leak` | code outside a type that writes, borrows, indexes or calls a method on a field the type otherwise keeps to itself and works on, directly or through a getter or setter |
| `shallow-module` | a package or a module with children that shows more than `shallow_functions` working functions, hides at most as many, and has a list (of things, or of one thing's functions) past the same bound |
| `wide-directory` | a directory with more than `wide_directory` files and directories of code |
| `long-file` | a file of more than `long_file` lines, blank and comment lines too |

The bounds and `enabled` are [`architecture` in `.sych/config.yaml`](CONFIG.md#architecture-bounds), read from the
target tip, so a change cannot loosen the bounds its own code is judged by.

Upstream's exceptions hold. Where an item is declared and what owns it differ: a method belongs to its type wherever
its `impl` block is, so moving one changes nothing. A wrapper that adds nothing (a forwarder, a getter, a setter) is
seen through: a use of it is a use of what it hands on, and it does not deepen a module. Reading a value is no leak, a
marker field holds nothing, and a type that keeps none of its fields, or does no work of its own on them, guards
nothing. The semantic checks leave test code out; the layout checks count it.

## How the model is built

For each project a change touches (the outermost Cargo workspace or nearest `Cargo.toml`, the nearest `go.mod`,
`tsconfig.json` or `package.json`, the nearest Python project file or the repository's root; at most
`tooling.indexers.max_projects`, those in `review.fixture_paths` left out), Sych writes the diff base and head from pinned Git objects, never from a working
tree, and runs the pinned SCIP indexer on the whole project on each side: rust-analyzer, scip-go, scip-typescript,
scip-python. The syntax of each indexed file (syn, tree-sitter, oxc) adds what an index does not say: where an
item's code is, whether a function takes a receiver or implements a trait or protocol, which functions only wrap
others, what a field's type holds and what the code does with each member it names.

| Language | Packages and modules | What belongs to a type |
|---|---|---|
| Rust | crates and their targets (test and bench targets are test code), `mod` | its fields, variants and every `impl` item, trait items included |
| Go | the module and its packages, by directory | its fields and receiver methods |
| TypeScript, JavaScript | the package, its directories and files (`index` is its directory) | a class's properties, methods and accessors |
| Python | the project's package and its dotted modules | a class's attributes, `self.x` fields and methods |

Upstream's rules are Rust's; the others are adaptations, not a bit-for-bit match. In Go, TypeScript and Python a value
is a shared reference, so any use of a field as a whole borrows it, and only a field whose declared type (or literal
value) is a number, boolean or string reads as a copy; a call on it reads only when it holds a number, boolean or
string. A Python special method other than a constructor counts as a protocol implementation, as a Rust trait method
does. Go forbids a cycle of packages, so a Go tangle is the ring of a single package.

No repository code runs: the indexers get a `PATH` of the pinned indexers alone and an empty `HOME`, cargo cannot
compile, so no build script or proc-macro runs, scip-python gets an empty environment and never starts `python` or
`pip`, and no package script runs. The server's `tooling.indexers` bounds each run
([CONFIG.md](CONFIG.md)): a run past `timeout_seconds` (600), or whose processes together hold more than `memory_mb`
(4096) resident, is killed; an index past `max_index_mb` (512), or a project past `max_files` (5000) source files, is
not read. Such a project is `ARCHITECTURE_UNAVAILABLE`, never clean. Memory is measured from Linux's `/proc`; where
there is none, only the time bound holds, and the facts say so. An index is cached in the state directory by its inputs: the indexer and its version,
whether dependencies are installed, and every blob of the project.

## What is complete

What an index cannot see is a limitation, never taken as absence: unexpanded attribute and derive macros, `reflect`,
`getattr`/`setattr`/`__dict__`, untyped JavaScript and `any`, files that do not parse, and, in TypeScript and Python,
member names the index leaves unresolved though the project has a field or function so named. Each check says what
it proves: a circle or a leak found stands, but with such gaps the four semantic checks are `PARTIAL`, since the
absence of another is not proven. The layout checks read the files themselves and are complete.

## Against the diff base

Each finding is known by its check and its key (the members of a circle, a field's or a module's name, a path), never
by a line. Against the diff base it is `NEW`, `WORSE` (larger: more members, places, functions shown, files or
lines), `SAME` (existing debt) or `BETTER`; a circle that loses or gains members is the same circle, and a renamed file
keeps its findings. The base's findings head no longer has are `fixed`. Without a checked base nothing is new or old:
the run says so as a gap, `ARCHITECTURE_BASE_UNCHECKED`, as it does `ARCHITECTURE_UNAVAILABLE` for a project whose
checks did not run.

## In the result

A finding the change introduces or makes worse, and, where the base was not checked, one in a file it touches, is a
result finding: `IMPROVEMENT` at `LOW` with the computed message, what the change did, the notes and the picture of
the circle, the spread or the bars. It is anchored where the check places it, also in a file the change does not
touch, where it appears in the summary rather than inline. Existing debt and fixed findings stay in the facts.

A required repository rule `ARCH-<CHECK>` (`ARCH-MODULE-CYCLE`, `ARCH-TANGLE`, `ARCH-FIELD-LEAK`,
`ARCH-SHALLOW-MODULE`, `ARCH-WIDE-DIRECTORY`, `ARCH-LONG-FILE`) makes such a finding a `RULE_VIOLATION` at the rule's
`severity_on_violation`. An exception of that rule in force over the finding's path waives it as an exception waives
an obligation: the finding is kept with the waiver in its limitations, as an improvement that does not raise the
recommendation.

The JSON report holds every project's `architecture_facts`: status, whether the base was checked, the limits, the
coverage of each check, each finding with its change and base size, and the fixed ones. The Markdown report adds an
`Architecture` section with the same, and draws each finding's picture as a code block.
