use rustc_hash::FxHashMap;
use ty_module_resolver::ResolverFile;
use ty_python_semantic::types::display::qualified_name_components_from_scope;
use ty_python_semantic::types::list_members;
use ty_python_semantic::types::signatures::{ConcatenateTail, ParametersKind, Signature};
use ty_python_semantic::types::tuple::{Tuple, VariableSegment};
use ty_python_semantic::types::{
    ClassLiteral, GenericContext, KnownInstanceType, LiteralValueTypeKind, NominalInstanceType,
    ParameterKind, ProgramEnvironment, SubclassOfInner, Type, TypeVarKind, TypeVarVariance,
};
use ty_python_semantic::{Db, Program};

use crate::protocol::{
    ClassMemberInfo, ParameterInfo, TupleElementInfo, TypeDescriptor, TypeId,
    TypedDictExtraItemsInfo, TypedDictFieldInfo,
};

/// A session-scoped registry that deduplicates types by identity.
///
/// Since ty's `Type<'db>` derives `Hash + Eq` and Salsa interns types,
/// the same type from different files maps to the same ID.
pub struct TypeRegistry<'db> {
    type_to_id: FxHashMap<Type<'db>, TypeId>,
    descriptors: FxHashMap<TypeId, TypeDescriptor>,
    next_id: TypeId,
    /// Tracks all type IDs registered since the last `drain_new_types()`,
    /// including component types registered transitively by `build_descriptor`.
    tracked_new_ids: Vec<TypeId>,
    /// A session covers a single project, hence a single program, so one
    /// environment serves every descriptor built here.
    env: ProgramEnvironment<'db>,
}

pub struct RegistrationResult {
    pub type_id: TypeId,
    pub is_new: bool,
}

/// A qualified name that cannot identify the class it names is worse than none:
/// ty spells a class built from a runtime name `<unknown>`, so two of them in one
/// scope render identically and a client keying by this field merges them.
fn identifying(qualified_name: String) -> Option<String> {
    (!qualified_name.contains("<unknown>")).then_some(qualified_name)
}

