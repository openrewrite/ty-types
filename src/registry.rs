use rustc_hash::FxHashMap;
use ty_python_semantic::types::Type;
use ty_python_semantic::Db;

use crate::protocol::{TypeDescriptor, TypeId};

/// A session-scoped registry that deduplicates types by identity.
///
/// Since ty's `Type<'db>` derives `Hash + Eq` and Salsa interns types,
/// the same type from different files maps to the same ID.
pub struct TypeRegistry<'db> {
    type_to_id: FxHashMap<Type<'db>, TypeId>,
    descriptors: FxHashMap<TypeId, TypeDescriptor>,
    next_id: TypeId,
}

pub struct RegistrationResult {
    pub type_id: TypeId,
    pub is_new: bool,
}

impl<'db> TypeRegistry<'db> {
    pub fn new() -> Self {
        Self {
            type_to_id: FxHashMap::default(),
            descriptors: FxHashMap::default(),
            next_id: 1, // start at 1, reserve 0 for "no type"
        }
    }

    /// Register a type and return its ID. If the type was already registered,
    /// returns the existing ID with is_new = false.
    pub fn register(&mut self, ty: Type<'db>, db: &'db dyn Db) -> RegistrationResult {
        if let Some(&id) = self.type_to_id.get(&ty) {
            return RegistrationResult {
                type_id: id,
                is_new: false,
            };
        }

        let id = self.next_id;
        self.next_id += 1;
        self.type_to_id.insert(ty, id);

        let descriptor = self.build_descriptor(ty, db);
        self.descriptors.insert(id, descriptor);

        RegistrationResult {
            type_id: id,
            is_new: true,
        }
    }

    /// Get the descriptor for a type ID.
    pub fn get_descriptor(&self, id: TypeId) -> Option<&TypeDescriptor> {
        self.descriptors.get(&id)
    }

    /// Get all descriptors as a map.
    pub fn all_descriptors(&self) -> std::collections::HashMap<TypeId, TypeDescriptor> {
        self.descriptors
            .iter()
            .map(|(&id, d)| (id, d.clone()))
            .collect()
    }

    /// Register a type that is a component of another type (e.g., union member),
    /// returning just its ID.
    fn register_component(&mut self, ty: Type<'db>, db: &'db dyn Db) -> TypeId {
        self.register(ty, db).type_id
    }

    fn display_string(&self, ty: Type<'db>, db: &'db dyn Db) -> String {
        format!("{}", ty.display(db))
    }

