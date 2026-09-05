# getLibraryApi Public-API Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `getLibraryApi` JSON-RPC method to ty-types that, given a path to an installed Python package directory, returns its public-API types (module-level declarations including class members) as JSON, with types defined outside the package emitted as lightweight `classRef` references.

**Architecture:** A new `library.rs` walks the package directory for `.py`/`.pyi` modules (stub preferred, underscore-private paths skipped), enumerates each module's public top-level symbols via ty's `global_scope` + `all_end_of_scope_members` (filtered by `__all__`/underscore), and registers each symbol's type through a `TypeRegistry` configured with a **boundary root**. The registry's one new behavior: a class literal defined outside the boundary root is emitted as a `classRef` descriptor (identity only, no member expansion) instead of a full `classLiteral`. `main.rs` dispatches the method using a fresh boundary-configured registry per call.

**Tech Stack:** Rust (edition 2024), the pinned `openrewrite/ruff` `ty-types-2` fork (consumed as a path-dependency submodule), serde/serde_json, JSON-RPC over stdio. Tests are subprocess integration tests (spawn `--serve`, drive JSON-RPC) using `tempfile`, matching the existing `tests/integration/main.rs` harness.

---

## File Structure

- **`ruff/scripts/widen_ty_visibility.sh`** (modify) — add a fix-up that makes `mod dunder_all;` public so `dunder_all_names` is reachable. Durable across future upstream syncs.
- **`Cargo.toml`** (modify) — add `ty_python_core` path dependency (provides `global_scope`).
- **`src/protocol.rs`** (modify) — `GetLibraryApiParams`, `LibrarySymbolInfo`, `LibraryModuleInfo`, `GetLibraryApiResult`, and the `TypeDescriptor::ClassRef` variant + its `strip_display` arm.
- **`src/registry.rs`** (modify) — a `boundary_root: Option<SystemPathBuf>` field, a `with_boundary` constructor, and `classRef` emission for class literals defined outside the root (in both the `Type::ClassLiteral` and `Type::GenericAlias` descriptor arms).
- **`src/library.rs`** (create) — module discovery (walk, stub preference, private filter), public-symbol filtering, and the extraction driver that produces `Vec<LibraryModuleInfo>`.
- **`src/main.rs`** (modify) — `mod library;`, dispatch `"getLibraryApi"`, and a `handle_get_library_api` handler.
- **`tests/integration/main.rs`** (modify) — behavior tests for each decision.
- **`CLAUDE.md`** (modify) — document the method and the `classRef` variant.

---

### Task 1: Widen `dunder_all` visibility in the fork

The `__all__` reader `dunder_all_names` is a `pub fn`, but its module is declared `mod dunder_all;` (private) in `ty_python_semantic/src/lib.rs`, so it is unreachable from our crate. The fork's `widen_ty_visibility.sh` blanket-converts `pub(crate)→pub` but does not touch bare `mod` declarations outside `types.rs`. Add a targeted fix-up, then rebuild the two custom fork commits **on the current pinned upstream base** (no upstream version bump).

**Files:**
- Modify: `ruff/scripts/widen_ty_visibility.sh` (Fix-ups section, after the `types.rs` bare-`mod` fix-up around line 118)

- [ ] **Step 1: Add the fix-up line to the script**

In `ruff/scripts/widen_ty_visibility.sh`, immediately after the existing block:

```bash
# Private `mod` declarations in types.rs — the blanket sed only catches
# `pub(crate) mod`, not bare `mod`.  Make them all public.
sed -i '' 's/^mod \([a-z_]*;\)/pub mod \1/' \
    "$TARGET/src/types.rs"
```

insert:

```bash
# `dunder_all` is declared bare-private in lib.rs; widen so external consumers
# can call `dunder_all::dunder_all_names` for __all__-aware public-API filtering.
sed -i '' 's/^mod dunder_all;$/pub mod dunder_all;/' \
    "$TARGET/src/lib.rs"
```

- [ ] **Step 2: Verify the script edit is the only working-tree change in the fork**

