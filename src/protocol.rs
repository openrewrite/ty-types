use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── JSON-RPC envelope ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: serde_json::Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError { code, message }),
            id,
        }
    }
}

// ─── Method params ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub project_root: String,
}

#[derive(Debug, Deserialize)]
pub struct GetTypesParams {
    pub file: String,
}

// ─── Response payloads ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct InitializeResult {
    pub ok: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTypesResult {
    pub nodes: Vec<NodeAttribution>,
    pub types: HashMap<TypeId, TypeDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTypeRegistryResult {
    pub types: HashMap<TypeId, TypeDescriptor>,
}

/// CLI one-shot output: nodes grouped by file, shared type registry.
#[derive(Debug, Serialize)]
pub struct CliResult {
    pub files: HashMap<String, Vec<NodeAttribution>>,
    pub types: HashMap<TypeId, TypeDescriptor>,
}

// ─── Node attribution ────────────────────────────────────────────────

pub type TypeId = u32;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeAttribution {
    pub start: u32,
    pub end: u32,
    pub node_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_id: Option<TypeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_signature: Option<CallSignatureInfo>,
}

// ─── Call signature info ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSignatureInfo {
    pub parameters: Vec<ParameterInfo>,
    pub return_type_id: Option<TypeId>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_id: Option<TypeId>,
    pub kind: &'static str,
    pub has_default: bool,
}

// ─── Structured type descriptors ─────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TypeDescriptor {
    // Instance types
    #[serde(rename_all = "camelCase")]
    Instance {
        display: String,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_args: Vec<TypeId>,
    },

    // Class literal: type[MyClass]
    #[serde(rename_all = "camelCase")]
    ClassLiteral {
        display: String,
        class_name: String,
    },

    // type[C] — subclass-of
    #[serde(rename_all = "camelCase")]
    SubclassOf {
        display: String,
        base: TypeId,
    },

    // Composite types
    Union {
        display: String,
        members: Vec<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    Intersection {
        display: String,
        positive: Vec<TypeId>,
        negative: Vec<TypeId>,
    },

    // Callables
    #[serde(rename_all = "camelCase")]
    Function {
        display: String,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    Callable {
        display: String,
    },

    #[serde(rename_all = "camelCase")]
    BoundMethod {
        display: String,
    },

    // Literals
    #[serde(rename_all = "camelCase")]
    IntLiteral {
        display: String,
        value: i64,
    },

    #[serde(rename_all = "camelCase")]
    BoolLiteral {
        display: String,
        value: bool,
    },

    #[serde(rename_all = "camelCase")]
    StringLiteral {
        display: String,
        value: String,
    },

    #[serde(rename_all = "camelCase")]
    BytesLiteral {
        display: String,
        value: String, // hex-encoded
    },

    #[serde(rename_all = "camelCase")]
    EnumLiteral {
        display: String,
        class_name: String,
        member_name: String,
    },

    #[serde(rename_all = "camelCase")]
    LiteralString {
        display: String,
    },

    // Dynamic / special
    #[serde(rename_all = "camelCase")]
    Dynamic {
        display: String,
        dynamic_kind: String,
    },

    Never {
        display: String,
    },

    Truthy {
        display: String,
    },

    Falsy {
        display: String,
    },

    // Type system types
    #[serde(rename_all = "camelCase")]
    TypeVar {
        display: String,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    Module {
        display: String,
        module_name: String,
    },

    #[serde(rename_all = "camelCase")]
    TypeAlias {
        display: String,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    TypedDict {
        display: String,
    },

    #[serde(rename_all = "camelCase")]
    TypeIs {
        display: String,
        narrowed_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    TypeGuard {
        display: String,
        guarded_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    NewType {
        display: String,
        name: String,
        base_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    SpecialForm {
        display: String,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    Property {
        display: String,
    },

    // Fallback for internal ty types
    Other {
        display: String,
    },
}
