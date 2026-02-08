#![allow(dead_code)]

mod collector;
mod project;
mod protocol;
mod registry;

use std::io::{self, BufRead, Write};

use protocol::{
    GetTypesParams, GetTypesResult, GetTypeRegistryResult, InitializeParams, InitializeResult,
    JsonRpcRequest, JsonRpcResponse,
};
use registry::TypeRegistry;
use ruff_db::files::system_path_to_file;
use ruff_db::system::{SystemPath, SystemPathBuf};
use ty_project::ProjectDatabase;

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut lines = stdin.lock().lines();

    // Outer loop: wait for initialize, then enter session
    loop {
        let Some(line) = read_line(&mut lines) else {
            break;
        };

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(&stdout, &JsonRpcResponse::error(
                    serde_json::Value::Null, -32700, format!("Parse error: {e}"),
                ));
                continue;
            }
        };

        match request.method.as_str() {
            "initialize" => {
                let (db, root) = match do_initialize(&request) {
                    Ok(pair) => {
                        write_response(&stdout, &JsonRpcResponse::success(
                            request.id.clone(),
                            serde_json::to_value(InitializeResult { ok: true }).unwrap(),
                        ));
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
                write_response(&stdout, &JsonRpcResponse::success(
                    request.id, serde_json::json!({"ok": true}),
                ));
                return;
            }
            _ => {
                write_response(&stdout, &JsonRpcResponse::error(
                    request.id, -32000, "Not initialized. Call 'initialize' first.".to_string(),
                ));
            }
        }
    }
}

/// Run the session loop with a persistent TypeRegistry.
/// Returns true if shutdown was requested.
fn run_session(
    db: &ProjectDatabase,
    project_root: &SystemPathBuf,
    lines: &mut io::Lines<io::StdinLock<'_>>,
    stdout: &io::Stdout,
) -> bool {
    // The registry lives for the duration of this function,
    // sharing the 'db lifetime with the database reference.
    let mut registry = TypeRegistry::new();

    loop {
        let Some(line) = read_line(lines) else {
            return true;
        };

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(stdout, &JsonRpcResponse::error(
                    serde_json::Value::Null, -32700, format!("Parse error: {e}"),
                ));
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
                write_response(stdout, &JsonRpcResponse::success(
                    request.id, serde_json::json!({"ok": true}),
                ));
                return true;
            }
            "initialize" => {
                // Re-initialize: respond with error suggesting restart
                write_response(stdout, &JsonRpcResponse::error(
                    request.id, -32000,
                    "Already initialized. Send 'shutdown' first to reinitialize.".to_string(),
                ));
            }
            _ => {
                write_response(stdout, &JsonRpcResponse::error(
                    request.id, -32601,
                    format!("Method not found: {}", request.method),
                ));
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

fn do_initialize(request: &JsonRpcRequest) -> Result<(ProjectDatabase, SystemPathBuf), JsonRpcResponse> {
    let params: InitializeParams = serde_json::from_value(request.params.clone())
        .map_err(|e| JsonRpcResponse::error(request.id.clone(), -32602, format!("Invalid params: {e}")))?;

    let root = SystemPathBuf::from_path_buf(std::path::PathBuf::from(&params.project_root))
        .map_err(|p| JsonRpcResponse::error(
            request.id.clone(), -32000,
            format!("Non-Unicode path: {}", p.display()),
        ))?;

    let db = project::create_database(&params.project_root)
        .map_err(|e| JsonRpcResponse::error(
            request.id.clone(), -32000, format!("Failed to initialize: {e}"),
        ))?;

    Ok((db, root))
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
                request.id.clone(), -32602, format!("Invalid params: {e}"),
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
                request.id.clone(), -32000,
                format!("Failed to resolve file '{}': {e}", params.file),
            );
        }
    };

    let result = collector::collect_types(db, file, registry);

    let response = GetTypesResult {
        nodes: result.nodes,
        types: result.new_types,
    };

    JsonRpcResponse::success(
        request.id.clone(),
        serde_json::to_value(response).unwrap(),
    )
}

fn handle_get_type_registry(request: &JsonRpcRequest, registry: &TypeRegistry<'_>) -> JsonRpcResponse {
    let response = GetTypeRegistryResult {
        types: registry.all_descriptors(),
    };

    JsonRpcResponse::success(
        request.id.clone(),
        serde_json::to_value(response).unwrap(),
    )
}
