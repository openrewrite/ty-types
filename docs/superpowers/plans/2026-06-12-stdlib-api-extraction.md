# getStdlibApi Stdlib-API Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add a `getStdlibApi` JSON-RPC method that extracts the Python standard library's public-API types (per the project's configured Python version), reusing the `getLibraryApi` machinery. Classes outside the requested local module set are emitted as `classRef`.

**Architecture:** The stdlib lives in ty's *vendored* typeshed, not on disk, so discovery uses `ty_module_resolver::all_modules(db)` filtered to the standard-library search path instead of a filesystem walk. The registry's boundary is generalized from "under a filesystem root" to an enum that also supports "local iff the class's top-level module is in a given set." Symbol enumeration and `__all__` filtering are factored into a shared helper used by both methods.

**Tech Stack:** Rust (edition 2024), the pinned `openrewrite/ruff` `ty-types-2` fork, serde/serde_json, subprocess integration tests with `tempfile`.

This builds on the just-merged `getLibraryApi` feature (same branch `frothy-fox`, same PR #19).

## Design summary

- **Method:** `getStdlibApi`, params `{ modules?: [string], includeDisplay?: bool }`. The Python version is taken from the `initialize` project config (no param). Output reuses the `getLibraryApi` result shape.
- **`modules` semantics (the local unit):** the set of requested top-level module names is the *local unit*. Classes whose owning top-level module is in the set → full `classLiteral`; classes in any other module (other stdlib modules, and `builtins` when not requested) → `classRef`. The output `modules` array contains exactly the requested modules' symbols (plus their submodules, e.g. `os.path` under `os`). **Omitting `modules` ⇒ all stdlib top-level modules are local** (a single fully-expanded dump, `builtins` included).
- **Discovery:** `all_modules(db)` filtered to modules whose `search_path(db).is_some_and(|sp| sp.is_standard_library())`, excluding the `_typeshed` package (typeshed's internal helpers, not importable).
- **Boundary generalization:** `TypeRegistry`'s `boundary_root: Option<SystemPathBuf>` becomes `boundary: Option<Boundary>` where
  ```
  enum Boundary { UnderRoot(SystemPathBuf), Modules(FxHashSet<String>) }
  ```
  A class is *external* iff a boundary is set and the class is not local to it:
  - `UnderRoot(root)` → local iff the class's file system-path starts with `root` (today's `getLibraryApi`).
  - `Modules(set)` → local iff the top-level component of the class's module name is in `set`.

## File Structure

- `src/protocol.rs` — `GetStdlibApiParams`; rename `GetLibraryApiResult` → shared `PublicApiResult` (with a type alias kept for clarity) OR reuse `GetLibraryApiResult` for both. (Plan reuses `GetLibraryApiResult` to minimize churn.)
- `src/registry.rs` — `Boundary` enum; `boundary: Option<Boundary>`; `with_boundary_root` / `with_boundary_modules` constructors; `is_external(db, file) -> bool`; update the two descriptor arms to call `is_external`.
- `src/library.rs` — factor a shared `collect_modules(db, items, registry)` helper (`items: impl Iterator<Item = (String /*module name*/, File)>`); keep `extract_library_api` (now building items from the fs walk); add `extract_stdlib_api(db, requested: &FxHashSet<String>, registry)` using `all_modules`.
- `src/main.rs` — `getStdlibApi` dispatch + `handle_get_stdlib_api`.
- `tests/integration/main.rs` — stdlib tests.
- `CLAUDE.md` — document the method.

---

### Task ST1: Boundary generalization in the registry (pure refactor, no behavior change)

Generalize the boundary so `getLibraryApi` keeps working identically, then later tasks add the module-set variant's usage. All 35 existing tests must stay green.

**Files:** Modify `src/registry.rs`, `src/main.rs`, `src/library.rs`.

- [ ] **Step 1: Replace the `boundary_root` field with a `Boundary` enum**

In `src/registry.rs`:
- Change the import `use ruff_db::system::{SystemPath, SystemPathBuf};` to also keep `SystemPath` (already there).
- Add near the top (module scope):
  ```rust
  /// Bounds which class literals get fully expanded vs. emitted as `classRef`.
  pub enum Boundary {
      /// Local iff the class's file is under this filesystem root (package extraction).
      UnderRoot(SystemPathBuf),
      /// Local iff the class's top-level module name is in this set (stdlib extraction).
      Modules(rustc_hash::FxHashSet<String>),
  }
  ```
- Replace the struct field `boundary_root: Option<SystemPathBuf>,` with `boundary: Option<Boundary>,`.
- In `new()`, replace `boundary_root: None,` with `boundary: None,`.
- Replace the `with_boundary` method with two constructors:
  ```rust
      /// Bound class-literal expansion to a filesystem `root` (package extraction).
      pub fn with_boundary_root(root: SystemPathBuf) -> Self {
          Self {
              boundary: Some(Boundary::UnderRoot(root)),
              ..Self::new()
          }
      }

      /// Bound class-literal expansion to a set of top-level module names
      /// (stdlib extraction): classes outside these modules become `classRef`.
      pub fn with_boundary_modules(modules: rustc_hash::FxHashSet<String>) -> Self {
          Self {
              boundary: Some(Boundary::Modules(modules)),
              ..Self::new()
          }
      }
  ```

- [ ] **Step 2: Replace `file_under_root` with an `is_external` method**

Remove the free function `file_under_root`. Add this method inside `impl<'db> TypeRegistry<'db>`:
```rust
    /// Whether `file`'s defining class should be emitted as a `classRef`
    /// (i.e. a boundary is set and the class is not local to it).
    fn is_external(&self, db: &dyn Db, file: ruff_db::files::File) -> bool {
        let Some(boundary) = &self.boundary else {
            return false;
        };
        let local = match boundary {
            Boundary::UnderRoot(root) => file
                .path(db)
                .as_system_path()
                .is_some_and(|p| p.starts_with(root.as_path())),
            Boundary::Modules(modules) => ty_module_resolver::file_to_module(db, file)
                .map(|m| m.name(db).to_string())
                .map(|name| name.split('.').next().unwrap_or(&name).to_string())
                .is_some_and(|top| modules.contains(&top)),
        };
        !local
    }
```
(`Db` and `ty_module_resolver` are already imported/used in this file.)

- [ ] **Step 3: Update the two descriptor arms to use `is_external`**

In `build_descriptor`:
- `Type::ClassLiteral` arm: replace
  ```rust
                let external = self
                    .boundary_root
                    .as_ref()
                    .is_some_and(|root| !file_under_root(db, cl_file, root.as_path()));
  ```
  with
  ```rust
                let external = self.is_external(db, cl_file);
  ```
- `Type::GenericAlias` arm: replace the analogous `let external = self.boundary_root...` block with
  ```rust
                let external = self.is_external(db, origin_file);
  ```

- [ ] **Step 4: Update the `getLibraryApi` call site**

In `src/main.rs`, in `handle_get_library_api`, change `TypeRegistry::with_boundary(root.clone())` to `TypeRegistry::with_boundary_root(root.clone())`.

In `src/library.rs`, update the doc comment on `extract_library_api` that mentions `with_boundary` to `with_boundary_root`.

- [ ] **Step 5: Verify the full suite is still green (no behavior change)**

Run: `cargo test 2>&1 | tail -15`
Expected: all 35 tests pass — this task is a pure refactor.

- [ ] **Step 6: Commit**

```bash
git add src/registry.rs src/main.rs src/library.rs
git commit -m "Generalize registry boundary to support module-set scoping"
```

---

### Task ST2: Protocol params for getStdlibApi

**Files:** Modify `src/protocol.rs`.

- [ ] **Step 1: Add `GetStdlibApiParams`**

After `GetLibraryApiParams` add:
```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetStdlibApiParams {
    /// Top-level stdlib module names to extract as the local unit. Empty ⇒ all
    /// stdlib modules are local (a single fully-expanded dump).
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default = "default_true")]
    pub include_display: bool,
}
```

`getStdlibApi` reuses the existing `GetLibraryApiResult` for its response (same `modules` + `types` shape).

- [ ] **Step 2: Verify compile + commit**

Run: `cargo check 2>&1 | tail -5` → `Finished`.

```bash
git add src/protocol.rs
git commit -m "Add GetStdlibApiParams"
```

---

### Task ST3: Shared symbol collection + stdlib discovery + dispatch (TDD)

**Files:** Modify `src/library.rs`, `src/main.rs`; test in `tests/integration/main.rs`.

- [ ] **Step 1: Write the failing test**

Append to `tests/integration/main.rs`:
```rust
fn get_stdlib_api_request(modules: &[&str], id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getStdlibApi",
        "params": {"modules": modules},
        "id": id
    })
    .to_string()
}

#[test]
fn test_stdlib_single_module_with_classref_boundary() {
    // Extract just `string`: its own classes are full classLiterals, while a
    // referenced builtins class (str) — outside the requested set — is a classRef.
    let dir = create_test_project(&[("placeholder.py", "x = 1\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_stdlib_api_request(&["string"], 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let modules = result["modules"].as_array().expect("modules array");

    // Only the requested module is emitted.
    assert!(modules.iter().any(|m| m["name"] == "string"), "should emit `string`");
    assert!(!modules.iter().any(|m| m["name"] == "os"), "should not emit unrequested `os`");

    // `string.Template` is a public class of the module.
    let string_mod = modules.iter().find(|m| m["name"] == "string").unwrap();
    let has_template = string_mod["symbols"].as_array().unwrap()
        .iter().any(|s| s["name"] == "Template");
    assert!(has_template, "string should expose Template");

    // builtins `str` (referenced, outside the local set) is a classRef, never a full classLiteral.
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();
    assert!(!types.values().any(|t| t["kind"] == "classLiteral" && t["className"] == "str"),
        "builtins str must not be a full classLiteral");
    assert!(types.values().any(|t| t["kind"] == "classRef" && t["className"] == "str"),
        "builtins str should be a classRef");
}
```

- [ ] **Step 2: Run it, verify it FAILS**

Run: `cargo test --test integration test_stdlib_single_module_with_classref_boundary 2>&1 | tail -15`
Expected: FAIL — "Method not found: getStdlibApi".

- [ ] **Step 3: Factor the shared collector and add stdlib extraction in `src/library.rs`**

Refactor `extract_library_api` so the per-module symbol work is shared, and add `extract_stdlib_api`. Add imports:
```rust
use rustc_hash::FxHashSet;
use ty_module_resolver::all_modules;
```

Add the shared helper (registers public symbols for a sequence of `(module_name, file)`):
```rust
/// Register the public top-level symbols of each `(module_name, file)` into the
/// registry, producing one `LibraryModuleInfo` per module.
fn collect_modules<'db>(
    db: &'db ProjectDatabase,
    items: impl IntoIterator<Item = (String, ruff_db::files::File, String)>,
    registry: &mut TypeRegistry<'db>,
) -> Vec<LibraryModuleInfo> {
    let mut modules = Vec::new();
    for (name, file, rel) in items {
        let scope = global_scope(db, file);
        let dunder_all = ty_python_semantic::dunder_all::dunder_all_names(db, file);

        let mut symbols = Vec::new();
        for mwd in all_end_of_scope_members(db, scope) {
            let sym_name = mwd.member.name.as_str();
            if !is_public_symbol(sym_name, dunder_all) {
                continue;
            }
            let type_id = registry.register(mwd.member.ty, db).type_id;
            symbols.push(LibrarySymbolInfo { name: sym_name.to_string(), type_id });
        }
        symbols.sort_by(|a, b| a.name.cmp(&b.name));
        modules.push(LibraryModuleInfo { name, file: rel, symbols });
    }
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    modules
}
```

Rewrite `extract_library_api` to build items from discovery and delegate:
```rust
pub fn extract_library_api<'db>(
    db: &'db ProjectDatabase,
    root: &SystemPath,
    registry: &mut TypeRegistry<'db>,
) -> anyhow::Result<Vec<LibraryModuleInfo>> {
    let mut items = Vec::new();
    for discovered in discover_module_files(root)? {
        let Ok(file) = system_path_to_file(db, discovered.abs.as_path()) else { continue };
        let Some(module) = ty_module_resolver::file_to_module(db, file) else { continue };
        items.push((module.name(db).to_string(), file, discovered.rel));
    }
    Ok(collect_modules(db, items, registry))
}
```

Add stdlib extraction:
```rust
/// Extract the public API of the standard library. `requested` is the set of
/// top-level module names to emit; empty ⇒ all stdlib modules. The `registry`
/// should be built with `TypeRegistry::with_boundary_modules(local_set)`.
pub fn extract_stdlib_api<'db>(
    db: &'db ProjectDatabase,
    requested: &FxHashSet<String>,
    registry: &mut TypeRegistry<'db>,
) -> Vec<LibraryModuleInfo> {
    let mut items = Vec::new();
    for module in all_modules(db) {
        let is_stdlib = module
            .search_path(db)
            .is_some_and(|sp| sp.is_standard_library());
        if !is_stdlib {
            continue;
        }
        let name = module.name(db).to_string();
        let top = name.split('.').next().unwrap_or(&name);
        // `_typeshed` is typeshed's internal helper package, not an importable module.
        if top == "_typeshed" {
            continue;
        }
        if !requested.is_empty() && !requested.contains(top) {
            continue;
        }
        let Some(file) = module.file(db) else { continue };
        // For stdlib, the "file path relative to root" is just the module name.
        items.push((name.clone(), file, name));
    }
    collect_modules(db, items, registry)
}
```

Note: verify `module.file(db)` returns `Option<File>` (use `let Some(file) = ... else continue`); if it returns `File` directly, drop the `let Some`. Verify `search_path(db)` returns `Option<&SearchPath>` and `is_standard_library()` is callable (it is `pub` after the fork widening). Check against `ruff/crates/ty_module_resolver/src/{module.rs,path.rs,list.rs}` and adjust the exact calls if the compiler complains; the names were confirmed present.

- [ ] **Step 4: Dispatch in `src/main.rs`**

- Add `GetStdlibApiParams` to the `use protocol::{...}` list.
- After the `"getLibraryApi"` arm in `run_session`, add:
  ```rust
              "getStdlibApi" => {
                  let response = handle_get_stdlib_api(&request, db);
                  write_response(stdout, &response);
              }
  ```
- Add the handler:
  ```rust
  fn handle_get_stdlib_api<'db>(
      request: &JsonRpcRequest,
      db: &'db ProjectDatabase,
  ) -> JsonRpcResponse {
      let params: GetStdlibApiParams = match serde_json::from_value(request.params.clone()) {
          Ok(p) => p,
          Err(e) => {
              return JsonRpcResponse::error(request.id.clone(), -32602, format!("Invalid params: {e}"));
          }
      };

      let requested: rustc_hash::FxHashSet<String> = params.modules.into_iter().collect();
      // Local unit for the classRef boundary = the requested modules; empty ⇒ all
      // stdlib local. We pass the same set; an empty set means "no boundary cut
      // between stdlib modules", so use a boundary only when a subset is requested.
      let mut registry = if requested.is_empty() {
          // Everything stdlib is local: a class is external only if it isn't stdlib,
          // which for a stdlib-only walk never happens — use an all-local module set
          // is impossible to enumerate cheaply, so use no boundary (full expansion).
          TypeRegistry::new()
      } else {
          TypeRegistry::with_boundary_modules(requested.clone())
      };

      let modules = library::extract_stdlib_api(db, &requested, &mut registry);

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

Rationale for the `requested.is_empty()` branch: with no subset requested, every stdlib module is local, so there is no cut to make and `TypeRegistry::new()` (no boundary, full expansion) is correct and avoids needing to enumerate "all stdlib module names" up front. When a subset is requested, `with_boundary_modules(requested)` makes everything outside it a `classRef`.

- [ ] **Step 5: Run the new test, verify it PASSES**

Run: `cargo test --test integration test_stdlib_single_module_with_classref_boundary 2>&1 | tail -20`
Expected: PASS. If `Template` isn't found, the stdlib may resolve differently for the default Python version — capture `responses[1]` and inspect; if `string`'s public API differs, pick another stable public class actually present (e.g. `Formatter`). If `str` shows as a full `classLiteral`, the module-set boundary isn't cutting — recheck `is_external`'s `Modules` arm and that `builtins` is not in `requested`.

- [ ] **Step 6: Full suite + commit**

Run: `cargo test 2>&1 | tail -15` → all pass.

```bash
git add src/library.rs src/main.rs tests/integration/main.rs
git commit -m "Add getStdlibApi: stdlib discovery via all_modules with module-set boundary"
```

---

### Task ST4: Whole-stdlib dump test, docs, final verification

**Files:** `tests/integration/main.rs`, `CLAUDE.md`.

- [ ] **Step 1: Add a whole-stdlib (no `modules`) smoke test**

```rust
#[test]
fn test_stdlib_all_modules_dump() {
    let dir = create_test_project(&[("placeholder.py", "x = 1\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        // No `modules` ⇒ all stdlib local, fully expanded.
        &get_stdlib_api_request(&[], 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let modules = result["modules"].as_array().expect("modules array");

    // Sanity: a broad set of well-known stdlib modules appear.
    for expected in ["os", "sys", "collections", "builtins"] {
        assert!(modules.iter().any(|m| m["name"] == expected),
            "stdlib dump should include `{expected}`");
    }
    // With everything local, builtins `str` is fully expanded (not a classRef).
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();
    assert!(types.values().any(|t| t["kind"] == "classLiteral" && t["className"] == "str"),
        "in a whole-stdlib dump, str should be a full classLiteral");
    assert!(!types.values().any(|t| t["kind"] == "classRef" && t["className"] == "str"),
        "in a whole-stdlib dump, str should not be a classRef");
}
```

Run: `cargo test --test integration test_stdlib_all_modules_dump 2>&1 | tail -20`
Expected: PASS. (This test exercises a large extraction; it may take a few seconds — acceptable.)

- [ ] **Step 2: Document the method in `CLAUDE.md`**

Update the Wire Protocol methods line to include `getStdlibApi`:
```
Methods: `initialize`, `getTypes`, `getTypeRegistry`, `getLibraryApi`, `getStdlibApi`, `shutdown`.
```
And add a short note after the existing `getLibraryApi` description (or in the methods area) describing `getStdlibApi`: extracts the standard library's public API for the project's configured Python version; `modules` selects the local unit (others become `classRef`); empty ⇒ whole-stdlib dump.

- [ ] **Step 3: Clippy + full suite**

Run: `cargo clippy --all-targets 2>&1 | tail -30` — no warnings from our files (ignore `ruff/`).
Run: `cargo test 2>&1 | tail -15` — all pass.

- [ ] **Step 4: Commit**

```bash
git add tests/integration/main.rs CLAUDE.md
git commit -m "Test whole-stdlib dump; document getStdlibApi"
```

---

## Notes for the implementer

- Edit only files under the worktree `/Users/knut/git/openrewrite/ty-types/.worktrees/frothy-fox/`.
- `all_modules`, `Module::search_path`, `Module::file`, `SearchPath::is_standard_library` were confirmed present in the fork; let the compiler confirm exact signatures and adjust calls minimally if needed (check `ruff/crates/ty_module_resolver/src/{list.rs,module.rs,path.rs}`).
- `file_to_module` is expected to resolve vendored stdlib files to their module names (it underpins ty's own stdlib resolution); the `Modules` boundary depends on this. If a stdlib class unexpectedly fails to resolve to a module and is wrongly treated as external, report it.
- Keep `getLibraryApi` behavior identical — Task ST1 is a pure refactor and the existing 35 tests are the guard.
