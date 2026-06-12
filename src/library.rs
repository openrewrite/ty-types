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
fn is_public_symbol(
    name: &str,
    dunder_all: Option<&rustc_hash::FxHashSet<ruff_python_ast::name::Name>>,
) -> bool {
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
        let Ok(abs) = SystemPathBuf::from_path_buf(abs_std) else {
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

    // Modules that ty cannot resolve to a dotted name (e.g. a stray file with no
    // reachable package chain) are silently skipped — they are not part of any
    // importable public API.
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
            if !is_public_symbol(sym_name, dunder_all) {
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
