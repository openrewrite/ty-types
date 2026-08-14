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
    /// First-party package root. When set, the session's `getTypes` registry emits
    /// classes defined outside this root as `classRef` (identity only).
    #[serde(default)]
    pub first_party_root: Option<String>,
    /// First-party top-level module names. Used when `first_party_root` is absent:
    /// classes outside these modules are emitted as `classRef`.
    #[serde(default)]
    pub first_party_modules: Vec<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLibraryApiParams {
    /// Single root, unioned with `roots`. Accepted for backward compatibility.
    #[serde(default)]
    pub root: Option<String>,
    /// Absolute paths to the roots the distribution installs. Each is either a
    /// package directory or a bare module file, and the boundary spans their union.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Emit modules with an underscore-prefixed path component, which the
    /// convention marks private.
    #[serde(default)]
    pub include_private_modules: bool,
    /// Emit module-level symbols that `__all__` omits, or — with no `__all__` —
    /// underscore-prefixed ones.
    #[serde(default)]
    pub include_non_exported_symbols: bool,
    #[serde(default = "default_true")]
    pub include_display: bool,
}

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySymbolInfo {
    pub name: String,
    pub type_id: TypeId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryModuleInfo {
    /// Dotted module FQN, e.g. "requests.sessions".
    pub name: String,
    /// Module file path relative to its root's parent, e.g. "requests/sessions.py".
    /// Including the root's own name keeps same-named modules from sibling roots
    /// (`mypy/main.py` vs `mypyc/main.py`) distinct.
    pub file: String,
    pub symbols: Vec<LibrarySymbolInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLibraryApiResult {
    pub modules: Vec<LibraryModuleInfo>,
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
    /// Set on leading positional parameters of a `Concatenate[T1, ..., Tn, P]` or
    /// `Concatenate[T1, ..., Tn, ...]` signature.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub concatenate_prefix: bool,
    /// Set on the `*args` / `**kwargs` parameters that stand in for a `ParamSpec` tail,
    /// carrying the name of that `ParamSpec` (e.g. `"P"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param_spec_name: Option<String>,
}

// ─── Structured type details ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TupleElementInfo {
    pub type_id: TypeId,
    /// `fixed` for a single element, `homogeneous` for the `...` segment of
    /// `tuple[int, ...]`, `typeVarTuple` for the `*Ts` of `tuple[int, *Ts]`.
    /// The latter two each stand for an unknown number of elements.
    pub kind: &'static str,
}

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

/// PEP 728 `extra_items=` policy: values of undeclared keys are exposed with this
/// declared type and mutability. Present only when explicitly declared.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypedDictExtraItemsInfo {
    pub type_id: TypeId,
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
        /// Element types in source order, for tuples and their subclasses.
        /// `typeArgs` cannot express these: `tuple` has a single generic
        /// parameter, so it conflates `tuple[int, str]` with `tuple[int | str, ...]`.
        /// An empty list is a fixed-length empty tuple; absent means not a tuple.
        #[serde(skip_serializing_if = "Option::is_none")]
        tuple_elements: Option<Vec<TupleElementInfo>>,
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

    // Reference to a class defined outside the extracted package boundary.
    // Identity only — no members, supertypes, or type parameters. Maps to
    // the V3 type-table TAG_CLASS_REF on the consumer side.
    #[serde(rename_all = "camelCase")]
    ClassRef {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
    },

    // type[C] — subclass-of
    #[serde(rename_all = "camelCase")]
    SubclassOf {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        base: TypeId,
    },

    // TypeForm[T] — PEP 747 type-form value wrapping a type expression
    #[serde(rename_all = "camelCase")]
    TypeForm {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        type_argument: TypeId,
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
        #[serde(skip_serializing_if = "Vec::is_empty")]
        parameters: Vec<ParameterInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_type: Option<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    BoundMethod {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        class_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_parameters: Vec<TypeId>,
        parameters: Vec<ParameterInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_type: Option<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    WrapperDescriptor {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        descriptor_kind: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
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
        /// "TypeVar", "ParamSpec", "TypeVarTuple", "Self"
        #[serde(skip_serializing_if = "Option::is_none")]
        typevar_kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        variance: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        upper_bound: Option<TypeId>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        constraints: Vec<TypeId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default_type: Option<TypeId>,
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
        /// Dotted path of the enclosing modules and classes, followed by `name`
        /// (e.g. `a.b.C.D`).
        #[serde(skip_serializing_if = "Option::is_none")]
        qualified_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        value_type: Option<TypeId>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        type_parameters: Vec<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    KnownInstance {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        /// Canonical module of the known class (e.g. `dataclasses` for
        /// `dataclasses.Field`, `typing` for `typing.Final`). Lets consumers
        /// build the correct fully-qualified name instead of assuming `typing`.
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        /// Which singleton this is (`Range`, `FunctoolsPartial`, `TypeVar`, …).
        /// `className` alone cannot distinguish them, since several share a class.
        known_instance_kind: &'static str,
        /// `range(...)` results whose emptiness ty could determine.
        #[serde(skip_serializing_if = "Option::is_none")]
        is_non_empty: Option<bool>,
        /// The callable a `functools.partial(...)` wraps.
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapped_type: Option<TypeId>,
        /// Signature left after a `functools.partial(...)` binds its arguments.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        parameters: Vec<ParameterInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        return_type: Option<TypeId>,
    },

    #[serde(rename_all = "camelCase")]
    TypedDict {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        fields: Vec<TypedDictFieldInfo>,
        /// `true` when the TypedDict forbids undeclared keys (`closed=True`,
        /// or equivalently `extra_items=Never`). Omitted when `false`.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        closed: bool,
        /// Explicit `extra_items=` policy, when declared. Omitted otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        extra_items: Option<TypedDictExtraItemsInfo>,
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

    /// An enum instance with one or more canonical members excluded
    /// (e.g. `Color & ~Literal[Color.RED]`).
    #[serde(rename_all = "camelCase")]
    EnumComplement {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        class_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        module_name: Option<String>,
        class_id: TypeId,
        excluded_names: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        rest: Vec<TypeId>,
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
            | Self::ClassRef { display, .. }
            | Self::SubclassOf { display, .. }
            | Self::TypeForm { display, .. }
            | Self::Union { display, .. }
            | Self::Intersection { display, .. }
            | Self::Function { display, .. }
            | Self::Callable { display, .. }
            | Self::BoundMethod { display, .. }
            | Self::WrapperDescriptor { display, .. }
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
            | Self::KnownInstance { display, .. }
            | Self::TypedDict { display, .. }
            | Self::TypeIs { display, .. }
            | Self::TypeGuard { display, .. }
            | Self::NewType { display, .. }
            | Self::SpecialForm { display, .. }
            | Self::Property { display, .. }
            | Self::EnumComplement { display, .. }
            | Self::Other { display, .. } => {
                *display = None;
            }
        }
    }
}