Run: `cd ruff && git status --porcelain`
Expected: exactly one modified file — ` M scripts/widen_ty_visibility.sh`

- [ ] **Step 3: Confirm the current two custom commits and their base**

Run: `cd ruff && git log --oneline -3`
Expected (top to bottom):
```
43fb3f5 Widen ty_python_semantic visibility to pub
ebd3c8d Add widen_ty_visibility.sh script
a4f73ff [ty] Route reStructuredText parameter documentation ...
```
The base (`a4f73ff…`) is `HEAD~2`. (If the hashes differ, use whatever the bottom commit is; it is the pinned upstream base.)

- [ ] **Step 4: Rebuild the two custom commits on the same base, including the new widening**

This re-creates the script commit (with the edited script) and the visibility commit (re-applying the blanket widening plus the new `dunder_all` line) without fetching a newer upstream.

```bash
cd ruff
# Move HEAD back to the pinned upstream base. --soft keeps everything staged;
# the subsequent mixed reset unstages but leaves the working tree untouched, so
# the Step-1 script edit and all widened files stay on disk. (Do NOT use git
# stash here: the mixed reset un-tracks scripts/widen_ty_visibility.sh because
# the base commit doesn't contain it, and `git stash pop` then refuses to
# overwrite the now-untracked file.)
git reset --soft HEAD~2
git reset                                  # unstage (working tree keeps the script edit + widened files)
# Re-commit the script files (now including the dunder_all fix-up)
git add scripts/widen_ty_visibility.sh scripts/CLAUDE.md
git commit -m "Add widen_ty_visibility.sh script

This branch (ty-types-2) is a fork of astral-sh/ruff that widens
visibility in ty_python_semantic from pub(crate)/pub(super) to pub
for consumption by OpenRewrite.

To sync with upstream:  scripts/widen_ty_visibility.sh
To sync and run tests:  scripts/widen_ty_visibility.sh --test"
# Apply the new dunder_all widening to the working tree
sed -i '' 's/^mod dunder_all;$/pub mod dunder_all;/' crates/ty_python_semantic/src/lib.rs
cargo fmt -p ty_python_semantic
# Re-commit the full visibility widening (existing widened files + dunder_all)
git add crates/ty_python_semantic
git commit -m "Widen ty_python_semantic visibility to pub"
```

- [ ] **Step 5: Verify `dunder_all` is now public and the crate compiles**

Run: `cd ruff && grep -n '^pub mod dunder_all;' crates/ty_python_semantic/src/lib.rs`
Expected: one match: `51:pub mod dunder_all;`

Run: `cd ruff && cargo check -p ty_python_semantic 2>&1 | tail -5`
Expected: finishes with `Finished` (no errors).

- [ ] **Step 6: Commit the submodule pointer bump in the outer repo**

The fork's commit hashes changed, so the `ruff` gitlink moved.

```bash
cd /Users/knut/git/openrewrite/ty-types/.worktrees/frothy-fox
git add ruff
git commit -m "Bump ruff submodule: widen dunder_all visibility for library API"
```

Note: pushing the rewritten `ty-types-2` fork branch to the remote (`cd ruff && git push --force origin ty-types-2`) is an outward-facing action — leave it to the maintainer or do it explicitly when ready. Local builds and tests work against the submodule working tree without pushing.

---

### Task 2: Add `ty_python_core` dependency and protocol types

**Files:**
- Modify: `Cargo.toml` (dependencies)
- Modify: `src/protocol.rs`

- [ ] **Step 1: Add the `ty_python_core` path dependency**

In `Cargo.toml`, under `[dependencies]`, after the `ty_module_resolver` line, add:

```toml
ty_python_core = { path = "ruff/crates/ty_python_core" }
```

- [ ] **Step 2: Add request/response param types to `protocol.rs`**

