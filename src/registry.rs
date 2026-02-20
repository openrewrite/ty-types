use rustc_hash::FxHashMap;
use ty_python_semantic::Db;
use ty_python_semantic::types::list_members;
use ty_python_semantic::types::{
    ClassLiteral, LiteralValueTypeKind, ParameterKind, Type, TypeGuardLike,
};

use crate::protocol::{ClassMemberInfo, ParameterInfo, TypeDescriptor, TypeId, TypedDictFieldInfo};

/// A session-scoped registry that deduplicates types by identity.
///
/// Since ty's `Type<'db>` derives `Hash + Eq` and Salsa interns types,
/// the same type from different files maps to the same ID.
pub struct TypeRegistry<'db> {
    type_to_id: FxHashMap<Type<'db>, TypeId>,
    descriptors: FxHashMap<TypeId, TypeDescriptor>,
    next_id: TypeId,
    /// Tracks all type IDs registered since the last `start_tracking()` call,
    /// including component types registered transitively by `build_descriptor`.
    tracked_new_ids: Vec<TypeId>,
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
            tracked_new_ids: Vec::new(),
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
        self.tracked_new_ids.push(id);

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

    /// Begin tracking newly registered types (including transitive components).
    pub fn start_tracking(&mut self) {
        self.tracked_new_ids.clear();
    }

    /// Drain all type IDs registered since the last `start_tracking()` call
    /// and return their descriptors.
    pub fn drain_new_types(&mut self) -> std::collections::HashMap<TypeId, TypeDescriptor> {
        self.tracked_new_ids
            .drain(..)
            .filter_map(|id| self.descriptors.get(&id).map(|d| (id, d.clone())))
            .collect()
    }

    /// Register a type that is a component of another type (e.g., union member,
    /// parameter type), returning just its ID.
    pub fn register_component(&mut self, ty: Type<'db>, db: &'db dyn Db) -> TypeId {
        self.register(ty, db).type_id
    }

    fn display_string(&self, ty: Type<'db>, db: &'db dyn Db) -> Option<String> {
        Some(format!("{}", ty.display(db)))
    }

    fn build_function_params(
        &mut self,
        func_ty: Type<'db>,
        db: &'db dyn Db,
    ) -> (Vec<ParameterInfo>, Option<TypeId>) {
        let func = match func_ty.as_function_literal() {
            Some(f) => f,
            None => return (vec![], None),
        };
        let callable_sig = func.signature(db);
        // TODO: only the first overload is used; overloaded functions lose
        // all but the first signature. Consider representing overloads.
        let sig = match callable_sig.iter().next() {
            Some(s) => s,
            None => return (vec![], None),
        };

        let parameters: Vec<ParameterInfo> = sig
            .parameters()
            .into_iter()
            .map(|param| {
                let type_id = {
                    let ann_ty = param.annotated_type();
                    if matches!(ann_ty, Type::Dynamic(_)) {
                        None
                    } else {
                        Some(self.register_component(ann_ty, db))
                    }
                };
                let name = param
                    .display_name()
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                let (kind, has_default) = match param.kind() {
                    ParameterKind::PositionalOnly { default_type, .. } => {
                        ("positionalOnly", default_type.is_some())
                    }
                    ParameterKind::PositionalOrKeyword { default_type, .. } => {
                        ("positionalOrKeyword", default_type.is_some())
                    }
                    ParameterKind::Variadic { .. } => ("variadic", false),
                    ParameterKind::KeywordOnly { default_type, .. } => {
                        ("keywordOnly", default_type.is_some())
                    }
                    ParameterKind::KeywordVariadic { .. } => ("keywordVariadic", false),
                };
                ParameterInfo {
                    name,
                    type_id,
                    kind,
                    has_default,
                }
            })
            .collect();

        let return_ty = sig.return_ty;
        let return_type = if matches!(return_ty, Type::Dynamic(_)) {
            None
        } else {
            Some(self.register_component(return_ty, db))
        };

        (parameters, return_type)
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
                display: Some("Never".to_string()),
            },

            Type::LiteralValue(literal) => {
                let display = self.display_string(ty, db);
                match literal.kind() {
                    LiteralValueTypeKind::Int(n) => TypeDescriptor::IntLiteral {
                        display,
                        value: n.as_i64(),
                    },
                    LiteralValueTypeKind::Bool(b) => {
                        TypeDescriptor::BoolLiteral { display, value: b }
                    }
                    LiteralValueTypeKind::String(s) => TypeDescriptor::StringLiteral {
                        display,
                        value: s.value(db).to_string(),
                    },
                    LiteralValueTypeKind::Bytes(_) => {
                        let value = format!("{}", ty.display(db));
                        TypeDescriptor::BytesLiteral { display, value }
                    }
                    LiteralValueTypeKind::LiteralString => {
                        TypeDescriptor::LiteralString { display }
                    }
                    LiteralValueTypeKind::Enum(e) => TypeDescriptor::EnumLiteral {
                        display,
                        class_name: e.enum_class(db).name(db).to_string(),
                        member_name: e.name(db).to_string(),
                    },
                }
            }

            Type::AlwaysTruthy => TypeDescriptor::Truthy {
                display: Some("AlwaysTruthy".to_string()),
            },

            Type::AlwaysFalsy => TypeDescriptor::Falsy {
                display: Some("AlwaysFalsy".to_string()),
            },

            Type::Union(union_ty) => {
                let display = self.display_string(ty, db);
                let members: Vec<TypeId> = union_ty
                    .elements(db)
                    .iter()
                    .map(|&member| self.register_component(member, db))
                    .collect();
                TypeDescriptor::Union { display, members }
            }

            Type::Intersection(intersection_ty) => {
                let display = self.display_string(ty, db);
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

            Type::NominalInstance(instance) => {
                let display = self.display_string(ty, db);
                let class_name = instance.class_literal(db).name(db).to_string();

                // Extract type arguments from specialization
                let class_type = instance.class(db);
                let type_args: Vec<TypeId> = class_type
                    .static_class_literal(db)
                    .and_then(|(_, spec)| spec)
                    .map(|spec| {
                        spec.types(db)
                            .iter()
                            .map(|&t| self.register_component(t, db))
                            .collect()
                    })
                    .unwrap_or_default();

                // Register the class literal as a component
                let class_id = Some(
                    self.register_component(Type::ClassLiteral(instance.class_literal(db)), db),
                );

                TypeDescriptor::Instance {
                    display,
                    class_name,
                    module_name: None,
                    type_args,
                    class_id,
                }
            }

            Type::ProtocolInstance(instance) => {
                let display = self.display_string(ty, db);
                if let Some(nominal) = instance.to_nominal_instance() {
                    let class_name = nominal.class_literal(db).name(db).to_string();

                    let class_type = nominal.class(db);
                    let type_args: Vec<TypeId> = class_type
                        .static_class_literal(db)
                        .and_then(|(_, spec)| spec)
                        .map(|spec| {
                            spec.types(db)
                                .iter()
                                .map(|&t| self.register_component(t, db))
                                .collect()
                        })
                        .unwrap_or_default();

                    let class_id = Some(
                        self.register_component(Type::ClassLiteral(nominal.class_literal(db)), db),
                    );

                    TypeDescriptor::Instance {
                        display,
                        class_name,
                        module_name: None,
                        type_args,
                        class_id,
                    }
                } else {
                    // Synthesized protocols have no class backing
                    let class_name = format!("{}", ty.display(db));
                    TypeDescriptor::Instance {
                        display,
                        class_name,
                        module_name: None,
                        type_args: vec![],
                        class_id: None,
                    }
                }
            }

            Type::ClassLiteral(class_literal) => {
                let display = self.display_string(ty, db);
                let class_name = class_literal.name(db).to_string();
                let supertypes: Vec<TypeId> = match class_literal {
                    ClassLiteral::Static(static_class) => static_class
                        .explicit_bases(db)
                        .iter()
                        .map(|&base| self.register_component(base, db))
                        .collect(),
                    ClassLiteral::Dynamic(dynamic_class) => dynamic_class
                        .explicit_bases(db)
                        .iter()
                        .map(|&base| self.register_component(base, db))
                        .collect(),
                    ClassLiteral::DynamicNamedTuple(_) => vec![],
                };

                // Extract directly-defined class members (not inherited)
                let members: Vec<ClassMemberInfo> = match class_literal {
                    ClassLiteral::Static(static_class) => {
                        list_members::all_end_of_scope_members(db, static_class.body_scope(db))
                            .map(|mwd| {
                                let type_id = self.register_component(mwd.member.ty, db);
                                ClassMemberInfo {
                                    name: mwd.member.name.to_string(),
                                    type_id,
                                }
                            })
                            .collect()
                    }
                    _ => vec![],
                };

                TypeDescriptor::ClassLiteral {
                    display,
                    class_name,
                    supertypes,
                    members,
                }
            }

            Type::GenericAlias(alias) => {
                let display = self.display_string(ty, db);
                let origin = alias.origin(db);
                let class_name = origin.name(db).to_string();
                let supertypes: Vec<TypeId> = origin
                    .explicit_bases(db)
                    .iter()
                    .map(|&base| self.register_component(base, db))
                    .collect();
                let members: Vec<ClassMemberInfo> =
                    list_members::all_end_of_scope_members(db, origin.body_scope(db))
                        .map(|mwd| {
                            let type_id = self.register_component(mwd.member.ty, db);
                            ClassMemberInfo {
                                name: mwd.member.name.to_string(),
                                type_id,
                            }
                        })
                        .collect();
                TypeDescriptor::ClassLiteral {
                    display,
                    class_name,
                    supertypes,
                    members,
                }
            }

            Type::SubclassOf(subclass_of_ty) => {
                let display = self.display_string(ty, db);
                let base = match subclass_of_ty.subclass_of() {
                    ty_python_semantic::types::SubclassOfInner::Class(class_ty) => {
                        self.register_component(Type::ClassLiteral(class_ty.class_literal(db)), db)
                    }
                    _ => {
                        // Dynamic or TypeVar — register the full type as-is
                        self.register_component(ty, db)
                    }
                };
                TypeDescriptor::SubclassOf { display, base }
            }

            Type::FunctionLiteral(func) => {
                let display = self.display_string(ty, db);
                let name = func.name(db).to_string();
                let (parameters, return_type) = self.build_function_params(ty, db);
                TypeDescriptor::Function {
                    display,
                    name,
                    parameters,
                    return_type,
                }
            }

            Type::Callable(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Callable { display }
            }

            Type::BoundMethod(bound) => {
                let display = self.display_string(ty, db);
                let func = bound.function(db);
                let func_ty = Type::FunctionLiteral(func);
                let name = Some(func.name(db).to_string());
                let (parameters, return_type) = self.build_function_params(func_ty, db);
                TypeDescriptor::BoundMethod {
                    display,
                    name,
                    parameters,
                    return_type,
                }
            }

            Type::KnownBoundMethod(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::BoundMethod {
                    display,
                    name: None,
                    parameters: vec![],
                    return_type: None,
                }
            }

            Type::ModuleLiteral(module_ty) => {
                let display = self.display_string(ty, db);
                let module_name = module_ty.module(db).name(db).to_string();
                TypeDescriptor::Module {
                    display,
                    module_name,
                }
            }

            Type::TypeVar(_) => {
                let display_str = format!("{}", ty.display(db));
                TypeDescriptor::TypeVar {
                    display: Some(display_str.clone()),
                    name: display_str,
                }
            }

            Type::TypeAlias(_) => {
                let display_str = format!("{}", ty.display(db));
                TypeDescriptor::TypeAlias {
                    display: Some(display_str.clone()),
                    name: display_str,
                }
            }

            Type::TypedDict(typed_dict) => {
                let display = self.display_string(ty, db);
                let name = typed_dict
                    .defining_class()
                    .map(|c| c.name(db).to_string())
                    .unwrap_or_default();
                let schema = typed_dict.items(db);
                let fields: Vec<TypedDictFieldInfo> = schema
                    .iter()
                    .map(|(field_name, field)| {
                        let type_id = self.register_component(field.declared_ty, db);
                        TypedDictFieldInfo {
                            name: field_name.to_string(),
                            type_id,
                            required: field.is_required(),
                            read_only: field.is_read_only(),
                        }
                    })
                    .collect();
                TypeDescriptor::TypedDict {
                    display,
                    name,
                    fields,
                }
            }

            Type::TypeIs(type_is) => {
                let display = self.display_string(ty, db);
                let narrowed_type = self.register_component(type_is.return_type(db), db);
                TypeDescriptor::TypeIs {
                    display,
                    narrowed_type,
                }
            }

            Type::TypeGuard(type_guard) => {
                let display = self.display_string(ty, db);
                let guarded_type = self.register_component(type_guard.return_type(db), db);
                TypeDescriptor::TypeGuard {
                    display,
                    guarded_type,
                }
            }

            Type::NewTypeInstance(newtype) => {
                let display = self.display_string(ty, db);
                let name = newtype.name(db).to_string();
                let base_type = self.register_component(newtype.concrete_base_type(db), db);
                TypeDescriptor::NewType {
                    display,
                    name,
                    base_type,
                }
            }

            Type::SpecialForm(sf) => {
                let display = self.display_string(ty, db);
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
