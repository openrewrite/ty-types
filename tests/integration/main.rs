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
fn test_generic_function_type_parameters() {
    let dir = create_test_project(&[("g.py", "def identity[T](x: T) -> T: return x\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("g.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the function type for 'identity'
    let func_type = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "identity")
        .expect("should have a function type for 'identity'");

    // Should have typeParameters with one entry
    let type_params = func_type["typeParameters"]
        .as_array()
        .expect("typeParameters should be an array");
    assert_eq!(
        type_params.len(),
        1,
        "identity[T] should have 1 type parameter"
    );

    // The type parameter should point to a TypeVar named T
    let tv_id = type_params[0].to_string();
    let tv_type = &types[&tv_id];
    assert_eq!(tv_type["kind"], "typeVar");
    assert_eq!(tv_type["name"], "T");
}

#[test]
fn test_generic_class_type_parameters() {
    let dir = create_test_project(&[("gc.py", "class Box[T]:\n    value: T\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("gc.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the class literal for 'Box'
    let class_type = types
        .values()
        .find(|t| t["kind"] == "classLiteral" && t["className"] == "Box")
        .expect("should have a classLiteral for 'Box'");

    // Should have typeParameters with one entry
    let type_params = class_type["typeParameters"]
        .as_array()
        .expect("typeParameters should be an array");
    assert_eq!(type_params.len(), 1, "Box[T] should have 1 type parameter");

    // The type parameter should point to a TypeVar named T
    let tv_id = type_params[0].to_string();
    let tv_type = &types[&tv_id];
    assert_eq!(tv_type["kind"], "typeVar");
    assert_eq!(tv_type["name"], "T");
}

#[test]
fn test_non_generic_function_no_type_parameters() {
    let dir = create_test_project(&[("ng.py", "def add(a: int, b: int) -> int: return a + b\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("ng.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the function type for 'add'
    let func_type = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "add")
        .expect("should have a function type for 'add'");

    // typeParameters should be absent (skip_serializing_if = "Vec::is_empty")
    assert!(
        func_type.get("typeParameters").is_none(),
        "non-generic function should not have typeParameters key"
    );
}

#[test]
fn test_generic_call_type_arguments() {
    let dir = create_test_project(&[(
        "g.py",
        "def identity[T](x: T) -> T: return x\nresult = identity(42)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("g.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let nodes: Vec<NodeInfo> = serde_json::from_value(result["nodes"].clone()).unwrap();
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the ExprCall node for identity(42)
    let call_node = nodes
        .iter()
        .find(|n| n.node_kind == "ExprCall")
        .expect("should have an ExprCall node");

    let call_sig = call_node
        .call_signature
        .as_ref()
        .expect("ExprCall should have a call signature");

    // Should have one type argument (T resolved to int)
    assert_eq!(
        call_sig.type_arguments.len(),
        1,
        "identity(42) should have 1 type argument, got {:?}",
        call_sig.type_arguments
    );

    // The type argument should be Literal[42] or int
    let ta_id = call_sig.type_arguments[0].to_string();
    let ta_type = &types[&ta_id];
    assert!(
        ta_type["kind"] == "intLiteral" || ta_type["kind"] == "instance",
        "type argument should be int-like, got {:?}",
        ta_type
    );
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

#[test]
fn test_parameter_default_type() {
    let dir = create_test_project(&[("d.py", "def f(x: int = 42): pass\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("d.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the function type for 'f'
    let func_type = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "f")
        .expect("should have a function type for 'f'");

    let params = func_type["parameters"].as_array().unwrap();
    let x_param = params
        .iter()
        .find(|p| p["name"] == "x")
        .expect("should have parameter 'x'");

    assert_eq!(x_param["hasDefault"], true);
    let default_id = x_param["defaultTypeId"]
        .as_u64()
        .expect("x should have a defaultTypeId");

    // The default type should be Literal[42]
    let default_type = &types[&default_id.to_string()];
    assert_eq!(default_type["kind"], "intLiteral");
    assert_eq!(default_type["value"], 42);
}

#[test]
fn test_specialized_return_type() {
    let dir = create_test_project(&[(
        "g.py",
        "def identity[T](x: T) -> T: return x\nresult = identity(42)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("g.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let nodes: Vec<NodeInfo> = serde_json::from_value(result["nodes"].clone()).unwrap();
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the ExprCall node for identity(42)
    let call_node = nodes
        .iter()
        .find(|n| n.node_kind == "ExprCall")
        .expect("should have an ExprCall node");

    let call_sig = call_node
        .call_signature
        .as_ref()
        .expect("ExprCall should have a call signature");

    // The return type should be specialized to Literal[42], not the generic T
    let ret_id = call_sig
        .return_type_id
        .expect("call signature should have a return type");
    let ret_type = &types[&ret_id.to_string()];
    assert_eq!(
        ret_type["kind"], "intLiteral",
        "specialized return type should be intLiteral, got {:?}",
        ret_type
    );
    assert_eq!(ret_type["value"], 42);
}

#[test]
fn test_instance_supertypes() {
    let dir = create_test_project(&[(
        "inh.py",
        "class Animal: pass\nclass Dog(Animal): pass\nd = Dog()\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("inh.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the Instance type for Dog (the type of `d`)
    let dog_instance = types
        .values()
        .find(|t| t["kind"] == "instance" && t["className"] == "Dog")
        .expect("should have an Instance for Dog");

    let supertypes = dog_instance["supertypes"]
        .as_array()
        .expect("Dog instance should have supertypes");
    assert!(
        !supertypes.is_empty(),
        "Dog instance should have at least one supertype"
    );

    // One of the supertypes should resolve to type[Animal]
    let has_animal_supertype = supertypes.iter().any(|st_id| {
        let st = &types[&st_id.to_string()];
        st["className"] == "Animal"
    });
    assert!(
        has_animal_supertype,
        "Dog's supertypes should include Animal, got: {:?}",
        supertypes
            .iter()
            .map(|id| &types[&id.to_string()])
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_module_names() {
    let dir = create_test_project(&[
        (
            "mymodule.py",
            "class MyClass: pass\ndef my_func() -> int: return 1\n",
        ),
        (
            "main.py",
            "from mymodule import MyClass, my_func\nx = MyClass()\ny = my_func()\n",
        ),
    ]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("mymodule.py", 2),
        &get_types_request("main.py", 3),
        &get_type_registry_request(4),
        &shutdown_request(99),
    ]);

    let registry: TypeMap =
        serde_json::from_value(responses[3]["result"]["types"].clone()).unwrap();

    // ClassLiteral for MyClass should have module_name "mymodule"
    let class_type = registry
        .values()
        .find(|t| t["kind"] == "classLiteral" && t["className"] == "MyClass")
        .expect("should have classLiteral for MyClass");
    assert_eq!(
        class_type["moduleName"],
        "mymodule",
        "MyClass classLiteral should have moduleName 'mymodule', got {:?}",
        class_type.get("moduleName")
    );

    // Function for my_func should have module_name "mymodule"
    let func_type = registry
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "my_func")
        .expect("should have function for my_func");
    assert_eq!(
        func_type["moduleName"],
        "mymodule",
        "my_func should have moduleName 'mymodule', got {:?}",
        func_type.get("moduleName")
    );

    // Instance of MyClass should have module_name "mymodule"
    let instance_type = registry
        .values()
        .find(|t| t["kind"] == "instance" && t["className"] == "MyClass")
        .expect("should have instance for MyClass");
    assert_eq!(
        instance_type["moduleName"],
        "mymodule",
        "MyClass instance should have moduleName 'mymodule', got {:?}",
        instance_type.get("moduleName")
    );
}

#[test]
fn test_typevar_variance_covariant() {
    let dir = create_test_project(&[(
        "v.py",
        "from typing import TypeVar\nT_co = TypeVar('T_co', covariant=True)\nclass Box[T_co]: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("v.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find a TypeVar — any T_co should have a variance field
    let tv = types
        .values()
        .find(|t| t["kind"] == "typeVar" && t["name"] == "T_co");
    if let Some(tv) = tv {
        assert!(
            tv.get("variance").is_some(),
            "typeVar should have a variance field, got {:?}",
            tv
        );
    }
}

#[test]
fn test_typevar_variance_on_pep695() {
    let dir = create_test_project(&[("vp.py", "def identity[T](x: T) -> T: return x\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("vp.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let tv = types
        .values()
        .find(|t| t["kind"] == "typeVar" && t["name"] == "T")
        .expect("should have a TypeVar T");

    // PEP 695 typevars should have an inferred variance
    assert!(
        tv.get("variance").is_some(),
        "PEP 695 typeVar should have variance, got {:?}",
        tv
    );
    let variance = tv["variance"].as_str().unwrap();
    assert!(
        ["covariant", "contravariant", "invariant"].contains(&variance),
        "variance should be a valid value, got {:?}",
        variance
    );
}

#[test]
fn test_typevar_with_upper_bound() {
    let dir = create_test_project(&[(
        "b.py",
        "from typing import TypeVar\nT = TypeVar('T', bound=int)\ndef f(x: T) -> T: return x\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("b.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the TypeVar T used as a type parameter of function f
    let func_type = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "f")
        .expect("should have function f");
    let type_params = func_type["typeParameters"]
        .as_array()
        .expect("should have typeParameters");
    assert_eq!(type_params.len(), 1);

    let tv_id = type_params[0].to_string();
    let tv = &types[&tv_id];
    assert_eq!(tv["kind"], "typeVar");
    assert_eq!(tv["name"], "T");

    // Should have an upperBound pointing to int
    assert!(
        tv.get("upperBound").is_some(),
        "bounded TypeVar should have upperBound, got {:?}",
        tv
    );
    let bound_id = tv["upperBound"].to_string();
    let bound_type = &types[&bound_id];
    assert_eq!(
        bound_type["className"], "int",
        "upper bound should be int, got {:?}",
        bound_type
    );
}

#[test]
fn test_typevar_with_constraints() {
    let dir = create_test_project(&[(
        "c.py",
        "from typing import TypeVar\nT = TypeVar('T', int, str)\ndef f(x: T) -> T: return x\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("c.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Find the TypeVar T used as a type parameter of function f
    let func_type = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "f")
        .expect("should have function f");
    let type_params = func_type["typeParameters"]
        .as_array()
        .expect("should have typeParameters");
    assert_eq!(type_params.len(), 1);

    let tv_id = type_params[0].to_string();
    let tv = &types[&tv_id];
    assert_eq!(tv["kind"], "typeVar");
    assert_eq!(tv["name"], "T");

    // Should have constraints with two entries (int and str)
    let constraints = tv["constraints"]
        .as_array()
        .expect("constrained TypeVar should have constraints array");
    assert_eq!(
        constraints.len(),
        2,
        "TypeVar(T, int, str) should have 2 constraints, got {:?}",
        constraints
    );

    // Verify the constraint types are int and str
    let constraint_names: Vec<&str> = constraints
        .iter()
        .filter_map(|c| {
            let cid = c.to_string();
            types[&cid]["className"].as_str()
        })
        .collect();
    assert!(
        constraint_names.contains(&"int") && constraint_names.contains(&"str"),
        "constraints should include int and str, got {:?}",
        constraint_names
    );
}

#[test]
fn test_typevar_no_bounds_no_constraints() {
    let dir = create_test_project(&[("nb.py", "def identity[T](x: T) -> T: return x\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("nb.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let tv = types
        .values()
        .find(|t| t["kind"] == "typeVar" && t["name"] == "T")
        .expect("should have TypeVar T");

    // Unbounded TypeVar should not have upperBound or constraints keys
    assert!(
        tv.get("upperBound").is_none(),
        "unbounded TypeVar should not have upperBound, got {:?}",
        tv
    );
    assert!(
        tv.get("constraints").is_none(),
        "unconstrained TypeVar should not have constraints key, got {:?}",
        tv
    );
}
