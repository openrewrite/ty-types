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
- Rust edition 2024, requires Rust 1.93+.

## Wire Protocol

JSON-RPC over stdin/stdout, one JSON object per line.

Methods: `initialize`, `getTypes`, `getTypeRegistry`, `shutdown`.

## TypeDescriptor Variants

Each type in the registry is represented as a `TypeDescriptor` with a `kind` discriminator:

| Kind | Description | Key Fields |
|------|-------------|------------|
| `instance` | Instance of a class (`str`, `int`, `MyClass()`) | `className`, `moduleName`, `supertypes`, `typeArgs`, `classId` |
| `classLiteral` | Class object itself (`type[MyClass]`) | `className`, `moduleName`, `typeParameters`, `supertypes`, `members` |
| `subclassOf` | Subclass-of constraint | `base` |
| `union` | Union type (`X \| Y`) | `members` |
| `intersection` | Intersection type | `positive`, `negative` |
| `function` | Named function (`def foo(...)`) | `name`, `moduleName`, `typeParameters`, `parameters`, `returnType` |
| `callable` | Anonymous callable (`Callable[[int], str]`) | `parameters`, `returnType` |
| `boundMethod` | Bound method (`obj.method`) | `name`, `className`, `moduleName`, `typeParameters`, `parameters`, `returnType` |
| `wrapperDescriptor` | Descriptor wrapper (`__get__`, `__set__`) | `descriptorKind`, `parameters`, `returnType` |
| `knownInstance` | Well-known singleton instance (`TypeVar`, `typing.Callable`) | `className` |
| `intLiteral` | Literal int | `value` |
| `boolLiteral` | Literal bool | `value` |
| `stringLiteral` | Literal string | `value` |
| `bytesLiteral` | Literal bytes | `value` |
| `enumLiteral` | Enum member | `className`, `memberName` |
| `literalString` | `LiteralString` type | — |
| `dynamic` | `Any`, `Unknown`, etc. | `dynamicKind` |
| `never` | Bottom type | — |
| `truthy` / `falsy` | Truthiness narrowing | — |
| `typeVar` | Type variable in scope | `name`, `typevarKind`, `bound`, `constraints`, `defaultType` |
| `module` | Module literal | `moduleName` |
| `typeAlias` | Type alias (PEP 695 or legacy) | `name`, `valueType`, `typeParameters` |
| `typedDict` | TypedDict | `name`, `fields` |
| `typeIs` / `typeGuard` | Type narrowing returns | `narrowedType` / `guardedType` |
| `newType` | NewType wrapper | `name`, `baseType` |
| `specialForm` | Typing special form | `name` |
| `property` | Property descriptor | — |
| `other` | Fallback for unhandled types | — |

All variants include an optional `display` field with ty's string representation.
