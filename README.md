# ty-types

A Rust CLI that exposes [ty](https://github.com/astral-sh/ty)'s Python type inference as structured JSON. It can infer types for one or more files in a single invocation, or run as a JSON-RPC server over stdio for multi-file sessions with cross-request type deduplication.

## Building

Requires Rust 1.93+. The `ruff/` submodule must be checked out first:

```bash
git submodule update --init
cargo build --release
```

The binary is at `target/release/ty-types`.

## Usage

### One-shot mode

Pass one or more Python files as arguments. The output is a single JSON object written to stdout:

```bash
ty-types app.py utils.py --project-root /path/to/project
```

If `--project-root` is omitted, it defaults to the parent directory of the first file.

**Output format:**

```json
{
  "files": {
    "/absolute/path/to/app.py": [ <NodeAttribution>, ... ],
    "/absolute/path/to/utils.py": [ <NodeAttribution>, ... ]
  },
  "types": {
    "1": <TypeDescriptor>,
    "2": <TypeDescriptor>
  }
}
```

`files` maps each file path to its list of typed AST nodes. `types` is a shared registry — nodes reference types by ID, and the same type (e.g. `int`) gets a single entry even if it appears in multiple files.

### JSON-RPC server mode

For processing many files or integrating with editors and tooling, run as a persistent server:

```bash
ty-types --serve
```

The server reads JSON-RPC requests from stdin (one per line) and writes responses to stdout. A session looks like:

```
→ {"jsonrpc":"2.0","method":"initialize","params":{"projectRoot":"/path/to/project"},"id":1}
← {"jsonrpc":"2.0","result":{"ok":true},"id":1}

→ {"jsonrpc":"2.0","method":"getTypes","params":{"file":"app.py"},"id":2}
← {"jsonrpc":"2.0","result":{"nodes":[...],"types":{"1":...,"2":...}},"id":2}

→ {"jsonrpc":"2.0","method":"getTypes","params":{"file":"utils.py"},"id":3}
← {"jsonrpc":"2.0","result":{"nodes":[...],"types":{"5":...}},"id":3}

→ {"jsonrpc":"2.0","method":"shutdown","id":99}
← {"jsonrpc":"2.0","result":{"ok":true},"id":99}
```

The type registry persists across `getTypes` requests within a session. Each `getTypes` response includes only the *newly discovered* types — types already sent in a previous response are not repeated. The client accumulates the registry as it goes.

To retrieve the full accumulated registry at any point, call `getTypeRegistry`.

## JSON-RPC methods

### `initialize`

Must be the first call. Creates the project database.

| Field | Type | Description |
|---|---|---|
| `params.projectRoot` | `string` | Absolute path to the Python project root |

Returns `{"ok": true}`.

### `getTypes`

Infers types for a Python file and returns the typed AST nodes plus any new type descriptors.

| Field | Type | Default | Description |
|---|---|---|---|
| `params.file` | `string` | | File path (absolute or relative to project root) |
| `params.includeDisplay` | `boolean` | `true` | Include human-readable `display` strings on type descriptors |

Returns:

```json
{
  "nodes": [ <NodeAttribution>, ... ],
  "types": { "<TypeId>": <TypeDescriptor>, ... }
}
```

### `getTypeRegistry`

Returns the full accumulated type registry from all `getTypes` calls in the current session. Takes no parameters.

Returns:

```json
{
  "types": { "<TypeId>": <TypeDescriptor>, ... }
}
```

### `shutdown`

Ends the session and exits the server. Returns `{"ok": true}`.

## Schema

### NodeAttribution

Each entry in the `nodes` array represents a typed AST node:

```json
{
  "start": 0,
  "end": 5,
  "nodeKind": "ExprName",
  "typeId": 1,
  "callSignature": null
}
```

| Field | Type | Description |
|---|---|---|
| `start` | `integer` | Byte offset of the node start in the source file |
| `end` | `integer` | Byte offset of the node end |
| `nodeKind` | `string` | AST node kind (see below) |
| `typeId` | `integer \| null` | Reference into the type registry |
| `callSignature` | `CallSignatureInfo \| null` | Present only on `ExprCall` nodes |

**Node kinds:** `StmtFunctionDef`, `StmtClassDef`, `StmtAssign`, `StmtFor`, `StmtWith`, `ExprCall`, `ExprBoolOp`, `ExprBinOp`, `ExprUnaryOp`, `ExprLambda`, `ExprIf`, `ExprDict`, `ExprSet`, `ExprListComp`, `ExprSetComp`, `ExprDictComp`, `ExprGenerator`, `ExprAwait`, `ExprYield`, `ExprYieldFrom`, `ExprCompare`, `ExprFString`, `ExprTString`, `ExprStringLiteral`, `ExprBytesLiteral`, `ExprNumberLiteral`, `ExprBooleanLiteral`, `ExprNoneLiteral`, `ExprEllipsisLiteral`, `ExprAttribute`, `ExprSubscript`, `ExprStarred`, `ExprName`, `ExprList`, `ExprTuple`, `ExprSlice`, `Parameter`, `ParameterWithDefault`, `Alias`

### CallSignatureInfo

Attached to `ExprCall` nodes. Contains the resolved signature at the call site, including any generic specialization:

```json
{
  "parameters": [ <ParameterInfo>, ... ],
  "returnTypeId": 3,
  "typeArguments": [4]
}
```

| Field | Type | Description |
|---|---|---|
| `parameters` | `ParameterInfo[]` | Resolved parameters of the called function |
| `returnTypeId` | `integer \| null` | Return type (specialized if generic) |
| `typeArguments` | `integer[]` | Type arguments inferred for generic calls (e.g. `T=int`) |

### ParameterInfo

```json
{
  "name": "x",
  "typeId": 2,
  "kind": "positionalOrKeyword",
  "hasDefault": true,
  "defaultTypeId": 5
}
```

| Field | Type | Description |
|---|---|---|
| `name` | `string` | Parameter name |
| `typeId` | `integer \| null` | Annotated type |
| `kind` | `string` | One of `positionalOnly`, `positionalOrKeyword`, `keywordOnly`, `variadic`, `keywordVariadic` |
| `hasDefault` | `boolean` | Whether the parameter has a default value |
| `defaultTypeId` | `integer \| null` | Type of the default value (e.g. `Literal[42]`) |

### TypeDescriptor

Every type in the registry is a tagged object with a `"kind"` discriminator. All variants include an optional `display` field with ty's human-readable representation (omit with `includeDisplay: false`).

Fields marked with *"omitted when empty"* are not present in the JSON when their value is empty or null.

#### `instance`

An object of a class (e.g. `int`, `str`, `MyClass()`).

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Class name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `supertypes` | `integer[]` | Resolved base class type IDs *(omitted when empty)* |
| `typeArgs` | `integer[]` | Specialization args, e.g. `list[int]` → `[<int>]` *(omitted when empty)* |
| `classId` | `integer` | Type ID of the corresponding `classLiteral` *(omitted when empty)* |

#### `classLiteral`

A class object itself (the value of `type[MyClass]`).

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Class name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `typeParameters` | `integer[]` | Generic type parameters (`T`, `U`, ...) *(omitted when empty)* |
| `supertypes` | `integer[]` | Explicit base classes *(omitted when empty)* |
| `members` | `ClassMemberInfo[]` | Directly defined class members *(omitted when empty)* |

`ClassMemberInfo`: `{ "name": string, "typeId": integer }`

#### `subclassOf`

A `type[C]` constraint (subclass relationship).

| Field | Type | Description |
|---|---|---|
| `base` | `integer` | Type ID of the base `classLiteral` |

#### `union`

A union type (`X | Y`).

| Field | Type | Description |
|---|---|---|
| `members` | `integer[]` | Type IDs of the union members |

#### `intersection`

A narrowed type from control flow (e.g. `isinstance` checks).

| Field | Type | Description |
|---|---|---|
| `positive` | `integer[]` | Types that must all be satisfied |
| `negative` | `integer[]` | Types that must not be satisfied |

#### `function`

A named function.

| Field | Type | Description |
|---|---|---|
| `name` | `string` | Function name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `typeParameters` | `integer[]` | Generic type parameters *(omitted when empty)* |
| `parameters` | `ParameterInfo[]` | Full signature |
| `returnType` | `integer \| null` | Return type ID |

#### `boundMethod`

A method bound to an instance.

| Field | Type | Description |
|---|---|---|
| `name` | `string \| null` | Method name *(omitted when empty)* |
| `moduleName` | `string \| null` | Defining module *(omitted when empty)* |
| `typeParameters` | `integer[]` | Generic type parameters *(omitted when empty)* |
| `parameters` | `ParameterInfo[]` | Full signature (without `self`) |
| `returnType` | `integer \| null` | Return type ID |

#### `callable`

A generic callable with unknown signature.

No additional fields beyond `display`.

#### `intLiteral`

| Field | Type | Description |
|---|---|---|
| `value` | `integer` | The literal value |

#### `boolLiteral`

| Field | Type | Description |
|---|---|---|
| `value` | `boolean` | `true` or `false` |

#### `stringLiteral`

| Field | Type | Description |
|---|---|---|
| `value` | `string` | The literal string value |

#### `bytesLiteral`

| Field | Type | Description |
|---|---|---|
| `value` | `string` | Display representation (e.g. `Literal[b"data"]`) |

#### `enumLiteral`

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Enum class name |
| `memberName` | `string` | Member name |

#### `literalString`

The `typing.LiteralString` special form. No additional fields.

#### `typeVar`

A generic type variable.

| Field | Type | Description |
|---|---|---|
| `name` | `string` | Variable name (e.g. `T`) |
| `variance` | `string \| null` | `covariant`, `contravariant`, or `invariant` *(omitted when empty)* |
| `upperBound` | `integer \| null` | Bound type ID (from `T: bound=int`) *(omitted when empty)* |
| `constraints` | `integer[]` | Constraint type IDs (from `T(int, str)`) *(omitted when empty)* |

#### `module`

| Field | Type | Description |
|---|---|---|
| `moduleName` | `string` | Fully qualified module name |

#### `typeAlias`

| Field | Type | Description |
|---|---|---|
| `name` | `string` | Alias name |

#### `typedDict`

| Field | Type | Description |
|---|---|---|
| `name` | `string` | TypedDict name |
| `fields` | `TypedDictFieldInfo[]` | Typed fields *(omitted when empty)* |

`TypedDictFieldInfo`: `{ "name": string, "typeId": integer, "required": boolean, "readOnly": boolean }`

#### `typeIs`

A `TypeIs[T]` return type for type narrowing functions.

| Field | Type | Description |
|---|---|---|
| `narrowedType` | `integer` | The narrowed type ID |

#### `typeGuard`

A `TypeGuard[T]` return type for type guard functions.

| Field | Type | Description |
|---|---|---|
| `guardedType` | `integer` | The guarded type ID |

#### `newType`

| Field | Type | Description |
|---|---|---|
| `name` | `string` | NewType name |
| `baseType` | `integer` | Underlying type ID |

#### `specialForm`

Typing special forms like `Any`, `Never`, `ClassVar`, etc.

| Field | Type | Description |
|---|---|---|
| `name` | `string` | Form name |

#### `dynamic`

Unknown or dynamically typed values.

| Field | Type | Description |
|---|---|---|
| `dynamicKind` | `string` | e.g. `Unknown` |

#### `never`

The bottom type (unreachable code). No additional fields.

#### `truthy` / `falsy`

Narrowed truthiness. No additional fields.

#### `property`

A property descriptor. No additional fields.

#### `other`

Fallback for ty-internal types not yet mapped to a structured descriptor. No additional fields beyond `display`.

## Example

Given `example.py`:

```python
x: int = 42
```

**One-shot:**

```bash
ty-types example.py
```

```json
{
  "files": {
    "/path/to/example.py": [
      { "start": 0, "end": 1, "nodeKind": "ExprName", "typeId": 1 },
      { "start": 9, "end": 11, "nodeKind": "ExprNumberLiteral", "typeId": 2 }
    ]
  },
  "types": {
    "1": { "kind": "instance", "display": "int", "className": "int" },
    "2": { "kind": "intLiteral", "display": "Literal[42]", "value": 42 }
  }
}
```

**Server mode** (processing two files across requests):

```bash
echo '{"jsonrpc":"2.0","method":"initialize","params":{"projectRoot":"/path/to/project"},"id":1}
{"jsonrpc":"2.0","method":"getTypes","params":{"file":"example.py"},"id":2}
{"jsonrpc":"2.0","method":"getTypes","params":{"file":"other.py"},"id":3}
{"jsonrpc":"2.0","method":"shutdown","id":99}' | ty-types --serve
```

The response for `id:2` includes type descriptors for `int` and `Literal[42]`. If `other.py` also uses `int`, the response for `id:3` will *not* repeat the `int` descriptor — it was already sent. Call `getTypeRegistry` at any point to get the full accumulated registry.
