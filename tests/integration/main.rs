#[allow(dead_code)]
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
fn test_type_form() {
    // PEP 747 `TypeForm[T]` value — exercises the `typeForm` descriptor and its
    // `typeArgument` cross-reference into the registry.
    let dir = create_test_project(&[(
        "tf.py",
        "from typing_extensions import TypeForm\n\
         string_form: TypeForm[str] = str\n\
         reveal_type(string_form)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("tf.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Should have a `TypeForm[str]` type-form descriptor.
    let type_form = types
        .values()
        .find(|t| t["kind"] == "typeForm" && t["display"].as_str() == Some("TypeForm[str]"))
        .expect("should have a 'TypeForm[str]' typeForm type");

    // Its `typeArgument` should reference the `str` type in the registry.
    let arg_id = type_form["typeArgument"]
        .as_u64()
        .expect("typeForm should have a typeArgument id");
    let arg = &types[&arg_id.to_string()];
    assert_eq!(
        arg["display"], "str",
        "typeArgument should resolve to 'str'"
    );
}

#[test]
fn test_typed_dict_extra_items() {
    // PEP 728 `extra_items=` — exercises the `typedDict` descriptor's `extraItems`
    // field and its `typeId` cross-reference into the registry.
    let dir = create_test_project(&[(
        "td.py",
        "from typing_extensions import TypedDict\n\
         class Movie(TypedDict, extra_items=int):\n\
         \x20   name: str\n\
         m: Movie = {\"name\": \"Blade Runner\"}\n\
         reveal_type(m)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("td.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let movie = types
        .values()
        .find(|t| t["kind"] == "typedDict" && t["name"] == "Movie")
        .expect("should have a 'Movie' typedDict type");

    // Not closed, and exposes extra items of type `int` that are mutable.
    assert_ne!(
        movie["closed"],
        serde_json::json!(true),
        "Movie is not closed"
    );
    let extra = &movie["extraItems"];
    assert_eq!(extra["readOnly"], false, "extra_items=int is mutable");
    let type_id = extra["typeId"]
        .as_u64()
        .expect("extraItems should carry a typeId");
    assert_eq!(
        types[&type_id.to_string()]["display"],
        "int",
        "extraItems typeId should resolve to 'int'"
    );
}

#[test]
fn test_typed_dict_closed() {
    // PEP 728 `closed=True` — the `typedDict` descriptor should report `closed: true`
    // and carry no `extraItems`.
    let dir = create_test_project(&[(
        "tdc.py",
        "from typing_extensions import TypedDict\n\
         class Sealed(TypedDict, closed=True):\n\
         \x20   name: str\n\
         s: Sealed = {\"name\": \"x\"}\n\
         reveal_type(s)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("tdc.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let sealed = types
        .values()
        .find(|t| t["kind"] == "typedDict" && t["name"] == "Sealed")
        .expect("should have a 'Sealed' typedDict type");

    assert_eq!(sealed["closed"], true, "Sealed is closed");
    assert!(
        sealed["extraItems"].is_null(),
        "closed TypedDict has no extraItems"
    );
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

#[test]
fn test_param_spec_signature() {
    // A callable typed `Callable[P, R]` — the signature's *args/**kwargs
    // stand in for the ParamSpec tail.
    let dir = create_test_project(&[(
        "ps.py",
        "from typing import Callable, ParamSpec, TypeVar\n\
         P = ParamSpec(\"P\")\n\
         R = TypeVar(\"R\")\n\
         def run(cb: Callable[P, R]) -> R: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("ps.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let run_fn = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "run")
        .expect("should have function 'run'");
    let cb_param = run_fn["parameters"]
        .as_array()
        .expect("parameters array")
        .iter()
        .find(|p| p["name"] == "cb")
        .expect("should have 'cb' parameter");
    let cb_type_id = cb_param["typeId"].as_u64().expect("cb typeId") as u32;
    let cb_type = types
        .get(&cb_type_id.to_string())
        .expect("cb type descriptor");
    assert_eq!(cb_type["kind"], "callable");

    let params = cb_type["parameters"]
        .as_array()
        .expect("callable parameters array");

    let ps_params: Vec<&serde_json::Value> = params
        .iter()
        .filter(|p| p.get("paramSpecName").is_some())
        .collect();
    assert_eq!(
        ps_params.len(),
        2,
        "expected 2 ParamSpec-backed params (*args, **kwargs), got {:?}",
        params
    );
    for p in &ps_params {
        assert_eq!(p["paramSpecName"], "P");
        assert!(
            p.get("concatenatePrefix").is_none(),
            "pure ParamSpec signature should not mark params as concatenatePrefix, got {:?}",
            p
        );
    }

    assert!(
        !params
            .iter()
            .any(|p| p.get("concatenatePrefix") == Some(&serde_json::Value::Bool(true))),
        "pure ParamSpec signature should not have any concatenatePrefix params, got {:?}",
        params
    );
}

#[test]
fn test_concatenate_signature() {
    // `Callable[Concatenate[int, P], R]` — leading positional params should be
    // marked as concatenatePrefix, and the synthesized *args/**kwargs should
    // carry the ParamSpec name.
    let dir = create_test_project(&[(
        "cc.py",
        "from typing import Callable, Concatenate, ParamSpec, TypeVar\n\
         P = ParamSpec(\"P\")\n\
         R = TypeVar(\"R\")\n\
         def run(cb: Callable[Concatenate[int, P], R]) -> R: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("cc.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let run_fn = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "run")
        .expect("should have function 'run'");
    let cb_param = run_fn["parameters"]
        .as_array()
        .expect("parameters array")
        .iter()
        .find(|p| p["name"] == "cb")
        .expect("should have 'cb' parameter");
    let cb_type_id = cb_param["typeId"].as_u64().expect("cb typeId") as u32;
    let cb_type = types
        .get(&cb_type_id.to_string())
        .expect("cb type descriptor");
    assert_eq!(cb_type["kind"], "callable");

    let params = cb_type["parameters"]
        .as_array()
        .expect("callable parameters array");

    let prefix_params: Vec<&serde_json::Value> = params
        .iter()
        .filter(|p| p.get("concatenatePrefix") == Some(&serde_json::Value::Bool(true)))
        .collect();
    assert_eq!(
        prefix_params.len(),
        1,
        "expected 1 concatenate prefix param, got {:?}",
        params
    );

    let ps_params: Vec<&serde_json::Value> = params
        .iter()
        .filter(|p| p.get("paramSpecName") == Some(&serde_json::json!("P")))
        .collect();
    assert_eq!(
        ps_params.len(),
        2,
        "expected 2 ParamSpec-backed params (*args, **kwargs), got {:?}",
        params
    );
    for p in &ps_params {
        assert!(
            p.get("concatenatePrefix").is_none(),
            "variadic ParamSpec params should not be marked as concatenatePrefix, got {:?}",
            p
        );
    }
}

#[test]
fn test_non_paramspec_signature_has_no_flags() {
    // A plain signature should not emit concatenatePrefix or paramSpecName on any parameter.
    let dir =
        create_test_project(&[("plain.py", "def add(a: int, b: int) -> int: return a + b\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("plain.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let add_fn = types
        .values()
        .find(|t| t["kind"] == "function" && t["name"] == "add")
        .expect("should have function 'add'");
    for p in add_fn["parameters"].as_array().unwrap() {
        assert!(
            p.get("concatenatePrefix").is_none(),
            "plain signature param should not have concatenatePrefix: {:?}",
            p
        );
        assert!(
            p.get("paramSpecName").is_none(),
            "plain signature param should not have paramSpecName: {:?}",
            p
        );
    }
}

#[test]
fn test_typevar_tuple_kind() {
    // Both the legacy `TypeVarTuple("Ts")` and PEP 695 `[*Us]` spellings
    // collapse to the same typevarKind.
    let dir = create_test_project(&[(
        "vt.py",
        "from typing import TypeVarTuple, Unpack\n\
         Ts = TypeVarTuple(\"Ts\")\n\
         def legacy(*args: Unpack[Ts]) -> tuple[Unpack[Ts]]: ...\n\
         def pep695[*Us](*args: *Us) -> tuple[*Us]: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("vt.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    for name in ["Ts", "Us"] {
        let tv = types
            .values()
            .find(|t| t["kind"] == "typeVar" && t["name"] == name)
            .unwrap_or_else(|| panic!("should have typevar '{name}'"));
        assert_eq!(
            tv["typevarKind"], "TypeVarTuple",
            "typevar '{name}' should be a TypeVarTuple, got {tv:?}"
        );
    }
}

#[test]
fn test_subclass_of_protocol() {
    // `type[SomeProtocol]` reports the protocol's own class as its base,
    // and never the subclassOf descriptor itself.
    let dir = create_test_project(&[(
        "sp.py",
        "from typing import Protocol\n\
         class Proto(Protocol):\n\
         \x20   def meth(self) -> int: ...\n\
         def takes(c: type[Proto]) -> None: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("sp.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let (id, subclass_of) = types
        .iter()
        .find(|(_, t)| t["kind"] == "subclassOf" && t["display"] == "type[Proto]")
        .expect("should have subclassOf 'type[Proto]'");
    let base_id = subclass_of["base"].as_u64().expect("base") as u32;
    assert_ne!(
        &base_id.to_string(),
        id,
        "subclassOf base must not point at itself"
    );

    let base = types.get(&base_id.to_string()).expect("base descriptor");
    assert_eq!(base["kind"], "classLiteral");
    assert_eq!(base["className"], "Proto");
}

#[test]
fn test_known_instance_kind_and_payloads() {
    // knownInstanceKind distinguishes singletons that share a class, and the
    // functools.partial / range variants carry their payloads.
    let dir = create_test_project(&[(
        "ki.py",
        "import functools\n\
         def base(a: int, b: str, c: float) -> bytes: ...\n\
         p = functools.partial(base, 1)\n\
         non_empty = range(1, 10)\n\
         empty = range(0)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("ki.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let partial = types
        .values()
        .find(|t| t["kind"] == "knownInstance" && t["knownInstanceKind"] == "FunctoolsPartial")
        .expect("should have a FunctoolsPartial knownInstance");
    assert_eq!(partial["className"], "partial");
    assert!(
        partial.get("wrappedType").is_some(),
        "partial should carry the wrapped callable: {partial:?}"
    );
    // `base` takes three parameters; binding `1` leaves `b` and `c`.
    let params = partial["parameters"].as_array().expect("parameters array");
    assert_eq!(
        params
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["b", "c"]
    );

    let emptiness: Vec<bool> = types
        .values()
        .filter(|t| t["kind"] == "knownInstance" && t["knownInstanceKind"] == "Range")
        .filter_map(|t| t["isNonEmpty"].as_bool())
        .collect();
    assert!(
        emptiness.contains(&true) && emptiness.contains(&false),
        "expected both a non-empty and an empty range, got {emptiness:?}"
    );
}

#[test]
fn test_type_alias_qualified_name() {
    // A type alias reports the dotted path of its enclosing modules and classes.
    let dir = create_test_project(&[(
        "ta.py",
        "type Top[T] = list[T]\n\
         class Outer:\n\
         \x20   type Nested = set[int]\n\
         def uses(a: Top[int], b: Outer.Nested) -> None: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("ta.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let find = |name: &str| {
        types
            .values()
            .find(|t| t["kind"] == "typeAlias" && t["name"] == name)
            .unwrap_or_else(|| panic!("should have type alias '{name}'"))
            .clone()
    };

    assert_eq!(find("Top")["qualifiedName"], "ta.Top");
    assert_eq!(find("Nested")["qualifiedName"], "ta.Outer.Nested");
}

#[test]
fn test_nested_class_qualified_name() {
    // `moduleName` plus `className` would call `nc.Outer.Inner` just `nc.Inner`.
    let dir = create_test_project(&[(
        "nc.py",
        "class Outer:\n\
         \x20   class Inner: ...\n\
         class Top: ...\n\
         def f(a: Outer.Inner, b: Top) -> type[Outer.Inner]: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("nc.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let find = |kind: &str, name: &str| {
        types
            .values()
            .find(|t| t["kind"] == kind && t["className"] == name)
            .unwrap_or_else(|| panic!("should have {kind} '{name}'"))
            .clone()
    };

    assert_eq!(find("instance", "Inner")["qualifiedName"], "nc.Outer.Inner");
    assert_eq!(
        find("classLiteral", "Inner")["qualifiedName"],
        "nc.Outer.Inner"
    );
    // A top-level class keeps the plain `module.Class` form.
    assert_eq!(find("instance", "Top")["qualifiedName"], "nc.Top");
    assert_eq!(find("classLiteral", "Top")["qualifiedName"], "nc.Top");
}

#[test]
fn test_enum_qualified_name() {
    // Each enum needs two or more members: ty collapses a single-member enum to
    // its instance type, which yields no `enumLiteral` at all.
    let dir = create_test_project(&[(
        "en.py",
        "import enum\n\
         class Color(enum.Enum):\n\
         \x20   RED = 1\n\
         \x20   GREEN = 2\n\
         class Outer:\n\
         \x20   class Shade(enum.Enum):\n\
         \x20       DARK = 1\n\
         \x20       LIGHT = 2\n\
         r = Color.RED\n\
         d = Outer.Shade.DARK\n\
         def narrow(x: Color) -> None:\n\
         \x20   if x is not Color.RED:\n\
         \x20       reveal_type(x)\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("en.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let find = |kind: &str, name: &str| {
        types
            .values()
            .find(|t| t["kind"] == kind && t["className"] == name)
            .unwrap_or_else(|| panic!("should have {kind} '{name}'"))
            .clone()
    };

    assert_eq!(find("enumLiteral", "Color")["qualifiedName"], "en.Color");
    assert_eq!(
        find("enumLiteral", "Shade")["qualifiedName"],
        "en.Outer.Shade"
    );

    let complement = find("enumComplement", "Color");
    assert_eq!(complement["moduleName"], "en");
    assert_eq!(complement["qualifiedName"], "en.Color");
}

#[test]
fn test_typed_dict_qualified_name() {
    let dir = create_test_project(&[(
        "tdq.py",
        "from typing import TypedDict\n\
         class Movie(TypedDict):\n\
         \x20   name: str\n\
         class Outer:\n\
         \x20   class Book(TypedDict):\n\
         \x20       title: str\n\
         def f(a: Movie, b: Outer.Book) -> None: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("tdq.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let find = |name: &str| {
        types
            .values()
            .find(|t| t["kind"] == "typedDict" && t["name"] == name)
            .unwrap_or_else(|| panic!("should have typedDict '{name}'"))
            .clone()
    };

    assert_eq!(find("Movie")["qualifiedName"], "tdq.Movie");
    assert_eq!(find("Book")["qualifiedName"], "tdq.Outer.Book");
}

#[test]
fn test_same_named_typed_dicts_in_different_modules_stay_distinct() {
    let dir = create_test_project(&[
        (
            "a.py",
            "from typing import TypedDict\n\
             class Movie(TypedDict):\n\
             \x20   name: str\n",
        ),
        (
            "b.py",
            "from typing import TypedDict\n\
             class Movie(TypedDict):\n\
             \x20   title: str\n",
        ),
        (
            "uses.py",
            "import a\n\
             import b\n\
             def f(x: a.Movie, y: b.Movie) -> None: ...\n",
        ),
    ]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("uses.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let mut qualified: Vec<&str> = types
        .values()
        .filter(|t| t["kind"] == "typedDict" && t["name"] == "Movie")
        .map(|t| t["qualifiedName"].as_str().expect("qualifiedName"))
        .collect();
    qualified.sort_unstable();

    assert_eq!(qualified, vec!["a.Movie", "b.Movie"]);
}

#[test]
fn test_new_type_qualified_name() {
    let dir = create_test_project(&[(
        "nt.py",
        "from typing import NewType\n\
         UserId = NewType(\"UserId\", int)\n\
         class Outer:\n\
         \x20   Tag = NewType(\"Tag\", str)\n\
         u = UserId(1)\n\
         t = Outer.Tag(\"x\")\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("nt.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let find = |name: &str| {
        types
            .values()
            .find(|t| t["kind"] == "newType" && t["name"] == name)
            .unwrap_or_else(|| panic!("should have newType '{name}'"))
            .clone()
    };

    assert_eq!(find("UserId")["qualifiedName"], "nt.UserId");
    assert_eq!(find("Tag")["qualifiedName"], "nt.Outer.Tag");
}

#[test]
fn test_tuple_elements() {
    // Each tuple shape: fixed-length, homogeneous, unpacked TypeVarTuple, empty.
    let dir = create_test_project(&[(
        "tup.py",
        "from typing import TypeVarTuple\n\
         Ts = TypeVarTuple(\"Ts\")\n\
         def fixed() -> tuple[int, str]: ...\n\
         def homogeneous() -> tuple[int, ...]: ...\n\
         def mixed(*args: *Ts) -> tuple[int, *Ts, bool]: ...\n\
         def empty() -> tuple[()]: ...\n",
    )]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("tup.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    // Returns (kind, className) per element, resolving each element's type id.
    let elements_of = |display: &str| -> Vec<(String, String)> {
        let tuple = types
            .values()
            .find(|t| t["kind"] == "instance" && t["display"] == display)
            .unwrap_or_else(|| panic!("should have instance '{display}'"));
        tuple["tupleElements"]
            .as_array()
            .unwrap_or_else(|| panic!("'{display}' should have tupleElements: {tuple:?}"))
            .iter()
            .map(|e| {
                let id = e["typeId"].as_u64().expect("typeId") as u32;
                let referenced = types.get(&id.to_string()).expect("element descriptor");
                let name = referenced["className"]
                    .as_str()
                    .or_else(|| referenced["name"].as_str())
                    .unwrap_or_default();
                (
                    e["kind"].as_str().expect("kind").to_string(),
                    name.to_string(),
                )
            })
            .collect()
    };

    assert_eq!(
        elements_of("tuple[int, str]"),
        vec![
            ("fixed".to_string(), "int".to_string()),
            ("fixed".to_string(), "str".to_string()),
        ]
    );
    assert_eq!(
        elements_of("tuple[int, ...]"),
        vec![("homogeneous".to_string(), "int".to_string())]
    );
    assert_eq!(
        elements_of("tuple[int, *Ts@mixed, bool]"),
        vec![
            ("fixed".to_string(), "int".to_string()),
            ("typeVarTuple".to_string(), "Ts".to_string()),
            ("fixed".to_string(), "bool".to_string()),
        ]
    );

    // Empty list, not absent: absence is reserved for non-tuples.
    let empty = types
        .values()
        .find(|t| t["kind"] == "instance" && t["display"] == "tuple[()]")
        .expect("should have instance 'tuple[()]'");
    assert_eq!(empty["tupleElements"].as_array().map(Vec::len), Some(0));
}

#[test]
fn test_non_tuple_instance_has_no_tuple_elements() {
    let dir = create_test_project(&[("nt.py", "def f() -> list[int]: ...\n")]);

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("nt.py", 2),
        &shutdown_request(99),
    ]);

    let result = &responses[1]["result"];
    let types: TypeMap = serde_json::from_value(result["types"].clone()).unwrap();

    let list = types
        .values()
        .find(|t| t["kind"] == "instance" && t["display"] == "list[int]")
        .expect("should have instance 'list[int]'");
    assert!(
        list.get("tupleElements").is_none(),
        "non-tuple instance should omit tupleElements: {list:?}"
    );
}

/// Fixture: a package whose `__init__.py` re-exports constants defined in a
/// submodule, and a consumer reaching them by alias and by module attribute.
fn reexported_constants_project() -> tempfile::TempDir {
    create_test_project(&[
        ("pkg/_impl.py", "SEP: str = \"/\"\nLIMIT = 5\n"),
        ("pkg/__init__.py", "from pkg._impl import SEP, LIMIT\n"),
        (
            "main.py",
            "import pkg\nfrom pkg import SEP as SEPARATOR\n\nprint(SEPARATOR)\nprint(pkg.LIMIT)\n",
        ),
    ])
}

fn get_types_with_bindings_request(file: &str, id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "getTypes",
        "params": {"file": file, "includeBindings": true},
        "id": id
    })
    .to_string()
}

/// Returns the `binding` of the one node of `node_kind` spanning exactly `text`.
fn binding_at(
    nodes: &[serde_json::Value],
    source: &str,
    node_kind: &str,
    text: &str,
) -> serde_json::Value {
    let matches: Vec<&serde_json::Value> = nodes
        .iter()
        .filter(|n| {
            n["nodeKind"] == node_kind
                && &source
                    [n["start"].as_u64().unwrap() as usize..n["end"].as_u64().unwrap() as usize]
                    == text
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one {node_kind} node spanning {text:?}, got {}",
        matches.len()
    );
    matches[0]["binding"].clone()
}

#[test]
fn test_binding_follows_reexport_chain_for_name_and_attribute_references() {
    let dir = reexported_constants_project();
    let source = std::fs::read_to_string(dir.path().join("main.py")).unwrap();

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_with_bindings_request("main.py", 2),
        &shutdown_request(99),
    ]);
    let nodes = responses[1]["result"]["nodes"].as_array().unwrap().clone();

    // An annotated assignment: its value type reports `builtins`, and its name
    // needs the fallback in `definition_name_of`.
    assert_eq!(
        binding_at(&nodes, &source, "ExprName", "SEPARATOR"),
        serde_json::json!({"definedIn": "pkg._impl", "qualifiedName": "pkg._impl.SEP"}),
    );

    // Reached as a module attribute rather than an imported name.
    assert_eq!(
        binding_at(&nodes, &source, "ExprAttribute", "pkg.LIMIT"),
        serde_json::json!({"definedIn": "pkg._impl", "qualifiedName": "pkg._impl.LIMIT"}),
    );
}

#[test]
fn test_binding_qualified_name_carries_the_enclosing_class() {
    let dir = create_test_project(&[(
        "cls.py",
        "class Holder:\n    MAX = 10\n\n\nprint(Holder.MAX)\n",
    )]);
    let source = std::fs::read_to_string(dir.path().join("cls.py")).unwrap();

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_with_bindings_request("cls.py", 2),
        &shutdown_request(99),
    ]);
    let nodes = responses[1]["result"]["nodes"].as_array().unwrap().clone();

    assert_eq!(
        binding_at(&nodes, &source, "ExprAttribute", "Holder.MAX"),
        serde_json::json!({"definedIn": "cls", "qualifiedName": "cls.Holder.MAX"}),
    );
}

#[test]
fn test_binding_picks_the_branch_matching_the_inferred_type() {
    let dir = create_test_project(&[
        ("pkg/_win.py", "TIMEOUT = 60\n"),
        ("pkg/_posix.py", "TIMEOUT = 30\n"),
        (
            "pkg/__init__.py",
            "import sys\n\nif sys.platform == \"win32\":\n    from pkg._win import TIMEOUT\nelse:\n    from pkg._posix import TIMEOUT\n",
        ),
        ("main.py", "from pkg import TIMEOUT\n\nprint(TIMEOUT)\n"),
    ]);
    let source = std::fs::read_to_string(dir.path().join("main.py")).unwrap();

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_with_bindings_request("main.py", 2),
        &shutdown_request(99),
    ]);
    let nodes = responses[1]["result"]["nodes"].as_array().unwrap().clone();

    // `_win` is the first branch in source order; the inferred `Literal[30]` is
    // what makes `_posix` the right answer.
    assert_eq!(
        binding_at(&nodes, &source, "ExprName", "TIMEOUT"),
        serde_json::json!({"definedIn": "pkg._posix", "qualifiedName": "pkg._posix.TIMEOUT"}),
    );
}

#[test]
fn test_binding_survives_a_symbol_that_is_declared_before_it_is_bound() {
    let dir = create_test_project(&[
        ("cfg.py", "COUNT: int\nCOUNT = 3\n"),
        ("main.py", "from cfg import COUNT\n\nprint(COUNT)\n"),
    ]);
    let source = std::fs::read_to_string(dir.path().join("main.py")).unwrap();

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_with_bindings_request("main.py", 2),
        &shutdown_request(99),
    ]);
    let nodes = responses[1]["result"]["nodes"].as_array().unwrap().clone();

    // `COUNT: int` declares without binding, and asking ty for its binding type
    // aborts the process rather than returning.
    assert_eq!(
        binding_at(&nodes, &source, "ExprName", "COUNT"),
        serde_json::json!({"definedIn": "cfg", "qualifiedName": "cfg.COUNT"}),
    );
}

#[test]
fn test_bindings_are_omitted_unless_requested() {
    let dir = reexported_constants_project();

    let responses = run_session(&[
        &initialize_request(dir.path().to_str().unwrap(), 1),
        &get_types_request("main.py", 2),
        &shutdown_request(99),
    ]);

    let nodes = responses[1]["result"]["nodes"].as_array().unwrap();
    assert!(
        nodes.iter().all(|n| n.get("binding").is_none()),
        "binding should be absent without includeBindings"
    );
}