After `GetTypesParams` (around line 66) add:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLibraryApiParams {
    /// Absolute path to the installed package directory to extract.
    pub root: String,
    #[serde(default = "default_true")]
    pub include_display: bool,
}
```

After `GetTypeRegistryResult` (around line 90) add:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySymbolInfo {
    pub name: String,
    pub type_id: TypeId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryModuleInfo {
    /// Dotted module FQN, e.g. "requests.sessions".
    pub name: String,
    /// Module file path relative to the package root, e.g. "sessions.py".
    pub file: String,
    pub symbols: Vec<LibrarySymbolInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLibraryApiResult {
    pub modules: Vec<LibraryModuleInfo>,
    pub types: HashMap<TypeId, TypeDescriptor>,
}
```

- [ ] **Step 3: Add the `ClassRef` variant to `TypeDescriptor`**

In the `TypeDescriptor` enum, immediately after the `ClassLiteral { … }` variant (ends around line 208) add:

```rust
    // Reference to a class defined outside the extracted package boundary.
    // Identity only — no members, supertypes, or type parameters. Maps to
    // the V3 type-table TAG_CLASS_REF on the consumer side.
    #[serde(rename_all = "camelCase")]
    ClassRef {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
    },
```

- [ ] **Step 4: Add the `ClassRef` arm to `strip_display`**

In `impl TypeDescriptor::strip_display`, add `Self::ClassRef { display, .. }` to the match alternatives (e.g. right after `Self::ClassLiteral { display, .. }`):

```rust
            | Self::ClassLiteral { display, .. }
            | Self::ClassRef { display, .. }
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check 2>&1 | tail -5`
Expected: `Finished` with no errors (a `dead_code` warning for the new unused types is acceptable — the crate has `#![allow(dead_code)]`).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/protocol.rs
git commit -m "Add ty_python_core dep and library-API protocol types"
```

---

### Task 3: Minimal `library.rs` + dispatch — list modules and public symbols

Bootstrap a runnable `getLibraryApi` with no filtering refinements yet: discover every `.py`/`.pyi` under the root and register every non-underscore top-level symbol with full type expansion (no boundary, no `__all__`, no stub preference). Later tasks add each refinement test-first.

**Files:**
- Create: `src/library.rs`
- Modify: `src/main.rs`
- Test: `tests/integration/main.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/integration/main.rs`:

```rust
fn get_library_api_request(root: &str, id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getLibraryApi",
        "params": {"root": root},
        "id": id
    })
    .to_string()
}

