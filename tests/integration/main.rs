mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use protocol::*;

/// Helper: spawn ty-types, send JSON-RPC requests, collect responses.
fn run_session(requests: &[&str]) -> Vec<serde_json::Value> {
    let binary = env!("CARGO_BIN_EXE_ty-types");

    let mut child = Command::new(binary)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn ty-types");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);

    // Write all requests
    for req in requests {
        writeln!(stdin, "{req}").unwrap();
    }
    drop(stdin); // close stdin to signal EOF

    // Read all responses
    let responses: Vec<serde_json::Value> = reader
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();

    child.wait().unwrap();
    responses
}

/// Helper: create a temp dir with Python files, return the path.
fn create_test_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    dir
}

fn initialize_request(project_root: &str, id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialize",
        "params": {"projectRoot": project_root},
        "id": id
    })
    .to_string()
}

fn get_types_request(file: &str, id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getTypes",
        "params": {"file": file},
        "id": id
    })
    .to_string()
}

fn get_type_registry_request(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getTypeRegistry",
        "id": id
    })
    .to_string()
}

fn shutdown_request(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "shutdown",
        "id": id
    })
    .to_string()
}

#[test]
fn test_initialize_and_shutdown() {
    let dir = create_test_project(&[]);
    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &shutdown_request(99),
    ]);

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["ok"], true);
    assert_eq!(responses[1]["id"], 99);
}

#[test]
fn test_simple_types() {
    let dir = create_test_project(&[("a.py", "x: int = 42\n")]);
    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("a.py", 2),
        &shutdown_request(99),
    ]);

    assert_eq!(responses.len(), 3);

    let result = &responses[1]["result"];
    let nodes: Vec<NodeInfo> = serde_json::from_value(result["nodes"].clone()).unwrap();
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Should have nodes for x, int, 42
    assert!(
        nodes.len() >= 3,
        "expected at least 3 nodes, got {}",
        nodes.len()
    );

    // Find the int literal node (42)
    let lit_42_node = nodes
        .iter()
        .find(|n| n.node_kind == "ExprNumberLiteral")
        .expect("should have a number literal node");
    let lit_42_type_id = lit_42_node
        .type_id
        .expect("number literal should have a type");

    let lit_42_type = &types[&lit_42_type_id.to_string()];
    assert_eq!(lit_42_type["kind"], "intLiteral");
    assert_eq!(lit_42_type["value"], 42);
    assert_eq!(lit_42_type["display"], "Literal[42]");

    // Find the int annotation
    let int_type = types
        .values()
        .find(|t| t["kind"] == "instance" && t["display"] == "int")
        .expect("should have an 'int' instance type");
    assert_eq!(int_type["className"], "int");
}

#[test]
fn test_cross_file_resolution() {
    let dir = create_test_project(&[
        ("a.py", "x: int = 42\n"),
        ("b.py", "from a import x\ny = x + 1\n"),
    ]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("a.py", 2),
        &get_types_request("b.py", 3),
        &shutdown_request(99),
    ]);

    assert_eq!(responses.len(), 4);

    // b.py should have x typed as int
    let b_result = &responses[2]["result"];
    let b_nodes: Vec<NodeInfo> = serde_json::from_value(b_result["nodes"].clone()).unwrap();

    // The import alias should resolve to 'int'
    let alias_node = b_nodes
        .iter()
        .find(|n| n.node_kind == "Alias")
        .expect("b.py should have an Alias node");
    assert!(
        alias_node.type_id.is_some(),
        "import alias should have a type"
    );
}