    fn build_descriptor(&mut self, ty: Type<'db>, db: &'db dyn Db) -> TypeDescriptor {
        match ty {
            Type::Dynamic(dynamic) => {
                let display = self.display_string(ty, db);
                let dynamic_kind = format!("{dynamic}");
                TypeDescriptor::Dynamic {
                    display,
                    dynamic_kind,
                }
            }

            Type::Never => TypeDescriptor::Never {
                display: "Never".to_string(),
            },

            Type::IntLiteral(n) => TypeDescriptor::IntLiteral {
                display: self.display_string(ty, db),
                value: n,
            },

            Type::BooleanLiteral(b) => TypeDescriptor::BoolLiteral {
                display: self.display_string(ty, db),
                value: b,
            },

            Type::StringLiteral(_) => TypeDescriptor::StringLiteral {
                display: self.display_string(ty, db),
                // The display string includes the value as Literal["..."]
                // We parse it out since the field accessor is pub(crate).
                value: extract_literal_string_value(&self.display_string(ty, db)),
            },

            Type::BytesLiteral(_) => TypeDescriptor::BytesLiteral {
                display: self.display_string(ty, db),
                value: self.display_string(ty, db),
            },

            Type::LiteralString => TypeDescriptor::LiteralString {
                display: "LiteralString".to_string(),
            },

            Type::AlwaysTruthy => TypeDescriptor::Truthy {
                display: "AlwaysTruthy".to_string(),
            },

            Type::AlwaysFalsy => TypeDescriptor::Falsy {
                display: "AlwaysFalsy".to_string(),
            },

            Type::Union(union_ty) => {
                let display = self.display_string(ty, db);
                // UnionType::elements is pub
                let members: Vec<TypeId> = union_ty
                    .elements(db)
                    .iter()
                    .map(|&member| self.register_component(member, db))
                    .collect();
                TypeDescriptor::Union { display, members }
            }

            Type::Intersection(intersection_ty) => {
                let display = self.display_string(ty, db);
                // iter_positive/iter_negative are pub
                let positive: Vec<TypeId> = intersection_ty
                    .iter_positive(db)
                    .map(|t| self.register_component(t, db))
                    .collect();
                let negative: Vec<TypeId> = intersection_ty
                    .iter_negative(db)
                    .map(|t| self.register_component(t, db))
                    .collect();
                TypeDescriptor::Intersection {
                    display,
                    positive,
                    negative,
                }
            }

            Type::NominalInstance(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Instance {
                    display: display.clone(),
                    class_name: display,
                    module_name: None,
                    type_args: vec![],
                }
            }

            Type::ProtocolInstance(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Instance {
                    display: display.clone(),
                    class_name: display,
                    module_name: None,
                    type_args: vec![],
                }
            }

            Type::ClassLiteral(_) | Type::GenericAlias(_) => {
                let display = self.display_string(ty, db);
                // Display is like "<class 'ClassName'>"
                let class_name = extract_class_name(&display);
                TypeDescriptor::ClassLiteral {
                    display,
                    class_name,
                }
            }

            Type::SubclassOf(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Other { display }
            }

            Type::FunctionLiteral(_) => {
                let display = self.display_string(ty, db);
                // Display format is "def name(params) -> return_type"
                let name = extract_function_name(&display);
                TypeDescriptor::Function { display, name }
            }

            Type::Callable(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Callable { display }
            }

            Type::BoundMethod(_) | Type::KnownBoundMethod(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::BoundMethod { display }
            }

            Type::ModuleLiteral(module_ty) => {
                let display = self.display_string(ty, db);
                // ModuleLiteralType::module is pub, Module::name takes &dyn salsa::Database
                let salsa_db: &dyn salsa::Database = db;
                let module_name = module_ty.module(salsa_db).name(salsa_db).to_string();
                TypeDescriptor::Module {
                    display,
                    module_name,
                }
            }

            Type::EnumLiteral(_) => {
                let display = self.display_string(ty, db);
                // Display is like "ClassName.MEMBER"
                let (class_name, member_name) = extract_enum_parts(&display);
                TypeDescriptor::EnumLiteral {
                    display,
                    class_name,
                    member_name,
                }
            }

            Type::TypeVar(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::TypeVar {
                    display: display.clone(),
                    name: display,
                }
            }

            Type::TypeAlias(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::TypeAlias {
                    display: display.clone(),
                    name: display,
                }
            }

            Type::TypedDict(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::TypedDict { display }
            }

            Type::TypeIs(_) | Type::TypeGuard(_) => {
                // return_type accessor is private, fall back to display + Other
                let display = self.display_string(ty, db);
                TypeDescriptor::Other { display }
            }

            Type::NewTypeInstance(newtype) => {
                let display = self.display_string(ty, db);
                // NewType::name is pub
                let name = newtype.name(db).to_string();
                // NewType::concrete_base_type is pub
                let base_type = self.register_component(newtype.concrete_base_type(db), db);
                TypeDescriptor::NewType {
                    display,
                    name,
                    base_type,
                }
            }

            Type::SpecialForm(sf) => {
                let display = self.display_string(ty, db);
                // SpecialFormType implements Display
                TypeDescriptor::SpecialForm {
                    display,
                    name: format!("{sf}"),
                }
            }

            Type::PropertyInstance(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Property { display }
            }

            Type::KnownInstance(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Other { display }
            }

            Type::WrapperDescriptor(_)
            | Type::DataclassDecorator(_)
            | Type::DataclassTransformer(_)
            | Type::BoundSuper(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Other { display }
            }
        }
    }
}

/// Extract the string value from a Literal["..."] display
fn extract_literal_string_value(display: &str) -> String {
    // Display format is like: Literal["hello"]
    // We want to extract: hello
    if let Some(start) = display.find('"') {
        if let Some(end) = display.rfind('"') {
            if start < end {
                return display[start + 1..end].to_string();
            }
        }
    }
    display.to_string()
}

/// Extract the class name from a "<class 'ClassName'>" display
fn extract_class_name(display: &str) -> String {
    // Display format: <class 'ClassName'>
    if let Some(start) = display.find('\'') {
        if let Some(end) = display.rfind('\'') {
            if start < end {
                return display[start + 1..end].to_string();
            }
        }
    }
    display.to_string()
}

/// Extract the function name from a "def name(...) -> ..." display
fn extract_function_name(display: &str) -> String {
    // Display formats:
    //   "def name(params) -> return_type"
    //   "Overload[...]"
    if let Some(rest) = display.strip_prefix("def ") {
        if let Some(paren_pos) = rest.find('(') {
            return rest[..paren_pos].to_string();
        }
    }
    display.to_string()
}

/// Extract class name and member name from "ClassName.MEMBER"
fn extract_enum_parts(display: &str) -> (String, String) {
    if let Some(dot_pos) = display.find('.') {
        let class_name = display[..dot_pos].to_string();
        let member_name = display[dot_pos + 1..].to_string();
        (class_name, member_name)
    } else {
        (display.to_string(), String::new())
    }
}