#[test]
fn test_library_lists_modules_and_symbols() {
    let dir = create_test_project(&[
        ("mypkg/__init__.py", "VERSION: str = \"1.0\"\n"),
        ("mypkg/core.py", "class Widget:\n    size: int = 1\n"),
    ]);
    let pkg_root = dir.path().join("mypkg");

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_library_api_request(pkg_root.to_str().unwrap(), 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let modules = result["modules"].as_array().expect("modules array");

    // The package's two modules are present.
    let core = modules
        .iter()
        .find(|m| m["name"] == "mypkg.core")
        .expect("should list mypkg.core");

    // Widget is a public symbol of mypkg.core.
    let widget = core["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Widget")
        .expect("mypkg.core should expose Widget");
    let widget_type_id = widget["typeId"].as_u64().unwrap();

    // Its type is in the registry as a classLiteral.
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();
    assert_eq!(types[&widget_type_id.to_string()]["kind"], "classLiteral");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test integration test_library_lists_modules_and_symbols 2>&1 | tail -15`
Expected: FAIL — the response is a JSON-RPC error (`Method not found: getLibraryApi`), so `result["modules"]` is null and `.as_array()` panics with "modules array".

- [ ] **Step 3: Create `src/library.rs`**

```rust
use std::collections::BTreeMap;

use ruff_db::files::system_path_to_file;
use ruff_db::system::{SystemPath, SystemPathBuf};
use ty_project::ProjectDatabase;
use ty_python_core::global_scope;
use ty_python_semantic::types::list_members::all_end_of_scope_members;

use crate::protocol::{LibraryModuleInfo, LibrarySymbolInfo};
use crate::registry::TypeRegistry;

struct DiscoveredModule {
    /// Absolute path to the chosen file (`.pyi` preferred over `.py`).
    abs: SystemPathBuf,
    /// Path relative to the package root, e.g. "core.py".
    rel: String,
}

/// True for a path component that marks a private module/package by the
/// underscore convention (`_internal`, `_impl`), but NOT for dunders such as
/// `__init__` / `__main__` / `__pycache__`.
fn is_private_component(comp: &str) -> bool {
    comp.starts_with('_') && !(comp.starts_with("__") && comp.ends_with("__"))
}

/// Whether a module-level symbol is public. With `__all__` present, membership
/// in it is authoritative; otherwise underscore-prefixed names are private.
fn is_public_symbol(name: &str, dunder_all: Option<&rustc_hash::FxHashSet<ruff_python_ast::name::Name>>) -> bool {
    match dunder_all {
        Some(names) => names.iter().any(|n| n.as_str() == name),
        None => !name.starts_with('_'),
    }
}

/// Walk `root` for importable module files. `.pyi` wins over `.py` for the same
/// module; files/dirs with an underscore-private path component are skipped.
fn discover_module_files(root: &SystemPath) -> anyhow::Result<Vec<DiscoveredModule>> {
    let root_std = std::path::Path::new(root.as_str());
    // key = module path relative to root WITHOUT extension; value = chosen file
    let mut chosen: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    let mut stack = vec![root_std.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();

            if file_type.is_dir() {
                if name == "__pycache__" || is_private_component(&name) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                if ext != "py" && ext != "pyi" {
                    continue;
                }
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if is_private_component(stem) {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(root_std) else {
                    continue;
                };
                let key = rel.with_extension("").to_string_lossy().into_owned();
                // Prefer .pyi: insert unless an already-chosen file is a stub.
                let keep_existing = chosen
                    .get(&key)
                    .and_then(|p| p.extension().and_then(|e| e.to_str()))
                    == Some("pyi");
                if !keep_existing {
                    chosen.insert(key, path);
                }
            }
        }
    }

    let mut modules = Vec::new();
    for (_key, abs_std) in chosen {
        let rel = abs_std
            .strip_prefix(root_std)
            .unwrap_or(&abs_std)
            .to_string_lossy()
            .into_owned();
        let Some(abs) = SystemPathBuf::from_path_buf(abs_std).ok() else {
            continue;
        };
        modules.push(DiscoveredModule { abs, rel });
    }
    Ok(modules)
}

/// Extract the public API of the package rooted at `root`. `registry` should be
/// constructed with `TypeRegistry::with_boundary(root)` so types defined outside
/// the package collapse to `classRef`.
pub fn extract_library_api<'db>(
    db: &'db ProjectDatabase,
    root: &SystemPath,
    registry: &mut TypeRegistry<'db>,
) -> anyhow::Result<Vec<LibraryModuleInfo>> {
    let mut modules = Vec::new();

    for discovered in discover_module_files(root)? {
        let Ok(file) = system_path_to_file(db, discovered.abs.as_path()) else {
            continue;
        };
        let Some(module) = ty_module_resolver::file_to_module(db, file) else {
            continue;
        };
        let name = module.name(db).to_string();

        let scope = global_scope(db, file);
        let dunder_all = ty_python_semantic::dunder_all::dunder_all_names(db, file);

        let mut symbols = Vec::new();
        for mwd in all_end_of_scope_members(db, scope) {
            let sym_name = mwd.member.name.as_str();
            if !is_public_symbol(sym_name, dunder_all.as_ref()) {
                continue;
            }
            let type_id = registry.register(mwd.member.ty, db).type_id;
            symbols.push(LibrarySymbolInfo {
                name: sym_name.to_string(),
                type_id,
            });
        }
        symbols.sort_by(|a, b| a.name.cmp(&b.name));

        modules.push(LibraryModuleInfo {
            name,
            file: discovered.rel,
            symbols,
        });
    }

    modules.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(modules)
}
```

Note: this task already wires `__all__`, underscore, stub-preference, and private-module filtering into the helpers. Tasks 4–6 add the **tests that lock those behaviors in**; if a helper has a bug those tests will catch it (they are written to fail first by construction where the behavior is non-trivial). The boundary/`classRef` behavior is added in Task 7 — until then `with_boundary` does not yet alter expansion.

- [ ] **Step 4: Wire the module and dispatch in `main.rs`**

Add the module declaration near the top of `src/main.rs` (after `mod collector;`):

```rust
mod library;
```

Extend the `use protocol::{…}` import list to include `GetLibraryApiParams, GetLibraryApiResult`.

In `run_session`'s `match request.method.as_str()` block, add a new arm after the `"getTypeRegistry"` arm:

```rust
            "getLibraryApi" => {
                let response = handle_get_library_api(&request, db);
                write_response(stdout, &response);
            }
```

Add the handler function (next to `handle_get_type_registry`):

```rust
fn handle_get_library_api<'db>(
    request: &JsonRpcRequest,
    db: &'db ProjectDatabase,
) -> JsonRpcResponse {
    let params: GetLibraryApiParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                format!("Invalid params: {e}"),
            );
        }
    };

    let root = match SystemPathBuf::from_path_buf(std::path::PathBuf::from(&params.root)) {
        Ok(p) => p,
        Err(p) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32000,
                format!("Non-Unicode path: {}", p.display()),
            );
        }
    };

    let mut registry = TypeRegistry::with_boundary(root.clone());
    let modules = match library::extract_library_api(db, root.as_path(), &mut registry) {
        Ok(m) => m,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32000,
                format!("Failed to extract library API: {e}"),
            );
        }
    };

    let mut types = registry.all_descriptors();
    if !params.include_display {
        for desc in types.values_mut() {
            desc.strip_display();
        }
    }

    let response = GetLibraryApiResult { modules, types };
    JsonRpcResponse::success(request.id.clone(), serde_json::to_value(response).unwrap())
}
```

This references `TypeRegistry::with_boundary`, added in Task 7. To keep this task self-contained and green, add a temporary shim now and replace it in Task 7. In `src/registry.rs`, add this method inside `impl<'db> TypeRegistry<'db>` (next to `new`):

```rust
    /// Construct a registry whose class-literal expansion is bounded to a package
    /// root. (Boundary behavior is added in Task 7; for now this ignores `_root`.)
    pub fn with_boundary(_root: SystemPathBuf) -> Self {
        Self::new()
    }