#[test]
fn test_cross_request_dedup() {
    let dir = create_test_project(&[
        ("a.py", "x: int = 42\n"),
        ("b.py", "from a import x\ny = x + 1\n"),
    ]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("a.py", 2),
        &get_types_request("b.py", 3),
        &shutdown_request(99),
    ]);

    let a_types: TypeMap = serde_json::from_value(responses[1]["result"]["types"].clone()).unwrap();
    let b_types: TypeMap = serde_json::from_value(responses[2]["result"]["types"].clone()).unwrap();

    // a.py should introduce 'int' as a new type
    let a_has_int = a_types.values().any(|t| t["display"] == "int");
    assert!(a_has_int, "a.py should introduce 'int' type");

    // b.py should NOT re-introduce 'int' (already seen from a.py)
    let b_has_int = b_types.values().any(|t| t["display"] == "int");
    assert!(
        !b_has_int,
        "b.py should NOT re-introduce 'int' — it was already sent with a.py"
    );

    // b.py nodes should still reference the 'int' type ID from a.py
    let b_nodes: Vec<NodeInfo> =
        serde_json::from_value(responses[2]["result"]["nodes"].clone()).unwrap();
    let int_type_id = a_types
        .iter()
        .find(|(_, t)| t["display"] == "int")
        .map(|(id, _)| id.parse::<u32>().unwrap())
        .unwrap();

    let b_uses_int = b_nodes.iter().any(|n| n.type_id == Some(int_type_id));
    assert!(
        b_uses_int,
        "b.py nodes should reference the same 'int' type ID as a.py"
    );
}

#[test]
fn test_function_and_class_types() {
    let dir = create_test_project(&[(
        "c.py",
        r#"def greet(name: str) -> str:
    return f"Hello, {name}!"

class Animal:
    species: str = "unknown"
"#,
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("c.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Should have a function type for greet
    let has_function = types
        .values()
        .any(|t| t["kind"] == "function" && t["display"].as_str().unwrap_or("").contains("greet"));
    assert!(has_function, "should have a function type for 'greet'");

    // Should have a class literal for Animal
    let has_class = types.values().any(|t| {
        t["kind"] == "classLiteral" && t["display"].as_str().unwrap_or("").contains("Animal")
    });
    assert!(has_class, "should have a class literal for 'Animal'");

    // Should have str instance type
    let has_str = types
        .values()
        .any(|t| t["kind"] == "instance" && t["display"] == "str");
    assert!(has_str, "should have 'str' instance type");
}

#[test]
fn test_union_type() {
    let dir = create_test_project(&[("u.py", "x: int | str = 42\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("u.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Should have a union type int | str
    let union_type = types
        .values()
        .find(|t| t["kind"] == "union" && t["display"].as_str() == Some("int | str"));
    assert!(
        union_type.is_some(),
        "should have an 'int | str' union type"
    );

    let union = union_type.unwrap();
    let members = union["members"].as_array().unwrap();
    assert_eq!(members.len(), 2, "int | str union should have 2 members");
}

#[test]
fn test_type_registry() {
    let dir = create_test_project(&[("a.py", "x: int = 42\n"), ("b.py", "y: str = 'hello'\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("a.py", 2),
        &get_types_request("b.py", 3),
        &get_type_registry_request(4),
        &shutdown_request(99),
    ]);

    let registry: TypeMap =
        serde_json::from_value(responses[3]["result"]["types"].clone()).unwrap();

    // Registry should contain types from both files
    let has_int = registry.values().any(|t| t["display"] == "int");
    let has_str = registry.values().any(|t| t["display"] == "str");
    let has_lit_42 = registry.values().any(|t| t["display"] == "Literal[42]");

    assert!(has_int, "registry should have 'int'");
    assert!(has_str, "registry should have 'str'");
    assert!(has_lit_42, "registry should have 'Literal[42]'");
}

#[test]
fn test_error_before_initialize() {
    let responses = run_session(&[&get_types_request("a.py", 1), &shutdown_request(99)]);

    assert_eq!(responses.len(), 2);
    assert!(
        responses[0]["error"].is_object(),
        "should return error before initialize"
    );
}

#[test]
fn test_invalid_file() {
    let dir = create_test_project(&[]);
    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("nonexistent.py", 2),
        &shutdown_request(99),
    ]);

    assert_eq!(responses.len(), 3);
    assert!(
        responses[1]["error"].is_object(),
        "should return error for nonexistent file"
    );
}
