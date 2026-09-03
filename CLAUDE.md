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
cargo test --profile fast-test       # Run tests — use this, not --release

# Smoke test
echo '{"jsonrpc":"2.0","method":"initialize","params":{"projectRoot":"/path/to/project"},"id":1}
{"jsonrpc":"2.0","method":"getTypes","params":{"file":"example.py"},"id":2}
{"jsonrpc":"2.0","method":"shutdown","id":99}' | cargo run
```

Always run the suite under `fast-test`. The `release` profile links with fat LTO
over the whole ruff graph at `codegen-units = 1`, and `cargo test` pays that twice
— once for the binary the integration tests spawn, once for the test binary — so a
one-line edit to `src/` costs 9 minutes against `fast-test`'s 9 seconds. The 45
tests themselves take seconds either way. `fast-test`'s first build compiles ruff
from scratch (~9 min, once per checkout).

Reach for `--release` only to measure inference speed or ship a binary.

## Key Constraints

- The `ruff/` submodule is pinned to a specific commit on `openrewrite/ruff` `ty-types-2` branch, which widens `pub(crate)` → `pub` across `ty_python_semantic`. This gives us access to structured type internals (callable signatures, type var bounds, known instance classes, etc.).
- Update the submodule with `cd ruff && git fetch origin ty-types-2 && git checkout origin/ty-types-2`.
- Rust edition 2024, requires Rust 1.96+.

## Wire Protocol

JSON-RPC over stdin/stdout, one JSON object per line.

Methods: `initialize`, `getTypes`, `getTypeRegistry`, `shutdown`.

A descriptor answers for a type, and the registry dedupes by ty's interned `Type` — `LIMIT = 5` and `CAP = 5` in different modules are one `intLiteral` entry. Anything tied to a *symbol* therefore belongs on `NodeAttribution`, which is where `BindingInfo` lives. See README.md: BindingInfo.

Any file can fail: ty's inference panics on some inputs. `collector::catch_collect` confines that to the one file, so drive inference through it — a bare `collect_types` call lets one bad file abort the whole process. See README.md: getTypes.

### Backwards compatibility

Clients — rewrite-python, moderne-cli's `PythonTypeMapping` — upgrade this binary on their own schedule, so treat the wire format as a published contract. Keep changes additive: a new request parameter takes `#[serde(default)]`, and a new response field is `Option` with `skip_serializing_if`, so a client that neither sends nor reads it sees the output it saw before. Where a feature costs real time or payload, put it behind a request flag that defaults off, as `includeDisplay` and `includeBindings` do.

Removing a field, renaming one, changing its type, or changing what an existing one means all break clients and need a version bump. Releases are tagged `v0.0.N` and the workflow rewrites the placeholder `version = "0.0.0"`, so a breaking release means moving the minor — `v0.1.0` — rather than continuing the patch series.

Confirm compatibility by running the previous binary and the new one over the same corpus and comparing. `files` and `types` are `HashMap`s that serialize in a different order on every run, so compare parsed JSON; a byte diff reports a difference either way and tells you nothing.

## TypeDescriptor Variants

Each type in the registry is represented as a `TypeDescriptor` with a `kind` discriminator:

| Kind | Description | Key Fields |
|------|-------------|------------|
| `instance` | Instance of a class (`str`, `int`, `MyClass()`); `tupleElements` is present for tuples and their subclasses | `className`, `moduleName`, `qualifiedName`, `supertypes`, `typeArgs`, `classId`, `tupleElements` |
| `classLiteral` | Class object itself (`type[MyClass]`) | `className`, `moduleName`, `qualifiedName`, `typeParameters`, `supertypes`, `members` |
| `subclassOf` | Subclass-of constraint. `base` is a `classLiteral` for a class or a protocol declared as one, an `instance` for a synthesized protocol, otherwise `dynamic` or `typeVar` | `base` |
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
| `enumLiteral` | Enum member | `className`, `qualifiedName`, `memberName` |
| `literalString` | `LiteralString` type | — |
| `dynamic` | `Any`, `Unknown`, etc. | `dynamicKind` |
| `never` | Bottom type | — |
| `truthy` / `falsy` | Truthiness narrowing | — |
| `typeVar` | Type variable in scope; `typevarKind` is one of `TypeVar`, `ParamSpec`, `TypeVarTuple`, `Self`, `TypeAlias` | `name`, `typevarKind`, `bound`, `constraints`, `defaultType` |
| `module` | Module literal | `moduleName` |
| `typeAlias` | Type alias (PEP 695 or legacy) | `name`, `qualifiedName`, `valueType`, `typeParameters` |
| `typedDict` | TypedDict | `name`, `qualifiedName`, `fields`, `closed`, `extraItems` |
| `typeIs` / `typeGuard` | Type narrowing returns | `narrowedType` / `guardedType` |
| `newType` | NewType wrapper | `name`, `qualifiedName`, `baseType` |
| `specialForm` | Typing special form | `name` |
| `property` | Property descriptor | — |
| `enumComplement` | Enum instance with one or more canonical members excluded (e.g. `Color & ~Literal[Color.RED]`) | `className`, `moduleName`, `qualifiedName`, `classId`, `excludedNames`, `rest` |
| `other` | Fallback for unhandled types | — |

All variants include an optional `display` field with ty's string representation.

`qualifiedName` is the dotted path of the enclosing modules and classes followed by the item's own name (e.g. `a.b.C.D`) — the field to key a class by. See README.md for the per-variant field tables.