```

Add `use ruff_db::system::SystemPathBuf;` to `src/registry.rs` imports.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test integration test_library_lists_modules_and_symbols 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 6: Run the full suite to confirm no regressions**

Run: `cargo test 2>&1 | tail -15`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/library.rs src/main.rs src/registry.rs
git commit -m "Add getLibraryApi: discover modules and public symbols"
```

---

### Task 4: Exclude underscore-private modules and packages

**Files:**
- Test: `tests/integration/main.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/integration/main.rs`:

```rust
#[test]
fn test_library_excludes_private_modules() {
    let dir = create_test_project(&[
        ("mypkg/__init__.py", ""),
        ("mypkg/public.py", "class Public: pass\n"),
        ("mypkg/_private.py", "class Hidden: pass\n"),
        ("mypkg/_internal/__init__.py", ""),
        ("mypkg/_internal/secret.py", "class Secret: pass\n"),
    ]);
    let pkg_root = dir.path().join("mypkg");

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_library_api_request(pkg_root.to_str().unwrap(), 2),
        &shutdown_request(99),
    ]);

    let modules = responses[1]["result"]["modules"].as_array().unwrap();
    let names: Vec<&str> = modules.iter().map(|m| m["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"mypkg.public"), "public module kept: {names:?}");
    assert!(!names.iter().any(|n| n.contains("_private")), "drop _private.py: {names:?}");
    assert!(!names.iter().any(|n| n.contains("_internal")), "drop _internal pkg: {names:?}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test integration test_library_excludes_private_modules 2>&1 | tail -15`