impl<'db> TypeRegistry<'db> {
    pub fn new(program: Program<'db>) -> Self {
        Self {
            type_to_id: FxHashMap::default(),
            descriptors: FxHashMap::default(),
            next_id: 1, // start at 1, reserve 0 for "no type"
            tracked_new_ids: Vec::new(),
            env: ProgramEnvironment::from_program(program),
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

        // `build_descriptor` registers component types, so a self-referential type only
        // terminates if the id is interned first. It also runs ty queries that can panic,
        // which `catch_collect` turns into one failed file — seeding the descriptor and
        // the pending id keeps every id the client receives resolvable.
        self.descriptors
            .insert(id, TypeDescriptor::Other { display: None });
        self.tracked_new_ids.push(id);

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

    /// Drain all type IDs registered since the previous drain and return their
    /// descriptors. Draining is the only thing that clears the pending set, so types
    /// registered by a request that failed part-way reach the client with the next
    /// successful one.
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

    fn resolve_module_name(&self, db: &'db dyn Db, file: ruff_db::files::File) -> Option<String> {
        let resolver_file = ResolverFile::new(db, file, self.env.resolver_environment(db));
        ty_module_resolver::file_to_module(db, resolver_file).map(|m| m.name(db).to_string())
    }

    fn display_string(&self, ty: Type<'db>, db: &'db dyn Db) -> Option<String> {
        Some(format!("{}", ty.display(db, &self.env)))
    }

    fn build_type_parameters(
        &mut self,
        generic_context: Option<GenericContext<'db>>,
        db: &'db dyn Db,
    ) -> Vec<TypeId> {
        let Some(ctx) = generic_context else {
            return vec![];
        };
        let vars: Vec<_> = ctx
            .variables(db)
            .filter(|bound_tv| !bound_tv.typevar(db).is_self(db))
            .collect();
        vars.into_iter()
            .map(|bound_tv| self.register_component(Type::TypeVar(bound_tv), db))
            .collect()
    }

    fn supertypes_from_class_literal(
        &mut self,
        cl: ClassLiteral<'db>,
        db: &'db dyn Db,
    ) -> Vec<TypeId> {
        cl.explicit_bases(db)
            .iter()
            .map(|&base| self.register_component(base, db))
            .collect()
    }

    fn build_params_from_signature(
        &mut self,
        sig: &Signature<'db>,
        db: &'db dyn Db,
    ) -> (Vec<TypeId>, Vec<ParameterInfo>, Option<TypeId>) {
        let type_parameters = self.build_type_parameters(sig.generic_context, db);

        let (in_concatenate, param_spec_name) = match sig.parameters().kind() {
            ParametersKind::ParamSpec(tv) => (false, Some(tv.name(db).to_string())),
            ParametersKind::Concatenate(ConcatenateTail::ParamSpec(tv)) => {
                (true, Some(tv.name(db).to_string()))
            }
            ParametersKind::Concatenate(ConcatenateTail::Gradual) => (true, None),
            _ => (false, None),
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
                let default_type_id = param
                    .default_type(db)
                    .map(|dt| self.register_component(dt, db));
                let is_variadic = param.is_variadic() || param.is_keyword_variadic();
                let concatenate_prefix = in_concatenate && !is_variadic;
                let this_param_spec_name = if is_variadic {
                    param_spec_name.clone()
                } else {
                    None
                };
                ParameterInfo {
                    name,
                    type_id,
                    kind,
                    has_default,
                    default_type_id,
                    concatenate_prefix,
                    param_spec_name: this_param_spec_name,
                }
            })
            .collect();

        let return_ty = sig.return_ty;
        let return_type = if matches!(return_ty, Type::Dynamic(_)) {
            None
        } else {
            Some(self.register_component(return_ty, db))
        };

        (type_parameters, parameters, return_type)
    }

    fn build_function_params(
        &mut self,
        func_ty: Type<'db>,
        db: &'db dyn Db,
    ) -> (Vec<TypeId>, Vec<ParameterInfo>, Option<TypeId>) {
        let func = match func_ty.as_function_literal() {
            Some(f) => f,
            None => return (vec![], vec![], None),
        };
        let callable_sig = func.signature(db);
        let sig = match callable_sig.iter().next() {
            Some(s) => s,
            None => return (vec![], vec![], None),
        };
        self.build_params_from_signature(sig, db)
    }

    /// Collapses the legacy and PEP 695 spellings of each kind: consumers care
    /// which kind of type variable it is, not how it was declared.
    fn typevar_kind_str(kind: TypeVarKind) -> &'static str {
        match kind {
            TypeVarKind::LegacyTypeVar | TypeVarKind::Pep695TypeVar => "TypeVar",
            TypeVarKind::TypingSelf => "Self",
            TypeVarKind::LegacyParamSpec | TypeVarKind::Pep695ParamSpec => "ParamSpec",
            TypeVarKind::LegacyTypeVarTuple | TypeVarKind::Pep695TypeVarTuple => "TypeVarTuple",
            TypeVarKind::Pep613Alias => "TypeAlias",
        }
    }

    /// Covers tuple subclasses as well as `tuple` itself, since their spec comes
    /// from the MRO rather than the class's own specialization.
    fn build_tuple_elements(
        &mut self,
        instance: NominalInstanceType<'db>,
        db: &'db dyn Db,
        env: &ProgramEnvironment<'db>,
    ) -> Option<Vec<TupleElementInfo>> {
        let spec = instance.tuple_spec(db, env)?;

        Some(match spec.as_ref() {
            Tuple::Fixed(tuple) => self.register_fixed_elements(tuple.all_elements(), db),
            Tuple::Variable(tuple) => {
                let mut elements = self.register_fixed_elements(tuple.prefix_elements(), db);
                let (variable_ty, kind) = match tuple.variable() {
                    VariableSegment::Homogeneous(element) => (element, "homogeneous"),
                    VariableSegment::TypeVarTuple(tv) => (Type::TypeVar(tv), "typeVarTuple"),
                };
                elements.push(TupleElementInfo {
                    type_id: self.register_component(variable_ty, db),
                    kind,
                });
                elements.extend(self.register_fixed_elements(tuple.suffix_elements(), db));
                elements
            }
        })
    }

