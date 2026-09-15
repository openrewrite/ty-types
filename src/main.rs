#![allow(dead_code)]

mod collector;
mod project;
mod protocol;
mod registry;

use std::io::{self, BufRead, Write};
use std::panic::AssertUnwindSafe;
use std::process;

use protocol::{
    CliResult, GetTypeRegistryResult, GetTypesParams, GetTypesResult, InitializeParams,
    InitializeResult, JsonRpcRequest, JsonRpcResponse,
};
use registry::TypeRegistry;
use ruff_db::Db as _;
use ruff_db::files::{File, FileError, system_path_to_file};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ty_module_resolver::ResolverFile;
use ty_project::{Db as _, ProjectDatabase};
use ty_python_core::ProgramFile;
use ty_python_semantic::types::ProgramEnvironment;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut serve = false;
    let mut bindings = false;
    let mut project_root: Option<String> = None;
    let mut file_paths: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--serve" => serve = true,
            "--bindings" => bindings = true,
            "--project-root" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --project-root requires a value");
                    process::exit(1);
                }
                project_root = Some(args[i].clone());
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: unknown option '{arg}'");
                print_usage();
                process::exit(1);
            }
            _ => {
                file_paths.push(args[i].clone());
            }
        }
        i += 1;
    }

    if serve && !file_paths.is_empty() {
        eprintln!("Error: --serve and FILE are mutually exclusive");
        process::exit(1);
    }

    if serve && bindings {
        eprintln!(
            "Error: --bindings applies to one-shot mode; \
             pass includeBindings on each getTypes request instead"
        );
        process::exit(1);
    }

    if serve {
        run_serve();
    } else if !file_paths.is_empty() {
        run_oneshot(&file_paths, project_root.as_deref(), bindings);
    } else {
        print_usage();
        process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: ty-types <FILE>... [--project-root DIR]");
    eprintln!("       ty-types --serve");
    eprintln!();
    eprintln!("Modes:");
    eprintln!("  <FILE>...   Infer types for one or more Python files, print JSON to stdout");
    eprintln!("  --serve     Run as a JSON-RPC server over stdin/stdout");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --project-root DIR   Override project root (defaults to first FILE's parent)");
    eprintln!("  --bindings           Report where each referenced symbol is bound");
}

/// One-shot mode: infer types for one or more files and print JSON to stdout.
fn run_oneshot(file_args: &[String], project_root_arg: Option<&str>, bindings: bool) {
    let first_absolute = std::fs::canonicalize(&file_args[0]).unwrap_or_else(|e| {
        eprintln!("Error: cannot resolve '{}': {e}", file_args[0]);
        process::exit(1);
    });

    let root_str = match project_root_arg {
        Some(r) => std::fs::canonicalize(r)
            .unwrap_or_else(|e| {
                eprintln!("Error: cannot resolve project root '{r}': {e}");
                process::exit(1);
            })
            .to_string_lossy()
            .into_owned(),
        None => first_absolute
            .parent()
            .expect("file has no parent directory")
            .to_string_lossy()
            .into_owned(),
    };

    let (db, _root) = project::create_database(&root_str).unwrap_or_else(|e| {
        eprintln!("Error: failed to initialize project: {e}");
        process::exit(1);
    });

    let program = db.project().program(&db);
    let mut registry = TypeRegistry::new(program);
    let mut files = std::collections::HashMap::new();
    let mut skipped = 0usize;

    for file_arg in file_args {
        // Spelled as the search roots are, so the file matches the root it lives under.
        let sys_path = db
            .system()
            .canonicalize_path(SystemPath::new(file_arg))
            .unwrap_or_else(|e| {
                eprintln!("Error: cannot resolve '{file_arg}': {e}");
                process::exit(1);
            });

        let file = system_path_to_file(&db, &sys_path).unwrap_or_else(|e| {
            eprintln!("Error: failed to resolve file '{file_arg}': {e}");
            process::exit(1);
        });

        let program_file = ProgramFile::new(&db, file, program);
        let registry = &mut registry;
        let collect =
            AssertUnwindSafe(|| collector::collect_types(&db, program_file, registry, bindings));
        match collector::catch_collect(sys_path.as_str(), collect) {
            Ok(result) => {
                files.insert(sys_path.as_str().to_string(), result.nodes);
            }
            Err(message) => {
                eprintln!("warning: {message}");
                skipped += 1;
            }
        }
    }

    let output = CliResult {
        files,
        types: registry.all_descriptors(),
    };

    serde_json::to_writer_pretty(io::stdout().lock(), &output).unwrap_or_else(|e| {
        eprintln!("Error: failed to write JSON: {e}");
        process::exit(1);
    });
    println!();

    // The JSON is complete for the files that did resolve, so it is written either
    // way; the status is what tells a caller the run was partial.
    if skipped > 0 {
        eprintln!("Error: {skipped} file(s) could not be analyzed");
        process::exit(1);
    }
}

/// JSON-RPC server mode over stdin/stdout.
fn run_serve() {
    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut lines = stdin.lock().lines();

    // Outer loop: wait for initialize, then enter session
    while let Some(line) = read_line(&mut lines) {
        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    &stdout,
                    &JsonRpcResponse::error(
                        serde_json::Value::Null,
                        -32700,
                        format!("Parse error: {e}"),
                    ),
                );
                continue;
            }
        };

        match request.method.as_str() {
            "initialize" => {
                let (db, root) = match do_initialize(&request) {
                    Ok(pair) => {
                        write_response(
                            &stdout,
                            &JsonRpcResponse::success(
                                request.id.clone(),
                                serde_json::to_value(InitializeResult { ok: true }).unwrap(),
                            ),
                        );
                        pair
                    }
                    Err(response) => {
                        write_response(&stdout, &response);
                        continue;
                    }
                };

                // Enter session loop with persistent registry
                if run_session(&db, &root, &mut lines, &stdout) {
                    return; // shutdown requested
                }
                // If session ended without shutdown (e.g., re-initialize),
                // loop back to wait for next initialize
            }
            "shutdown" => {
                write_response(
                    &stdout,
                    &JsonRpcResponse::success(request.id, serde_json::json!({"ok": true})),
                );
                return;
            }
            _ => {
                write_response(
                    &stdout,
                    &JsonRpcResponse::error(
                        request.id,
                        -32000,
                        "Not initialized. Call 'initialize' first.".to_string(),
                    ),
                );
            }
        }
    }
}