Expected: PASS (the `is_private_component` filter from Task 3 already handles this). If it FAILS, fix `is_private_component` / `discover_module_files` in `src/library.rs` until it passes — the test is the specification.

- [ ] **Step 3: Commit**

```bash
git add tests/integration/main.rs
git commit -m "Test: getLibraryApi excludes underscore-private modules"
```

---

### Task 5: Prefer `.pyi` stubs over `.py`

**Files:**
- Test: `tests/integration/main.rs`

- [ ] **Step 1: Write the failing test**

The `.py` and `.pyi` define a same-named symbol with *different* types; the stub must win.

```rust
#[test]
fn test_library_prefers_pyi_stub() {
    let dir = create_test_project(&[
        ("mypkg/__init__.py", ""),
        ("mypkg/mod.py", "value = 1\n"),          // runtime: int
        ("mypkg/mod.pyi", "value: str\n"),         // stub: str
    ]);
    let pkg_root = dir.path().join("mypkg");

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_library_api_request(pkg_root.to_str().unwrap(), 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let modules = result["modules"].as_array().unwrap();
    let m = modules.iter().find(|m| m["name"] == "mypkg.mod").expect("mypkg.mod present");
    assert_eq!(m["file"], "mod.pyi", "should choose the stub file");

    let value = m["symbols"].as_array().unwrap().iter()
        .find(|s| s["name"] == "value").expect("value symbol");
    let type_id = value["typeId"].as_u64().unwrap();
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();
    // From the stub, `value` is annotated `str`.
    assert_eq!(types[&type_id.to_string()]["display"], "str");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test integration test_library_prefers_pyi_stub 2>&1 | tail -15`
Expected: PASS (the stub-preference logic from Task 3 handles this). If it FAILS, fix the `.pyi`-preference branch in `discover_module_files` until it passes.

- [ ] **Step 3: Commit**

```bash
git add tests/integration/main.rs
git commit -m "Test: getLibraryApi prefers .pyi stubs over .py"
```

---

### Task 6: Filter symbols by `__all__` and underscore convention

**Files:**
- Test: `tests/integration/main.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn test_library_symbol_visibility() {
    let dir = create_test_project(&[
        ("mypkg/__init__.py", ""),
        // With __all__: only Exported is public, even though Hidden is non-underscore.
        ("mypkg/curated.py", "__all__ = [\"Exported\"]\nclass Exported: pass\nclass Hidden: pass\n"),
        // Without __all__: underscore name is private, plain name is public.
        ("mypkg/plain.py", "class Shown: pass\ndef _helper(): pass\n"),
    ]);
    let pkg_root = dir.path().join("mypkg");

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_library_api_request(pkg_root.to_str().unwrap(), 2),
        &shutdown_request(99),
    ]);

    let modules = responses[1]["result"]["modules"].as_array().unwrap();

    let curated = modules.iter().find(|m| m["name"] == "mypkg.curated").unwrap();
    let curated_syms: Vec<&str> = curated["symbols"].as_array().unwrap()
        .iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(curated_syms.contains(&"Exported"), "Exported kept: {curated_syms:?}");
    assert!(!curated_syms.contains(&"Hidden"), "Hidden excluded by __all__: {curated_syms:?}");

    let plain = modules.iter().find(|m| m["name"] == "mypkg.plain").unwrap();
    let plain_syms: Vec<&str> = plain["symbols"].as_array().unwrap()
        .iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(plain_syms.contains(&"Shown"), "Shown kept: {plain_syms:?}");
    assert!(!plain_syms.contains(&"_helper"), "_helper excluded: {plain_syms:?}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test integration test_library_symbol_visibility 2>&1 | tail -15`
Expected: PASS (the `is_public_symbol` + `dunder_all_names` logic from Task 3 handles this). If `Hidden` appears, the `__all__` reader is not reachable — re-check Task 1 (the `dunder_all` widening) and the `dunder_all_names` call in `src/library.rs`.

- [ ] **Step 3: Commit**