    fn register_fixed_elements(
        &mut self,
        elements: &[Type<'db>],
        db: &'db dyn Db,
    ) -> Vec<TupleElementInfo> {
        elements
            .iter()
            .map(|&element| TupleElementInfo {
                type_id: self.register_component(element, db),
                kind: "fixed",
            })
            .collect()
    }

    fn known_instance_kind_str(ki: KnownInstanceType<'db>) -> &'static str {
        match ki {
            KnownInstanceType::SubscriptedProtocol(_) => "SubscriptedProtocol",
            KnownInstanceType::SubscriptedGeneric(_) => "SubscriptedGeneric",
            KnownInstanceType::TypeVar(_) => "TypeVar",
            KnownInstanceType::TypeAliasType(_) => "TypeAliasType",
            KnownInstanceType::Deprecated(_) => "Deprecated",
            KnownInstanceType::Field(_) => "Field",
            KnownInstanceType::ConstraintSet(_) => "ConstraintSet",
            KnownInstanceType::ConstraintSetSolution(_) => "ConstraintSetSolution",
            KnownInstanceType::GenericContext(_) => "GenericContext",
            KnownInstanceType::Specialization(_) => "Specialization",
            KnownInstanceType::UnionType(_) => "UnionType",
            KnownInstanceType::Literal(_) => "Literal",
            KnownInstanceType::Annotated(_) => "Annotated",
            KnownInstanceType::TypeGenericAlias(_) => "TypeGenericAlias",
            KnownInstanceType::Callable(_) => "Callable",
            KnownInstanceType::LiteralStringAlias(_) => "LiteralStringAlias",
            KnownInstanceType::NewType(_) => "NewType",
            KnownInstanceType::Sentinel(_) => "Sentinel",
            KnownInstanceType::NamedTupleSpec(_) => "NamedTupleSpec",
            KnownInstanceType::FunctoolsPartial(_) => "FunctoolsPartial",
            KnownInstanceType::Range { .. } => "Range",
            KnownInstanceType::FunctoolsPartialCall(_) => "FunctoolsPartialCall",
        }
    }

    fn build_descriptor(&mut self, ty: Type<'db>, db: &'db dyn Db) -> TypeDescriptor {
        let env = self.env.clone();
        let python_version = env.python_version(db);
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
                        let value = format!("{}", ty.display(db, &env));
                        TypeDescriptor::BytesLiteral { display, value }
                    }
                    LiteralValueTypeKind::LiteralString => {
                        TypeDescriptor::LiteralString { display }
                    }
                    LiteralValueTypeKind::Enum(e) => {
                        let enum_class = e.enum_class(db);
                        TypeDescriptor::EnumLiteral {
                            display,
                            class_name: enum_class.name(db).to_string(),
                            qualified_name: identifying(enum_class.qualified_name(db).to_string()),
                            member_name: e.name(db).to_string(),
                        }
                    }
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
                let cl = instance.class_literal(db, &env);
                let class_name = cl.name(db).to_string();
                let module_name = self.resolve_module_name(db, cl.file(db));
                let qualified_name = identifying(cl.qualified_name(db).to_string());

                let supertypes = self.supertypes_from_class_literal(cl, db);

                // Extract type arguments from specialization
                let class_type = instance.class(db, &env);
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
                let class_id = Some(self.register_component(Type::ClassLiteral(cl), db));

                let tuple_elements = self.build_tuple_elements(instance, db, &env);

                TypeDescriptor::Instance {
                    display,
                    class_name,
                    module_name,
                    qualified_name,
                    supertypes,
                    type_args,
                    class_id,
                    tuple_elements,
                }
            }

            Type::ProtocolInstance(instance) => {
                let display = self.display_string(ty, db);
                if let Some(nominal) = instance.nominal_origin_instance(db) {
                    let cl = nominal.class_literal(db, &env);
                    let class_name = cl.name(db).to_string();
                    let module_name = self.resolve_module_name(db, cl.file(db));
                    let qualified_name = identifying(cl.qualified_name(db).to_string());

                    let supertypes = self.supertypes_from_class_literal(cl, db);

                    let class_type = nominal.class(db, &env);
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

                    let class_id = Some(self.register_component(Type::ClassLiteral(cl), db));

                    let tuple_elements = self.build_tuple_elements(nominal, db, &env);

                    TypeDescriptor::Instance {
                        display,
                        class_name,
                        module_name,
                        qualified_name,
                        supertypes,
                        type_args,
                        class_id,
                        tuple_elements,
                    }
                } else {
                    // Synthesized protocols have no class backing
                    let class_name = format!("{}", ty.display(db, &env));
                    TypeDescriptor::Instance {
                        display,
                        class_name,
                        module_name: None,
                        qualified_name: None,
                        supertypes: vec![],
                        type_args: vec![],
                        class_id: None,
                        tuple_elements: None,
                    }
                }
            }

            Type::ClassLiteral(class_literal) => {
                let display = self.display_string(ty, db);
                let class_name = class_literal.name(db).to_string();
                let module_name = self.resolve_module_name(db, class_literal.file(db));
                let qualified_name = identifying(class_literal.qualified_name(db).to_string());
                let type_parameters =
                    self.build_type_parameters(class_literal.generic_context(db), db);
                let supertypes = self.supertypes_from_class_literal(class_literal, db);

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
                    module_name,
                    qualified_name,
                    type_parameters,
                    supertypes,
                    members,
                }
            }

