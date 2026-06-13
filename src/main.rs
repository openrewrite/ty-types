#![allow(dead_code)]

mod collector;
mod library;
mod project;
mod protocol;
mod registry;

use std::io::{self, BufRead, Write};
use std::process;

use protocol::{
    CliResult, GetLibraryApiParams, GetLibraryApiResult, GetStdlibApiParams, GetTypeRegistryResult,
    GetTypesParams, GetTypesResult, InitializeParams, InitializeResult, JsonRpcRequest,
    JsonRpcResponse,
};
use registry::{Boundary, TypeRegistry};
use ruff_db::files::system_path_to_file;
use ruff_db::system::{SystemPath, SystemPathBuf};
use ty_project::ProjectDatabase;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut serve = false;
    let mut project_root: Option<String> = None;
    let mut file_paths: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--serve" => serve = true,
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

    if serve {
        run_serve();
    } else if !file_paths.is_empty() {
        run_oneshot(&file_paths, project_root.as_deref());
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
}

/// One-shot mode: infer types for one or more files and print JSON to stdout.
fn run_oneshot(file_args: &[String], project_root_arg: Option<&str>) {
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

    let db = project::create_database(&root_str).unwrap_or_else(|e| {
        eprintln!("Error: failed to initialize project: {e}");
        process::exit(1);
    });

    let mut registry = TypeRegistry::new();
    let mut files = std::collections::HashMap::new();

    for file_arg in file_args {
        let absolute = std::fs::canonicalize(file_arg).unwrap_or_else(|e| {
            eprintln!("Error: cannot resolve '{file_arg}': {e}");
            process::exit(1);
        });

        let sys_path = SystemPathBuf::from_path_buf(absolute.clone()).unwrap_or_else(|p| {
            eprintln!("Error: non-Unicode path: {}", p.display());
            process::exit(1);
        });

        let file =
            system_path_to_file(&db, SystemPath::new(sys_path.as_str())).unwrap_or_else(|e| {
                eprintln!("Error: failed to resolve file '{file_arg}': {e}");
                process::exit(1);
            });

        let result = collector::collect_types(&db, file, &mut registry);
        files.insert(absolute.to_string_lossy().into_owned(), result.nodes);
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
                let (db, root, boundary) = match do_initialize(&request) {
                    Ok(parts) => {
                        write_response(
                            &stdout,
                            &JsonRpcResponse::success(
                                request.id.clone(),
                                serde_json::to_value(InitializeResult { ok: true }).unwrap(),
                            ),
                        );
                        parts
                    }
                    Err(response) => {
                        write_response(&stdout, &response);
                        continue;
                    }
                };

                // Enter session loop with persistent registry
                if run_session(&db, &root, boundary, &mut lines, &stdout) {
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
    project_root: &SystemPathBuf,
    boundary: Option<Boundary>,
    lines: &mut io::Lines<io::StdinLock<'_>>,
    stdout: &io::Stdout,
) -> bool {
    // The registry lives for the duration of this function,
    // sharing the 'db lifetime with the database reference. When a first-party
    // boundary was supplied at initialize, classes outside it come back as
    // `classRef`; otherwise every class is fully expanded (default behavior).
    let mut registry = match boundary {
        Some(b) => TypeRegistry::with_boundary(b),
        None => TypeRegistry::new(),
    };

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
            "getLibraryApi" => {
                let response = handle_get_library_api(&request, db);
                write_response(stdout, &response);
            }
            "getStdlibApi" => {
                let response = handle_get_stdlib_api(&request, db);
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

/// Parse a path string into a `SystemPathBuf`, returning a JSON-RPC error
/// response (tagged with `id`) if the path is not valid Unicode.
fn parse_system_path(
    path: &str,
    id: &serde_json::Value,
) -> Result<SystemPathBuf, JsonRpcResponse> {
    SystemPathBuf::from_path_buf(std::path::PathBuf::from(path)).map_err(|p| {
        JsonRpcResponse::error(
            id.clone(),
            -32000,
            format!("Non-Unicode path: {}", p.display()),
        )
    })
}

fn do_initialize(
    request: &JsonRpcRequest,
) -> Result<(ProjectDatabase, SystemPathBuf, Option<Boundary>), JsonRpcResponse> {
    let params: InitializeParams = serde_json::from_value(request.params.clone()).map_err(|e| {
        JsonRpcResponse::error(request.id.clone(), -32602, format!("Invalid params: {e}"))
    })?;

    let root = parse_system_path(&params.project_root, &request.id)?;

    // Optional first-party boundary for the session's getTypes registry.
    // `firstPartyRoot` takes precedence; `firstPartyModules` is ignored when it
    // is set. With neither, there is no boundary (classes are fully expanded).
    let boundary = if let Some(first_party_root) = &params.first_party_root {
        Some(Boundary::UnderRoot(parse_system_path(
            first_party_root,
            &request.id,
        )?))
    } else if !params.first_party_modules.is_empty() {
        Some(Boundary::Modules(
            params.first_party_modules.into_iter().collect(),
        ))
    } else {
        None
    };

    let db = project::create_database(&params.project_root).map_err(|e| {
        JsonRpcResponse::error(
            request.id.clone(),
            -32000,
            format!("Failed to initialize: {e}"),
        )
    })?;

    Ok((db, root, boundary))
}

fn handle_get_types<'db>(
    request: &JsonRpcRequest,
    db: &'db ProjectDatabase,
    project_root: &SystemPathBuf,
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

    let file_path = if std::path::Path::new(&params.file).is_absolute() {
        SystemPathBuf::from_path_buf(std::path::PathBuf::from(&params.file))
            .unwrap_or_else(|_| SystemPathBuf::from(params.file.as_str()))
    } else {
        project_root.join(&params.file)
    };

    let file = match system_path_to_file(db, SystemPath::new(file_path.as_str())) {
        Ok(f) => f,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32000,
                format!("Failed to resolve file '{}': {e}", params.file),
            );
        }
    };

    let result = collector::collect_types(db, file, registry);

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

fn handle_get_library_api(
    request: &JsonRpcRequest,
    db: &ProjectDatabase,
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

    let root = match parse_system_path(&params.root, &request.id) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Use a fresh, boundary-scoped registry per call (not the session registry):
    // boundary state is request-scoped, and library extraction must not share or
    // pollute the session's getTypes type IDs. `root` is used as supplied by the
    // caller; the caller is expected to pass a clean absolute path.
    let mut registry = TypeRegistry::with_boundary_root(root.clone());
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

fn handle_get_stdlib_api(
    request: &JsonRpcRequest,
    db: &ProjectDatabase,
) -> JsonRpcResponse {
    let params: GetStdlibApiParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(request.id.clone(), -32602, format!("Invalid params: {e}"));
        }
    };

    let requested: rustc_hash::FxHashSet<String> = params.modules.into_iter().collect();
    // A subset request makes everything outside it a classRef; an empty request
    // means all stdlib is local, so no boundary cut is needed (full expansion).
    let mut registry = if requested.is_empty() {
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
