# CLAUDE.md

## Project Overview

**ty-types** is a Rust CLI tool that exposes ty's type inference via JSON-RPC over stdio. It uses ty (the Python type checker by Astral) as a library to provide structured type information for every AST node in a Python file.

## Architecture

- `src/main.rs` — JSON-RPC stdio loop with session lifecycle (initialize → getTypes* → shutdown)
- `src/protocol.rs` — Serde types for JSON-RPC requests/responses and TypeDescriptor enum
- `src/project.rs` — ProjectDatabase setup using OsSystem and ProjectMetadata::discover
- `src/registry.rs` — TypeRegistry: deduplicates Type<'db> → TypeId with structured descriptors
- `src/collector.rs` — SourceOrderVisitor that walks Python AST, gets types via HasType trait

The registry persists across getTypes requests within a session. This works because `run_session()` borrows `&ProjectDatabase` and creates `TypeRegistry<'db>` in the same scope, so the lifetime is naturally shared.

## Development Commands

```bash
cargo check                          # Type-check
cargo build                          # Build debug binary
cargo build --release                # Build release binary

# Smoke test
echo '{"jsonrpc":"2.0","method":"initialize","params":{"projectRoot":"/path/to/project"},"id":1}
{"jsonrpc":"2.0","method":"getTypes","params":{"file":"example.py"},"id":2}
{"jsonrpc":"2.0","method":"shutdown","id":99}' | cargo run
```

## Key Constraints

- The `ruff/` submodule is pinned to a specific commit on `openrewrite/ruff` `ty-types-2` branch, which widens `pub(crate)` → `pub` across `ty_python_semantic`. This gives us access to structured type internals (callable signatures, type var bounds, known instance classes, etc.).
- Update the submodule with `cd ruff && git fetch origin ty-types-2 && git checkout origin/ty-types-2`.
- Rust edition 2024, requires Rust 1.95+.

## Wire Protocol

JSON-RPC over stdin/stdout, one JSON object per line.

Methods: `initialize`, `getTypes`, `getTypeRegistry`, `getLibraryApi`, `getStdlibApi`, `shutdown`.

`getLibraryApi` extracts the public API of one installed distribution. Its `roots` param lists the paths that distribution installs — mypy ships `mypy/` and `mypyc/`, pytest ships `_pytest/`, `pytest/` and a bare `py.py` — each a package directory or a single module file. The boundary spans their union, so a class defined in one root and referenced from another stays a full `classLiteral`; classes outside every root become `classRef`. `root` names a single root and is unioned with `roots`. Two visibility filters are on by default and each has an opt-out: `includePrivateModules` keeps modules with an underscore-prefixed path component, and `includeNonExportedSymbols` keeps module-level symbols that `__all__` omits (or, with no `__all__`, underscore-prefixed ones). A root named explicitly is always extracted, whatever its name.

`getStdlibApi` extracts the standard library's public API for the project's configured Python version. Its `modules` param selects the local unit (top-level module names): classes in those modules are full `classLiteral`s, classes elsewhere become `classRef`. Omitting `modules` returns all stdlib modules fully expanded.

`initialize` accepts an optional first-party boundary that the session's `getTypes` registry honors: `firstPartyRoot` (a package root path) or `firstPartyModules` (top-level module names; used when `firstPartyRoot` is absent). When set, `getTypes` emits classes defined outside the boundary as `classRef` instead of fully expanding them; with neither field, every class is fully expanded (default behavior). `firstPartyRoot` takes precedence if both are given.

## TypeDescriptor Variants

Each type in the registry is represented as a `TypeDescriptor` with a `kind` discriminator:

| Kind | Description | Key Fields |
|------|-------------|------------|
| `instance` | Instance of a class (`str`, `int`, `MyClass()`); `tupleElements` is present for tuples and their subclasses | `className`, `moduleName`, `supertypes`, `typeArgs`, `classId`, `tupleElements` |
| `classLiteral` | Class object itself (`type[MyClass]`) | `className`, `moduleName`, `typeParameters`, `supertypes`, `members` |
| `subclassOf` | Subclass-of constraint. `base` is a `classLiteral` for a class or a protocol declared as one, an `instance` for a synthesized protocol, otherwise `dynamic` or `typeVar` | `base` |
| `classRef` | Reference to a class defined outside the extracted library boundary (identity only; maps to the type-table `TAG_CLASS_REF`) | `className`, `moduleName` |
| `typeForm` | `TypeForm[T]` value wrapping a type expression (PEP 747) | `typeArgument` |
| `union` | Union type (`X \| Y`) | `members` |
| `intersection` | Intersection type | `positive`, `negative` |
| `function` | Named function (`def foo(...)`) | `name`, `moduleName`, `typeParameters`, `parameters`, `returnType` |
| `callable` | Anonymous callable (`Callable[[int], str]`) | `parameters`, `returnType` |
| `boundMethod` | Bound method (`obj.method`) | `name`, `className`, `moduleName`, `typeParameters`, `parameters`, `returnType` |
| `wrapperDescriptor` | Descriptor wrapper (`__get__`, `__set__`) | `descriptorKind`, `parameters`, `returnType` |
| `knownInstance` | Well-known singleton instance (`TypeVar`, `typing.Callable`, `functools.partial(...)`, `range(...)`) | `className`, `knownInstanceKind`, `isNonEmpty`, `wrappedType`, `parameters`, `returnType` |
| `intLiteral` | Literal int | `value` |
| `boolLiteral` | Literal bool | `value` |
| `stringLiteral` | Literal string | `value` |
| `bytesLiteral` | Literal bytes | `value` |
| `enumLiteral` | Enum member | `className`, `memberName` |
| `literalString` | `LiteralString` type | — |
| `dynamic` | `Any`, `Unknown`, etc. | `dynamicKind` |
| `never` | Bottom type | — |
| `truthy` / `falsy` | Truthiness narrowing | — |
| `typeVar` | Type variable in scope; `typevarKind` is one of `TypeVar`, `ParamSpec`, `TypeVarTuple`, `Self`, `TypeAlias` | `name`, `typevarKind`, `bound`, `constraints`, `defaultType` |
| `module` | Module literal | `moduleName` |
| `typeAlias` | Type alias (PEP 695 or legacy) | `name`, `qualifiedName`, `valueType`, `typeParameters` |
| `typedDict` | TypedDict | `name`, `fields`, `closed`, `extraItems` |
| `typeIs` / `typeGuard` | Type narrowing returns | `narrowedType` / `guardedType` |
| `newType` | NewType wrapper | `name`, `baseType` |
| `specialForm` | Typing special form | `name` |
| `property` | Property descriptor | — |
| `enumComplement` | Enum instance with one or more canonical members excluded (e.g. `Color & ~Literal[Color.RED]`) | `className`, `moduleName`, `classId`, `excludedNames`, `rest` |
| `other` | Fallback for unhandled types | — |

All variants include an optional `display` field with ty's string representation.
