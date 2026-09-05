# ty-types

A Rust CLI that exposes [ty](https://github.com/astral-sh/ty)'s Python type inference as structured JSON. It can infer types for one or more files in a single invocation, or run as a JSON-RPC server over stdio for multi-file sessions with cross-request type deduplication.

## Building

Requires Rust 1.96+. The `ruff/` submodule must be checked out first:

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

If `--project-root` is omitted, it defaults to the parent directory of the first file. Pass `--bindings` to attach a [`BindingInfo`](#bindinginfo) to each name and attribute reference.

A file whose inference panics is reported on stderr and omitted from `files`; the remaining files are still analyzed, and the exit status is non-zero. The JSON on stdout is complete for the files that did resolve, so a caller reads the status to tell a partial run from a complete one.

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
| `params.includeBindings` | `boolean` | `false` | Include a `binding` on each name and attribute reference. Costs roughly 10% inference time and 17% payload |

Returns:

```json
{
  "nodes": [ <NodeAttribution>, ... ],
  "types": { "<TypeId>": <TypeDescriptor>, ... }
}
```

ty's inference panics on some inputs (e.g. [astral-sh/ty#4454](https://github.com/astral-sh/ty/issues/4454)). Such a file answers with a JSON-RPC error of code `-32001` naming the file and the panic, and the session stays usable — later requests are unaffected, and types already registered reach the client with the next successful response.

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
| `binding` | `BindingInfo` | On `ExprName` and `ExprAttribute` nodes under `includeBindings`, where the reference resolves *(omitted otherwise)* |

**Node kinds:** `StmtFunctionDef`, `StmtClassDef`, `StmtAssign`, `StmtFor`, `StmtWith`, `ExprCall`, `ExprBoolOp`, `ExprBinOp`, `ExprUnaryOp`, `ExprLambda`, `ExprIf`, `ExprDict`, `ExprSet`, `ExprListComp`, `ExprSetComp`, `ExprDictComp`, `ExprGenerator`, `ExprAwait`, `ExprYield`, `ExprYieldFrom`, `ExprCompare`, `ExprFString`, `ExprTString`, `ExprStringLiteral`, `ExprBytesLiteral`, `ExprNumberLiteral`, `ExprBooleanLiteral`, `ExprNoneLiteral`, `ExprEllipsisLiteral`, `ExprAttribute`, `ExprSubscript`, `ExprStarred`, `ExprName`, `ExprList`, `ExprTuple`, `ExprSlice`, `Parameter`, `ParameterWithDefault`, `Alias`

### BindingInfo

Where a referenced symbol is bound, following re-export chains to the original binding. `pkg/__init__.py` re-exporting `LIMIT` from `pkg._impl` gives the same answer at every reference:

```json
{
  "definedIn": "pkg._impl",
  "qualifiedName": "pkg._impl.LIMIT"
}
```

| Field | Type | Description |
|---|---|---|
| `definedIn` | `string` | Module holding the binding |
| `qualifiedName` | `string` | Dotted path through the enclosing scopes, e.g. `app.Holder.MAX`. `definedIn` says where the module part ends |

A reference resolves when the binding is a function, class, type alias, or plain or annotated assignment. Parameters, loop and `with` and `except` targets, walrus and comprehension bindings, and attributes assigned in a method body all yield no `binding`. Scopes other than modules and classes appear in `qualifiedName` as ty spells them, so a function-local binding reads `app.<locals of function 'f'>.x`.

`moduleName` on a type descriptor is the module of the *value's* type — `builtins` for `SEP: str = "/"`, and absent entirely for `LIMIT = 5`. Binding is a property of the reference, which is why it is reported here rather than on the type.

`definedIn` names the module that binds the symbol, not the one it is conventionally imported from: `os.sep` reports `posixpath.sep`. This is the convention descriptors already use — `os.path.join` reports `moduleName: "posixpath"`.

A symbol bound in more than one branch of a conditional import resolves to the branch whose type was inferred at the reference. Where the branches declare the same type, the choice among them is arbitrary.

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
| `concatenatePrefix` | `boolean` | `true` on the leading positional parameters of a `Concatenate[T1, ..., Tn, P]` or `Concatenate[T1, ..., Tn, ...]` signature *(omitted when false)* |
| `paramSpecName` | `string` | Set on the `*args` / `**kwargs` entries that stand in for a `ParamSpec` tail, carrying that `ParamSpec`'s name (e.g. `"P"`) *(omitted when absent)* |

### TypeDescriptor

Every type in the registry is a tagged object with a `"kind"` discriminator. All variants include an optional `display` field with ty's human-readable representation (omit with `includeDisplay: false`).

Fields marked with *"omitted when empty"* are not present in the JSON when their value is empty or null.

`qualifiedName`, wherever it appears below, is the dotted path of the enclosing scopes followed by the item's own name (e.g. `a.b.C.D`). It is the field to key a class by: `moduleName` and `className` together cannot tell `a.b.C.D` apart from `a.b.D`. Scopes other than modules and classes appear as ty spells them, so a class defined inside a function reads `a.<locals of function 'f'>.C` — split it on `.` only if that spelling is accounted for. It is omitted where it would not identify the class: ty names a class built from a runtime name `<unknown>`, and two such classes in one scope render identically.

#### `instance`

An object of a class (e.g. `int`, `str`, `MyClass()`).

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Class name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `qualifiedName` | `string` | Fully qualified class name; absent for synthesized protocols *(omitted when empty)* |
| `supertypes` | `integer[]` | Resolved base class type IDs *(omitted when empty)* |
| `typeArgs` | `integer[]` | Specialization args, e.g. `list[int]` → `[<int>]` *(omitted when empty)* |
| `classId` | `integer` | Type ID of the corresponding `classLiteral` *(omitted when empty)* |
| `tupleElements` | `TupleElement[]` | Element types, on tuples and their subclasses *(omitted otherwise)* |

Each `TupleElement` is `{"typeId": <integer>, "kind": <string>}`, where `kind` is `fixed` for a single element, `homogeneous` for the `...` segment of `tuple[int, ...]`, or `typeVarTuple` for the `*Ts` of `tuple[int, *Ts]`. The latter two each stand for an unknown number of elements.

#### `classLiteral`

A class object itself (the value of `type[MyClass]`).

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Class name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `qualifiedName` | `string` | Fully qualified class name *(omitted when empty)* |
| `typeParameters` | `integer[]` | Generic type parameters (`T`, `U`, ...) *(omitted when empty)* |
| `supertypes` | `integer[]` | Explicit base classes *(omitted when empty)* |
| `members` | `ClassMemberInfo[]` | Directly defined class members *(omitted when empty)* |

`ClassMemberInfo`: `{ "name": string, "typeId": integer }`

#### `subclassOf`

A `type[C]` constraint (subclass relationship).

| Field | Type | Description |
|---|---|---|
| `base` | `integer` | Type ID of the base: a `classLiteral` for a class or a protocol declared as one, an `instance` for a synthesized protocol, otherwise `dynamic` or `typeVar` |

#### `typeForm`

A `TypeForm[T]` value wrapping a type expression (PEP 747).

| Field | Type | Description |
|---|---|---|
| `typeArgument` | `integer` | Type ID of the wrapped type expression |

#### `knownInstance`

A well-known singleton instance ty tracks specially, such as `TypeVar`, `typing.Callable`, `functools.partial(...)` or `range(...)`.

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Class name |
| `knownInstanceKind` | `string` | Which known instance this is |
| `isNonEmpty` | `boolean` | Whether the instance is known to be non-empty *(omitted when unknown)* |
| `wrappedType` | `integer` | Type ID of the wrapped type, where the kind wraps one *(omitted otherwise)* |
| `parameters` | `ParameterInfo[]` | Parameters, where the kind is callable *(omitted when empty)* |
| `returnType` | `integer` | Return type ID, where the kind is callable *(omitted when empty)* |

#### `enumComplement`

An enum instance with one or more canonical members excluded, e.g. `Color & ~Literal[Color.RED]`.

| Field | Type | Description |
|---|---|---|
| `className` | `string` | Enum class name |
| `moduleName` | `string` | Defining module *(omitted when empty)* |
| `qualifiedName` | `string` | Fully qualified enum class name *(omitted when empty)* |
| `classId` | `integer` | Type ID of the enum's `classLiteral` |
| `excludedNames` | `string[]` | Member names excluded from the enum |
| `rest` | `integer[]` | Type IDs of the members that remain |

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
| `qualifiedName` | `string` | Fully qualified name of the enum class, not the member *(omitted when empty)* |
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
| `qualifiedName` | `string` | Fully qualified alias name *(omitted when empty)* |

#### `typedDict`

| Field | Type | Description |
|---|---|---|
| `name` | `string` | TypedDict name |
| `qualifiedName` | `string` | Fully qualified TypedDict name *(omitted when empty)* |
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
| `qualifiedName` | `string` | Fully qualified NewType name *(omitted when empty)* |
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
