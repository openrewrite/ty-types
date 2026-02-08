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

- ty's internal types (ClassLiteral, FunctionType, etc.) have mostly `pub(crate)` accessors. We can pattern-match on `Type<'db>` variants (which is `pub`) and use `Type::display(db)` (which is `pub`), but cannot access most structured fields directly.
- The `ruff/` submodule is pinned to a specific commit. Update with `git -C ruff pull origin main`.
- Rust edition 2024, requires Rust 1.93+.

## Wire Protocol

JSON-RPC over stdin/stdout, one JSON object per line.

Methods: `initialize`, `getTypes`, `getTypeRegistry`, `shutdown`.