```bash
git add tests/integration/main.rs
git commit -m "Test: getLibraryApi filters symbols by __all__ and underscore"
```

---

### Task 7: Boundary — emit `classRef` for classes defined outside the package

**Files:**
- Modify: `src/registry.rs`
- Test: `tests/integration/main.rs`

- [ ] **Step 1: Write the failing test**

A class defined in the package stays a full `classLiteral` (with members); a referenced stdlib class (`int`, defined in typeshed, outside the package root) appears as a `classRef`.

```rust
#[test]
fn test_library_boundary_classref() {
    let dir = create_test_project(&[
        ("mypkg/__init__.py", ""),
        ("mypkg/core.py", "class Widget:\n    size: int = 1\n\ndef make() -> Widget:\n    return Widget()\n"),
    ]);
    let pkg_root = dir.path().join("mypkg");

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_library_api_request(pkg_root.to_str().unwrap(), 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // The in-package class is a full classLiteral with members.
    let widget = types.values()
        .find(|t| t["kind"] == "classLiteral" && t["className"] == "Widget")
        .expect("Widget should be a full classLiteral");
    assert!(widget["members"].as_array().map(|m| !m.is_empty()).unwrap_or(false),
        "Widget should carry members");

    // `int` (typeshed, outside the package) must appear ONLY as a classRef,
    // never as a full classLiteral.
    let int_full = types.values()
        .any(|t| t["kind"] == "classLiteral" && t["className"] == "int");
    assert!(!int_full, "int must not be expanded as a full classLiteral");
    let int_ref = types.values()
        .any(|t| t["kind"] == "classRef" && t["className"] == "int");
    assert!(int_ref, "int should appear as a classRef");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test integration test_library_boundary_classref 2>&1 | tail -20`
Expected: FAIL — with the Task 3 shim, `with_boundary` ignores the root, so `int` is expanded as a full `classLiteral` and `int_full` is `true` (assertion "int must not be expanded as a full classLiteral" fails).

- [ ] **Step 3: Replace the `with_boundary` shim with a real boundary field**

In `src/registry.rs`, add a field to the `TypeRegistry` struct:

```rust
pub struct TypeRegistry<'db> {
    type_to_id: FxHashMap<Type<'db>, TypeId>,
    descriptors: FxHashMap<TypeId, TypeDescriptor>,
    next_id: TypeId,
    tracked_new_ids: Vec<TypeId>,
    /// When set, class literals whose definition file is not under this root
    /// are emitted as `classRef` instead of being expanded.
    boundary_root: Option<SystemPathBuf>,
}
```

Set it to `None` in `new()`:

```rust
    pub fn new() -> Self {
        Self {
            type_to_id: FxHashMap::default(),
            descriptors: FxHashMap::default(),
            next_id: 1,
            tracked_new_ids: Vec::new(),
            boundary_root: None,
        }
    }
```

Replace the temporary `with_boundary` shim from Task 3 with:

```rust
    /// Construct a registry that bounds class-literal expansion to `root`:
    /// classes defined outside `root` are emitted as `classRef`.
    pub fn with_boundary(root: SystemPathBuf) -> Self {
        Self {
            boundary_root: Some(root),
            ..Self::new()
        }
    }
```

- [ ] **Step 4: Add the boundary helper and `classRef` emission**

Add this free function to `src/registry.rs` (module scope, e.g. above `impl`):

```rust
fn file_under_root(db: &dyn Db, file: ruff_db::files::File, root: &ruff_db::system::SystemPath) -> bool {
    file.path(db)
        .as_system_path()
        .is_some_and(|p| p.starts_with(root))
}
```

Add the import `use ruff_db::system::{SystemPath, SystemPathBuf};` (merge with the `SystemPathBuf` import added in Task 3).

In `build_descriptor`, at the very start of the `Type::ClassLiteral(class_literal)` arm (before computing `display`), add:

