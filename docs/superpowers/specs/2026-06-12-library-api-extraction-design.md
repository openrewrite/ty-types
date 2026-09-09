# Design: `getLibraryApi` — public-API type extraction for Python libraries

Date: 2026-06-12
Status: Approved (pending spec review)

## Motivation

Moderne CLI builds type tables for third-party Java libraries by scanning a
JAR's class files with ASM, constructing `JavaType` objects, and serializing
them to a custom `types/*.bin` file (the V3 type-table format in
`moderne-cli/core/serialization/.../v3/type`). The parser then reads those
pre-built types instead of reconstructing them from the compiler symbol table.

We want the equivalent for Python third-party libraries: point a tool at an
installed package (in `site-packages`) and get back the types for its public
API surface — module-level declarations including class members — so the same
`types.bin` machinery can be fed for Python distributions.

ty already does the hard part (Python type inference), and ty-types already
exposes ty's structured types as JSON over JSON-RPC. This design adds a new
RPC method that extracts a library's public API as JSON.

## Scope and division of labor

The `.bin` V3 type-table format contains a minimal-perfect-hash index
(CHD / RecSplit, via a vendored copy of Thomas Mueller's `minperf`),
ZSTD-compressed entry-aligned body blocks, and a graph of OpenRewrite
`JavaType` instances. Reproducing that format byte-compatibly from Rust would
mean reimplementing the `minperf` MPH bit layout, the ZSTD block framing, and a
Python-`Type`→`JavaType` mapping, against an internal format that actively
churns. That is the single hardest piece of interop available here and buys
nothing that moderne-cli (which already owns a tested `TypeTableWriter`) cannot
do.

Therefore the work is split at the natural seam:

- **ty-types (Rust, this task):** infer Python types and emit the public API as
  JSON. This is the part only ty can do.
- **moderne-cli (Java, separate task):** consume that JSON, map descriptors to
  `JavaType`, and feed the existing `TypeTableWriter` to produce `types.bin`.
  CHD, ZSTD, format evolution — all reused, zero reimplementation.
- **Read side (later, separate):** a Python (or Rust) `types.bin` *deserializer*
  is a separate, later concern. Deserialization is easier than writing — it
  only evaluates the MPH (or linear-scans), never builds it. Out of scope here.

This document covers only the ty-types JSON extractor.

## Confirmed enabling APIs (ty fork `ty-types-2`)

All reachable in the pinned fork; no additional `pub(crate)→pub` patch needed:

- `ruff_db::files::system_path_to_file(db, path) -> Result<File, _>` — already
  used in `src/main.rs`.
- `ty_module_resolver::resolve::file_to_module(db, file) -> Option<Module>`
  (`pub`) — for the dotted module FQN.
- `ty_python_core::global_scope(db, file) -> ScopeId` (`pub`).
- `ty_python_semantic::types::list_members::all_end_of_scope_members(db, scope)`
  (`pub`) — the same call `registry.rs` already uses for class bodies; works on
  a module's global scope to enumerate module-level symbols and their types.
- `ty_python_semantic::dunder_all::dunder_all_names(db, file) -> Option<FxHashSet<Name>>`
  (`pub`) — `__all__` contents when present.

Note: a `ModuleLiteral`'s `available_submodule_attributes` is **import-driven**
(reads `semantic_index(...).imported_modules()`), so `all_members` on a module
only surfaces submodules the package's `__init__` imports. It would miss public
submodules not re-exported by `__init__`. Module discovery therefore uses a
filesystem walk of the package dir, not `all_members`.

## Data flow

1. `initialize(projectRoot)` sets up the `ProjectDatabase` as today. The
   `projectRoot` must be a venv/project where the target package **and its
   dependencies** resolve, so ty's environment + typeshed are available.
2. `getLibraryApi({ root })` takes an absolute path to an installed package
   directory and returns `modules → public symbols → typeId`, plus the shared
   type registry (the same `TypeId`/`TypeDescriptor` registry as `getTypes`).

## Components

### 1. Module discovery — filesystem walk (`src/library.rs`)

Walk `root` recursively for `.py`/`.pyi` files.

- **Stub preference:** when both `foo.pyi` and `foo.py` exist for the same
  module, use `foo.pyi` and ignore the `.py`. This matches ty's own stub
  preference and is the behavior a user expects.
- **Private-module filter:** skip any file whose path relative to `root` has an
  **underscore-prefixed path component** (e.g. `requests/_internal/x.py`).
  `__init__.py` / `__main__.py` are dunder, not private, and are kept.
- Each surviving file → `system_path_to_file` → `file_to_module` for its dotted
  module FQN.

### 2. Symbol enumeration + public filter

Per surviving module file:

- `global_scope(db, file)` → `all_end_of_scope_members(db, scope)` to enumerate
  module-level symbols and their inferred types.
- **Public filter:** if `dunder_all_names(db, file)` is `Some(names)`, keep only
  symbols in `names`; otherwise drop underscore-prefixed symbol names.
- Register each surviving symbol's type → `typeId`, recorded under the module.

Module-level functions, variables, and constants are part of the public surface
and are kept (subject to the filter above). Whether their *types* expand fully
or extern as a `classRef` is decided entirely by the boundary predicate below.

### 3. Defined-vs-referenced boundary (the one new registry behavior)

Add an optional **boundary predicate** to `TypeRegistry`:

- `None` (default) → today's expand-everything behavior. `getTypes` /
  `getTypeRegistry` are completely unchanged.
- `Some(under_root)` → before expanding a `ClassLiteral`'s members, resolve its
  definition file (`class.body_scope(db)` → scope's file → `file.path(db)`) and
  test whether that path is **under `root`**:
  - **Under `root`** → expand fully (the `TAG_CLASS` equivalent: full
    `classLiteral` with members).
  - **Outside `root`** (stdlib, numpy, another distribution) → emit a new
    lightweight **`classRef`** descriptor with no member expansion and no
    recursion. This mirrors `TAG_CLASS_REF` exactly, so moderne-cli maps it 1:1.

Instances of external classes still get their normal `instance` descriptor; only
the backing class literal is cut to a ref. The boundary is **class-only** —
non-`FullyQualified` types (methods, variables, generics, arrays) always get
full bodies, matching the V3 format where only fully-qualified types extern as a
ref.

### 4. New `TypeDescriptor` variant

```
classRef { className, moduleName, display? }
```

A reference to a class defined outside `root`. Carries identity only — no
members, no supertypes, no type parameters. Distinct from `classLiteral`
(defined here, full body) so the consumer maps it directly to `TAG_CLASS_REF`.

### 5. Output JSON

```jsonc
{
  "modules": [
    {
      "name": "requests.sessions",
      "file": "requests/sessions.py",          // path relative to root (or .pyi)
      "symbols": [
        { "name": "Session", "typeId": 12 },
        { "name": "get", "typeId": 30 }
      ]
    }
  ],
  "types": {                                     // the shared TypeId registry
    "12": { "kind": "classLiteral", /* members → typeIds */ },
    "…": {}
  }
}
```

Distribution coordinates (ecosystem / package id / version) are **not**
ty-types' responsibility — the caller knows them from the path / `.dist-info`
and owns them Java-side.

## Files

- `src/protocol.rs` — `getLibraryApi` request/response types; `classRef`
  `TypeDescriptor` variant.
- `src/registry.rs` — optional boundary predicate; `classRef` emission for
  external class literals.
- `src/library.rs` (new) — filesystem walk, `.pyi` preference, private-module
  filter, public-symbol filter; drives the registry to produce the module tree.
- `src/main.rs` — dispatch the new method in the JSON-RPC loop.
- `CLAUDE.md` — document the new method and the `classRef` variant.

## Testing (TDD)

A fixture package under `tests/` exercising every decision:

- nested submodules (depth > 1),
- an `_internal` (underscore-prefixed) private submodule that must be excluded,
- one module that defines `__all__` (assert only listed names appear),
- one module without `__all__` and an underscore-prefixed symbol (assert it is
  dropped),
- a class with methods and attributes (assert full `classLiteral` with member
  typeIds),
- a module-level function/variable whose type is a class defined **in** the
  package (assert full descriptor) and another whose type references a
  stdlib/external class (assert `classRef`),
- a `.pyi`/`.py` pair for the same module (assert the `.pyi` wins).

Tests:

- Rust unit tests for module discovery (walk, stub preference, private filter)
  and the public-symbol filter.
- A JSON-RPC smoke test over the stdio loop asserting the module tree,
  exclusion of private/underscore modules and symbols, and that the external
  class came back as `classRef`.

Per project convention, write the failing tests first, then implement to green.
```
