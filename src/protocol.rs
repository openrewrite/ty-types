use serde::{Deserialize, Serialize};
use std::borrow::Cow;
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
#[serde(rename_all = "camelCase")]
pub struct GetTypesParams {
    pub file: String,
    #[serde(default = "default_true")]
    pub include_display: bool,
}

fn default_true() -> bool {
    true
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
    pub node_kind: Cow<'static, str>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub type_arguments: Vec<TypeId>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_id: Option<TypeId>,
    pub kind: &'static str,
    pub has_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_type_id: Option<TypeId>,
}

// ─── Structured type details ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassMemberInfo {
    pub name: String,
    pub type_id: TypeId,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedDictFieldInfo {
    pub name: String,
    pub type_id: TypeId,
    pub required: bool,
    pub read_only: bool,
}

// ─── Structured type descriptors ─────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TypeDescriptor {
    // Instance types
    #[serde(rename_all = "camelCase")]
    Instance {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        supertypes: Vec<TypeId>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_args: Vec<TypeId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        class_id: Option<TypeId>,
    },

    // Class literal: type[MyClass]
    #[serde(rename_all = "camelCase")]
    ClassLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_parameters: Vec<TypeId>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        supertypes: Vec<TypeId>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        members: Vec<ClassMemberInfo>,
    },

    // type[C] — subclass-of
    #[serde(rename_all = "camelCase")]
    SubclassOf {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        base: TypeId,
    },

    // Composite types
    Union {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        members: Vec<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    Intersection {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        positive: Vec<TypeId>,
        negative: Vec<TypeId>,
    },

    // Callables
    #[serde(rename_all = "camelCase")]
    Function {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_parameters: Vec<TypeId>,
        parameters: Vec<ParameterInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_type: Option<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    Callable {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    #[serde(rename_all = "camelCase")]
    BoundMethod {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_parameters: Vec<TypeId>,
        parameters: Vec<ParameterInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_type: Option<TypeId>,
    },

    // Literals
    #[serde(rename_all = "camelCase")]
    IntLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        value: i64,
    },

    #[serde(rename_all = "camelCase")]
    BoolLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        value: bool,
    },

    #[serde(rename_all = "camelCase")]
    StringLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        value: String,
    },

    #[serde(rename_all = "camelCase")]
    BytesLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        value: String, // display representation, e.g. Literal[b"..."]
    },

    #[serde(rename_all = "camelCase")]
    EnumLiteral {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        member_name: String,
    },

    #[serde(rename_all = "camelCase")]
    LiteralString {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    // Dynamic / special
    #[serde(rename_all = "camelCase")]
    Dynamic {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        dynamic_kind: String,
    },

    Never {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    Truthy {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    Falsy {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    // Type system types
    #[serde(rename_all = "camelCase")]
    TypeVar {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    Module {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        module_name: String,
    },

    #[serde(rename_all = "camelCase")]
    TypeAlias {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    TypedDict {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        fields: Vec<TypedDictFieldInfo>,
    },

    #[serde(rename_all = "camelCase")]
    TypeIs {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        narrowed_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    TypeGuard {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        guarded_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    NewType {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
        base_type: TypeId,
    },

    #[serde(rename_all = "camelCase")]
    SpecialForm {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
    },

    #[serde(rename_all = "camelCase")]
    Property {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },

    // Fallback for internal ty types
    Other {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
}

impl TypeDescriptor {
    /// Set the `display` field to `None`, regardless of variant.
    pub fn strip_display(&mut self) {
        match self {
            Self::Instance { display, .. }
            | Self::ClassLiteral { display, .. }
            | Self::SubclassOf { display, .. }
            | Self::Union { display, .. }
            | Self::Intersection { display, .. }
            | Self::Function { display, .. }
            | Self::Callable { display, .. }
            | Self::BoundMethod { display, .. }
            | Self::IntLiteral { display, .. }
            | Self::BoolLiteral { display, .. }
            | Self::StringLiteral { display, .. }
            | Self::BytesLiteral { display, .. }
            | Self::EnumLiteral { display, .. }
            | Self::LiteralString { display, .. }
            | Self::Dynamic { display, .. }
            | Self::Never { display, .. }
            | Self::Truthy { display, .. }
            | Self::Falsy { display, .. }
            | Self::TypeVar { display, .. }
            | Self::Module { display, .. }
            | Self::TypeAlias { display, .. }
            | Self::TypedDict { display, .. }
            | Self::TypeIs { display, .. }
            | Self::TypeGuard { display, .. }
            | Self::NewType { display, .. }
            | Self::SpecialForm { display, .. }
            | Self::Property { display, .. }
            | Self::Other { display, .. } => {
                *display = None;
            }
        }
    }
}