/// Run the session loop with a persistent TypeRegistry.
/// Returns true if shutdown was requested.
fn run_session(
    db: &ProjectDatabase,
    project_root: &ProjectRoot,
    lines: &mut io::Lines<io::StdinLock<'_>>,
    stdout: &io::Stdout,
) -> bool {
    // The registry lives for the duration of this function,
    // sharing the 'db lifetime with the database reference.
    let mut registry = TypeRegistry::new(db.project().program(db));

    loop {
        let Some(line) = read_line(lines) else {
            return true;
        };

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    stdout,
                    &JsonRpcResponse::error(
                        serde_json::Value::Null,
                        -32700,
                        format!("Parse error: {e}"),
                    ),
                );
                continue;
            }
        };

        match request.method.as_str() {
            "getTypes" => {
                let response = handle_get_types(&request, db, project_root, &mut registry);
                write_response(stdout, &response);
            }
            "getTypeRegistry" => {
                let response = handle_get_type_registry(&request, &registry);
                write_response(stdout, &response);
            }
            "shutdown" => {
                write_response(
                    stdout,
                    &JsonRpcResponse::success(request.id, serde_json::json!({"ok": true})),
                );
                return true;
            }
            "initialize" => {
                // Re-initialize: respond with error suggesting restart
                write_response(
                    stdout,
                    &JsonRpcResponse::error(
                        request.id,
                        -32000,
                        "Already initialized. Send 'shutdown' first to reinitialize.".to_string(),
                    ),
                );
            }
            _ => {
                write_response(
                    stdout,
                    &JsonRpcResponse::error(
                        request.id,
                        -32601,
                        format!("Method not found: {}", request.method),
                    ),
                );
            }
        }
    }
}