```rust
            Type::ClassLiteral(class_literal) => {
                let cl_file = class_literal.file(db);
                let external = self
                    .boundary_root
                    .as_ref()
                    .is_some_and(|root| !file_under_root(db, cl_file, root.as_path()));
                if external {
                    let display = self.display_string(ty, db);
                    let class_name = class_literal.name(db).to_string();
                    let module_name = self.resolve_module_name(db, cl_file);
                    return TypeDescriptor::ClassRef {
                        display,
                        class_name,
                        module_name,
                    };
                }
                // ── existing full-expansion code continues unchanged ──
                let display = self.display_string(ty, db);
                // ...
            }
```

Apply the same guard at the start of the `Type::GenericAlias(alias)` arm, using the alias origin's file:

```rust
            Type::GenericAlias(alias) => {
                let origin = alias.origin(db);
                let origin_file = origin.file(db);
                let external = self
                    .boundary_root
                    .as_ref()
                    .is_some_and(|root| !file_under_root(db, origin_file, root.as_path()));
                if external {
                    let display = self.display_string(ty, db);
                    let class_name = origin.name(db).to_string();
                    let module_name = self.resolve_module_name(db, origin_file);
                    return TypeDescriptor::ClassRef {
                        display,
                        class_name,
                        module_name,
                    };
                }
                // ── existing full-expansion code continues unchanged ──
                let display = self.display_string(ty, db);
                // ...
            }
```

(Keep the existing bodies; only the early `external` guard and, in the `GenericAlias` arm, hoisting `origin`/`origin_file` to the top are new. Remove the now-duplicate `let origin = alias.origin(db);` further down that arm.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test integration test_library_boundary_classref 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Run the full suite**

Run: `cargo test 2>&1 | tail -15`
Expected: all tests pass — including the existing `getTypes` tests, which use `TypeRegistry::new()` (`boundary_root = None`) and are therefore unaffected.

- [ ] **Step 7: Commit**

```bash
git add src/registry.rs tests/integration/main.rs
git commit -m "Emit classRef for classes defined outside the package boundary"
```

---

### Task 8: Documentation and final verification

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Document the new method**

In `CLAUDE.md`, in the "Wire Protocol" section, update the methods line to include `getLibraryApi`:

```
Methods: `initialize`, `getTypes`, `getTypeRegistry`, `getLibraryApi`, `shutdown`.
```

- [ ] **Step 2: Document the `classRef` descriptor**

In the "TypeDescriptor Variants" table in `CLAUDE.md`, add a row (e.g. after the `subclassOf` row):

```
| `classRef` | Reference to a class defined outside the extracted library boundary (identity only; maps to the type-table `TAG_CLASS_REF`) | `className`, `moduleName` |
```

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -20`
Expected: no warnings from our crate (`src/*.rs`, `tests/*`). Fix any that appear.

- [ ] **Step 4: Run the full test suite once more**

Run: `cargo test 2>&1 | tail -15`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "Document getLibraryApi method and classRef descriptor"
```

---

## Notes for the implementer

- **Always read/edit files under the worktree** `/Users/knut/git/openrewrite/ty-types/.worktrees/frothy-fox/`, never the main checkout.
- **`db` types:** `extract_library_api` takes `&ProjectDatabase` (concrete). `global_scope` wants `&dyn ty_python_core::Db` and `all_end_of_scope_members` wants `&dyn ty_python_semantic::Db`; both accept `&ProjectDatabase` via concrete→`dyn` coercion, exactly as `collector.rs`/`main.rs` already pass it.
- **`SystemPathBuf::as_path()`** yields `&SystemPath`; `SystemPath::starts_with` does component-wise prefix matching.
- **First-party resolution:** the test fixtures place the package under the `initialize` project root, so `file_to_module` resolves dotted names (`mypkg`, `mypkg.core`) against the project's first-party search path. The caller must `initialize` with a root from which the package is importable.
- **Why a fresh registry per call:** `handle_get_library_api` uses its own boundary-configured `TypeRegistry`, separate from the session registry, so `classRef` cuts never mix with the fully-expanded descriptors `getTypes` produces (and dedup can't hand back a full class where a ref is wanted).