            Type::GenericAlias(alias) => {
                let display = self.display_string(ty, db);
                let origin = alias.origin(db);
                let class_name = origin.name(db).to_string();
                let module_name = self.resolve_module_name(db, origin.file(db));
                let qualified_name =
                    identifying(ClassLiteral::Static(origin).qualified_name(db).to_string());
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
                    module_name,
                    qualified_name,
                    type_parameters: vec![],
                    supertypes,
                    members,
                }
            }

            Type::SubclassOf(subclass_of_ty) => {
                let display = self.display_string(ty, db);
                // Every arm registers the constraint's operand. `ty` already holds
                // an id, so registering it here would point the descriptor at itself.
                let base = match subclass_of_ty.subclass_of() {
                    SubclassOfInner::Class(class_ty) => {
                        self.register_component(Type::ClassLiteral(class_ty.class_literal(db)), db)
                    }
                    // `type[SomeProtocol]` gets the same `classLiteral` base as a
                    // nominal class. Synthesized protocols have no class to name.
                    SubclassOfInner::Protocol(proto) => match proto.class_origin(db) {
                        Some(proto_class) => self.register_component(
                            Type::ClassLiteral(proto_class.class_literal(db)),
                            db,
                        ),
                        None => self.register_component(Type::ProtocolInstance(proto), db),
                    },
                    SubclassOfInner::Dynamic(dynamic) => {
                        self.register_component(Type::Dynamic(dynamic), db)
                    }
                    SubclassOfInner::TypeVar(bound_tv) => {
                        self.register_component(Type::TypeVar(bound_tv), db)
                    }
                };
                TypeDescriptor::SubclassOf { display, base }
            }

            Type::TypeForm(typeform) => {
                let display = self.display_string(ty, db);
                let type_argument = self.register_component(typeform.type_argument(db), db);
                TypeDescriptor::TypeForm {
                    display,
                    type_argument,
                }
            }

            Type::FunctionLiteral(func) => {
                let display = self.display_string(ty, db);
                let name = func.name(db).to_string();
                let module_name = self.resolve_module_name(db, func.file(db));
                let (type_parameters, parameters, return_type) = self.build_function_params(ty, db);
                TypeDescriptor::Function {
                    display,
                    name,
                    module_name,
                    type_parameters,
                    parameters,
                    return_type,
                }
            }

            Type::Callable(callable_ty) => {
                let display = self.display_string(ty, db);
                let sigs = callable_ty.signatures(db);
                if let Some(sig) = sigs.iter().next() {
                    let (_type_params, parameters, return_type) =
                        self.build_params_from_signature(sig, db);
                    TypeDescriptor::Callable {
                        display,
                        parameters,
                        return_type,
                    }
                } else {
                    TypeDescriptor::Callable {
                        display,
                        parameters: vec![],
                        return_type: None,
                    }
                }
            }

            Type::BoundMethod(bound) => {
                let display = self.display_string(ty, db);
                let func = bound.function(db);
                let func_ty = Type::FunctionLiteral(func);
                let name = Some(func.name(db).to_string());
                // Derive class name from the self_instance type
                let class_name = match bound.self_instance(db) {
                    Type::NominalInstance(inst) => {
                        Some(inst.class_literal(db, &env).name(db).to_string())
                    }
                    Type::ProtocolInstance(inst) => inst
                        .nominal_origin_instance(db)
                        .map(|n| n.class_literal(db, &env).name(db).to_string()),
                    _ => None,
                };
                let module_name = self.resolve_module_name(db, func.file(db));
                let (type_parameters, parameters, return_type) =
                    self.build_function_params(func_ty, db);
                TypeDescriptor::BoundMethod {
                    display,
                    name,
                    class_name,
                    module_name,
                    type_parameters,
                    parameters,
                    return_type,
                }
            }

            Type::KnownBoundMethod(known_bound) => {
                let display = self.display_string(ty, db);
                let class_name = Some(known_bound.class().name(python_version).to_string());
                let sigs: Vec<_> = known_bound.signatures(db, &env).collect();
                let (type_parameters, parameters, return_type) = sigs
                    .first()
                    .map(|sig| self.build_params_from_signature(sig, db))
                    .unwrap_or((vec![], vec![], None));
                TypeDescriptor::BoundMethod {
                    display,
                    name: None,
                    class_name,
                    module_name: None,
                    type_parameters,
                    parameters,
                    return_type,
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

            Type::TypeVar(bound_tv) => {
                let display = self.display_string(ty, db);
                let name = bound_tv.name(db).to_string();
                let kind = bound_tv.kind(db);
                let typevar_kind = Some(Self::typevar_kind_str(kind).to_string());

                let typevar = bound_tv.typevar(db);

                let variance = Some(
                    match bound_tv.variance(db) {
                        TypeVarVariance::Covariant => "covariant",
                        TypeVarVariance::Contravariant => "contravariant",
                        TypeVarVariance::Invariant | TypeVarVariance::Bivariant => "invariant",
                    }
                    .to_string(),
                );

                let upper_bound = typevar
                    .upper_bound(db, &env)
                    .map(|bound| self.register_component(bound, db));

                let constraints: Vec<_> = typevar
                    .constraints(db, &env)
                    .map(|cs| cs.iter().map(|&c| self.register_component(c, db)).collect())
                    .unwrap_or_default();

                let default_type = typevar
                    .default_type(db, &env)
                    .map(|dt| self.register_component(dt, db));

                TypeDescriptor::TypeVar {
                    display,
                    name,
                    typevar_kind,
                    variance,
                    upper_bound,
                    constraints,
                    default_type,
                }
            }

            Type::TypeAlias(type_alias) => {
                let display = self.display_string(ty, db);
                let name = type_alias.name(db).to_string();
                let value_ty = type_alias.value_type(db);
                let value_type = if matches!(value_ty, Type::Dynamic(_)) {
                    None
                } else {
                    Some(self.register_component(value_ty, db))
                };
                let qualified_name = identifying(type_alias.qualified_name(db).to_string());
                let type_parameters =
                    self.build_type_parameters(type_alias.generic_context(db), db);
                TypeDescriptor::TypeAlias {
                    display,
                    name,
                    qualified_name,
                    value_type,
                    type_parameters,
                }
            }

            Type::TypedDict(typed_dict) => {
                let display = self.display_string(ty, db);
                let defining_class = typed_dict.defining_class();
                let name = defining_class
                    .map(|c| c.name(db).to_string())
                    .unwrap_or_default();
                let qualified_name =
                    defining_class.and_then(|c| identifying(c.qualified_name(db).to_string()));
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
                // PEP 728 openness: `Closed` forbids undeclared keys, `Extra` exposes
                // them with a declared type and mutability. `ImplicitlyOpen` (the
                // default) carries neither flag.
                let openness = typed_dict.openness(db);
                let closed = openness.is_closed();
                let extra_items = openness.explicit_extra_items().map(|extra| {
                    let type_id = self.register_component(extra.declared_ty, db);
                    TypedDictExtraItemsInfo {
                        type_id,
                        read_only: extra.is_read_only(),
                    }
                });
                TypeDescriptor::TypedDict {
                    display,
                    name,
                    qualified_name,
                    fields,
                    closed,
                    extra_items,
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
                // A `NewType` has no defining class to ask, so walk its definition's
                // enclosing scopes — the primitive `TypeAliasType::qualified_name`
                // builds on. Definitions sit directly in that scope, so skip none.
                let definition = newtype.definition(db);
                let mut components = qualified_name_components_from_scope(
                    db,
                    definition.program_file(db),
                    definition.file_scope(db),
                    0,
                );
                components.push(name.clone());
                let base_type = self.register_component(newtype.concrete_base_type(db), db);
                TypeDescriptor::NewType {
                    display,
                    name,
                    qualified_name: Some(components.join(".")),
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

            Type::KnownInstance(ki) => {
                let display = self.display_string(ty, db);
                let class_name = ki.class(db).name(python_version).to_string();

                let is_non_empty = match ki {
                    KnownInstanceType::Range { is_non_empty } => Some(is_non_empty),
                    _ => None,
                };

                // `FunctoolsPartialCall` is the bound `__call__` of a partial, so
                // both carry the same wrapped callable and residual signature.
                let partial = match ki {
                    KnownInstanceType::FunctoolsPartial(p)
                    | KnownInstanceType::FunctoolsPartialCall(p) => Some(p),
                    _ => None,
                };
                let wrapped_type =
                    partial.map(|p| self.register_component(p.wrapped(db).inner(db), db));
                let (parameters, return_type) = partial
                    .and_then(|p| p.partial(db).signatures(db).iter().next())
                    .map(|sig| {
                        let (_, params, ret) = self.build_params_from_signature(sig, db);
                        (params, ret)
                    })
                    .unwrap_or((vec![], None));

                TypeDescriptor::KnownInstance {
                    display,
                    class_name,
                    known_instance_kind: Self::known_instance_kind_str(ki),
                    is_non_empty,
                    wrapped_type,
                    parameters,
                    return_type,
                }
            }

            Type::WrapperDescriptor(wrapper_kind) => {
                let display = self.display_string(ty, db);
                let descriptor_kind = format!("{wrapper_kind:?}");
                let sigs: Vec<_> = wrapper_kind.signatures(db, &env).collect();
                let (_type_params, parameters, return_type) = sigs
                    .first()
                    .map(|sig| self.build_params_from_signature(sig, db))
                    .unwrap_or((vec![], vec![], None));
                TypeDescriptor::WrapperDescriptor {
                    display,
                    descriptor_kind,
                    parameters,
                    return_type,
                }
            }

            Type::EnumComplement(complement) => {
                let display = self.display_string(ty, db);
                let enum_class = complement.enum_class(db);
                let class_name = enum_class.name(db).to_string();
                let module_name = self.resolve_module_name(db, enum_class.file(db));
                let qualified_name = identifying(enum_class.qualified_name(db).to_string());
                let class_id = self.register_component(Type::ClassLiteral(enum_class), db);
                let excluded_names = complement
                    .excluded_names(db)
                    .iter()
                    .map(|n| n.to_string())
                    .collect();
                let rest = complement
                    .rest(db)
                    .iter()
                    .map(|&t| self.register_component(t, db))
                    .collect();
                TypeDescriptor::EnumComplement {
                    display,
                    class_name,
                    module_name,
                    qualified_name,
                    class_id,
                    excluded_names,
                    rest,
                }
            }

            Type::DataclassDecorator(_)
            | Type::DataclassTransformer(_)
            | Type::SlotDescriptor(_)
            | Type::BoundSuper(_)
            | Type::Divergent(_) => {
                let display = self.display_string(ty, db);
                TypeDescriptor::Other { display }
            }
        }
    }
}