fn read_line(lines: &mut io::Lines<io::StdinLock<'_>>) -> Option<String> {
    loop {
        match lines.next()? {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => return Some(line),
            Err(e) => {
                eprintln!("Error reading stdin: {e}");
                return None;
            }
        }
    }
}

fn write_response(stdout: &io::Stdout, response: &JsonRpcResponse) {
    let mut out = stdout.lock();
    let _ = serde_json::to_writer(&mut out, response);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// The project root as the client spelled it, and as the module resolver sees it.
struct ProjectRoot {
    given: SystemPathBuf,
    canonical: SystemPathBuf,
}

fn do_initialize(
    request: &JsonRpcRequest,
) -> Result<(ProjectDatabase, ProjectRoot), JsonRpcResponse> {
    let params: InitializeParams = serde_json::from_value(request.params.clone()).map_err(|e| {
        JsonRpcResponse::error(request.id.clone(), -32602, format!("Invalid params: {e}"))
    })?;

    let (db, canonical) = project::create_database(&params.project_root).map_err(|e| {
        JsonRpcResponse::error(
            request.id.clone(),
            -32000,
            format!("Failed to initialize: {e}"),
        )
    })?;

    Ok((
        db,
        ProjectRoot {
            given: SystemPathBuf::from(params.project_root.as_str()),
            canonical,
        },
    ))
}

/// The file to infer types for, taking the spelling of `absolute` that the module
/// resolver can name.
///
/// A file reached through a symlinked search root is matched by its resolved path, and
/// a file the project reaches by a symlink out of every search root is matched by the
/// path it was asked for.
fn resolve_file(db: &ProjectDatabase, absolute: &SystemPath) -> Result<File, FileError> {
    let resolved = system_path_to_file(db, project::canonical(db.system(), absolute));
    match resolved {
        Ok(file) if belongs_to_a_module(db, file) => Ok(file),
        _ => match system_path_to_file(db, absolute) {
            Ok(file) if belongs_to_a_module(db, file) => Ok(file),
            _ => resolved,
        },
    }
}

fn belongs_to_a_module(db: &ProjectDatabase, file: File) -> bool {
    let env = ProgramEnvironment::from_program(db.project().program(db));
    let resolver_file = ResolverFile::new(db, file, env.resolver_environment(db));
    ty_module_resolver::file_to_module(db, resolver_file).is_some()
}

fn handle_get_types<'db>(
    request: &JsonRpcRequest,
    db: &'db ProjectDatabase,
    project_root: &ProjectRoot,
    registry: &mut TypeRegistry<'db>,
) -> JsonRpcResponse {
    let params: GetTypesParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                format!("Invalid params: {e}"),
            );
        }
    };

    let requested = SystemPath::new(&params.file);
    let absolute = match requested.strip_prefix(&project_root.given) {
        Ok(relative) => project_root.canonical.join(relative),
        Err(_) if requested.is_absolute() => requested.to_path_buf(),
        Err(_) => project_root.canonical.join(requested),
    };

    let file = match resolve_file(db, &absolute) {
        Ok(f) => f,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32000,
                format!("Failed to resolve file '{}': {e}", params.file),
            );
        }
    };
    let program_file = ProgramFile::new(db, file, db.project().program(db));
    let include_bindings = params.include_bindings;
    let collect =
        AssertUnwindSafe(|| collector::collect_types(db, program_file, registry, include_bindings));
    let result = match collector::catch_collect(file.path(db).as_str(), collect) {
        Ok(result) => result,
        Err(message) => {
            return JsonRpcResponse::error(request.id.clone(), -32001, message);
        }
    };

    let mut types = result.new_types;
    if !params.include_display {
        for desc in types.values_mut() {
            desc.strip_display();
        }
    }

    let response = GetTypesResult {
        nodes: result.nodes,
        types,
    };

    JsonRpcResponse::success(request.id.clone(), serde_json::to_value(response).unwrap())
}

fn handle_get_type_registry(
    request: &JsonRpcRequest,
    registry: &TypeRegistry<'_>,
) -> JsonRpcResponse {
    let response = GetTypeRegistryResult {
        types: registry.all_descriptors(),
    };

    JsonRpcResponse::success(request.id.clone(), serde_json::to_value(response).unwrap())
}
