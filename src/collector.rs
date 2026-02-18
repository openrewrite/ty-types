use std::collections::HashMap;

use ruff_python_ast::{
    self as ast, visitor::source_order, visitor::source_order::SourceOrderVisitor,
};
use ruff_text_size::Ranged;
use ty_python_semantic::types::ParameterKind;
use ty_python_semantic::types::ide_support::{
    call_signature_details, find_active_signature_from_details,
};
use ty_python_semantic::{Db, HasType, SemanticModel};

use crate::protocol::{CallSignatureInfo, NodeAttribution, ParameterInfo, TypeDescriptor, TypeId};
use crate::registry::TypeRegistry;

pub struct CollectionResult {
    pub nodes: Vec<NodeAttribution>,
    pub new_types: HashMap<TypeId, TypeDescriptor>,
}

pub fn collect_types<'db>(
    db: &'db dyn Db,
    file: ruff_db::files::File,
    registry: &mut TypeRegistry<'db>,
) -> CollectionResult {
    let ast = ruff_db::parsed::parsed_module(db, file).load(db);

    let mut collector = TypeCollector {
        model: SemanticModel::new(db, file),
        db,
        registry,
        nodes: Vec::new(),
        new_type_ids: Vec::new(),
    };

    collector.visit_body(ast.suite());

    let new_types: HashMap<TypeId, TypeDescriptor> = collector
        .new_type_ids
        .iter()
        .filter_map(|&id| {
            collector
                .registry
                .get_descriptor(id)
                .map(|d| (id, d.clone()))
        })
        .collect();

    CollectionResult {
        nodes: collector.nodes,
        new_types,
    }
}

struct TypeCollector<'db, 'reg> {
    model: SemanticModel<'db>,
    db: &'db dyn Db,
    registry: &'reg mut TypeRegistry<'db>,
    nodes: Vec<NodeAttribution>,
    new_type_ids: Vec<TypeId>,
}

impl<'db, 'reg> TypeCollector<'db, 'reg> {
    fn record_node(
        &mut self,
        node_kind: &str,
        range: ruff_text_size::TextRange,
        type_id: Option<TypeId>,
    ) {
        self.nodes.push(NodeAttribution {
            start: range.start().into(),
            end: range.end().into(),
            node_kind: node_kind.to_string(),
            type_id,
            call_signature: None,
        });
    }

    fn record_call_node(
        &mut self,
        range: ruff_text_size::TextRange,
        type_id: Option<TypeId>,
        call_signature: Option<CallSignatureInfo>,
    ) {
        self.nodes.push(NodeAttribution {
            start: range.start().into(),
            end: range.end().into(),
            node_kind: "ExprCall".to_string(),
            type_id,
            call_signature,
        });
    }

    fn register_type(&mut self, ty: ty_python_semantic::types::Type<'db>) -> TypeId {
        let result = self.registry.register(ty, self.db);
        if result.is_new {
            self.new_type_ids.push(result.type_id);
        }
        result.type_id
    }

    fn build_call_signature(
        &mut self,
        call_expr: &ast::ExprCall,
        return_type_id: Option<TypeId>,
    ) -> Option<CallSignatureInfo> {
        let signatures = call_signature_details(&self.model, call_expr);
        if signatures.is_empty() {
            return None;
        }

        let active_idx = find_active_signature_from_details(&signatures).unwrap_or(0);
        let sig = &signatures[active_idx];

        let parameters: Vec<ParameterInfo> = sig
            .parameter_names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let type_id = sig.parameter_types.get(i).map(|&ty| self.register_type(ty));

                let (kind, has_default) = if let Some(pk) = sig.parameter_kinds.get(i) {
                    match pk {
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
                    }
                } else {
                    ("positionalOrKeyword", false)
                };

                ParameterInfo {
                    name: name.clone(),
                    type_id,
                    kind,
                    has_default,
                }
            })
            .collect();

        Some(CallSignatureInfo {
            parameters,
            return_type_id,
        })
    }

    fn visit_target(&mut self, target: &ast::Expr) {
        match target {
            ast::Expr::List(ast::ExprList { elts, .. })
            | ast::Expr::Tuple(ast::ExprTuple { elts, .. }) => {
                for element in elts {
                    self.visit_target(element);
                }
            }
            _ => self.visit_expr(target),
        }
    }
}

impl SourceOrderVisitor<'_> for TypeCollector<'_, '_> {
    fn visit_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(function) => {
                if let Some(ty) = function.inferred_type(&self.model) {
                    let type_id = self.register_type(ty);
                    self.record_node("StmtFunctionDef", function.range(), Some(type_id));
                } else {
                    self.record_node("StmtFunctionDef", function.range(), None);
                }
            }
            ast::Stmt::ClassDef(class) => {
                if let Some(ty) = class.inferred_type(&self.model) {
                    let type_id = self.register_type(ty);
                    self.record_node("StmtClassDef", class.range(), Some(type_id));
                } else {
                    self.record_node("StmtClassDef", class.range(), None);
                }
            }
            ast::Stmt::Assign(assign) => {
                self.record_node("StmtAssign", assign.range(), None);
                for target in &assign.targets {
                    self.visit_target(target);
                }
                self.visit_expr(&assign.value);
                return;
            }
            ast::Stmt::For(for_stmt) => {
                self.record_node("StmtFor", for_stmt.range(), None);
                self.visit_target(&for_stmt.target);
                self.visit_expr(&for_stmt.iter);
                self.visit_body(&for_stmt.body);
                self.visit_body(&for_stmt.orelse);
                return;
            }
            ast::Stmt::With(with_stmt) => {
                self.record_node("StmtWith", with_stmt.range(), None);
                for item in &with_stmt.items {
                    if let Some(target) = &item.optional_vars {
                        self.visit_target(target);
                    }
                    self.visit_expr(&item.context_expr);
                }
                self.visit_body(&with_stmt.body);
                return;
            }
            _ => {}
        }

        source_order::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &ast::Expr) {
        let node_kind = expr_kind_name(expr);

        if let Some(ty) = expr.inferred_type(&self.model) {
            let type_id = self.register_type(ty);

            if let ast::Expr::Call(call_expr) = expr {
                let call_sig = self.build_call_signature(call_expr, Some(type_id));
                self.record_call_node(expr.range(), Some(type_id), call_sig);
            } else {
                self.record_node(node_kind, expr.range(), Some(type_id));
            }
        } else if let ast::Expr::Call(call_expr) = expr {
            let call_sig = self.build_call_signature(call_expr, None);
            self.record_call_node(expr.range(), None, call_sig);
        } else {
            self.record_node(node_kind, expr.range(), None);
        }

        source_order::walk_expr(self, expr);
    }

    fn visit_comprehension(&mut self, comprehension: &ast::Comprehension) {
        self.visit_expr(&comprehension.iter);
        self.visit_target(&comprehension.target);
        for if_expr in &comprehension.ifs {
            self.visit_expr(if_expr);
        }
    }

    fn visit_parameter(&mut self, parameter: &ast::Parameter) {
        if let Some(ty) = parameter.inferred_type(&self.model) {
            let type_id = self.register_type(ty);
            self.record_node("Parameter", parameter.range(), Some(type_id));
        } else {
            self.record_node("Parameter", parameter.range(), None);
        }

        source_order::walk_parameter(self, parameter);
    }

    fn visit_parameter_with_default(&mut self, parameter_with_default: &ast::ParameterWithDefault) {
        if let Some(ty) = parameter_with_default.inferred_type(&self.model) {
            let type_id = self.register_type(ty);
            self.record_node(
                "ParameterWithDefault",
                parameter_with_default.range(),
                Some(type_id),
            );
        } else {
            self.record_node("ParameterWithDefault", parameter_with_default.range(), None);
        }

        source_order::walk_parameter_with_default(self, parameter_with_default);
    }

    fn visit_alias(&mut self, alias: &ast::Alias) {
        if let Some(ty) = alias.inferred_type(&self.model) {
            let type_id = self.register_type(ty);
            self.record_node("Alias", alias.range(), Some(type_id));
        } else {
            self.record_node("Alias", alias.range(), None);
        }

        source_order::walk_alias(self, alias);
    }
}

fn expr_kind_name(expr: &ast::Expr) -> &'static str {
    match expr {
        ast::Expr::BoolOp(_) => "ExprBoolOp",
        ast::Expr::Named(_) => "ExprNamed",
        ast::Expr::BinOp(_) => "ExprBinOp",
        ast::Expr::UnaryOp(_) => "ExprUnaryOp",
        ast::Expr::Lambda(_) => "ExprLambda",
        ast::Expr::If(_) => "ExprIf",
        ast::Expr::Dict(_) => "ExprDict",
        ast::Expr::Set(_) => "ExprSet",
        ast::Expr::ListComp(_) => "ExprListComp",
        ast::Expr::SetComp(_) => "ExprSetComp",
        ast::Expr::DictComp(_) => "ExprDictComp",
        ast::Expr::Generator(_) => "ExprGenerator",
        ast::Expr::Await(_) => "ExprAwait",
        ast::Expr::Yield(_) => "ExprYield",
        ast::Expr::YieldFrom(_) => "ExprYieldFrom",
        ast::Expr::Compare(_) => "ExprCompare",
        ast::Expr::Call(_) => "ExprCall",
        ast::Expr::FString(_) => "ExprFString",
        ast::Expr::TString(_) => "ExprTString",
        ast::Expr::StringLiteral(_) => "ExprStringLiteral",
        ast::Expr::BytesLiteral(_) => "ExprBytesLiteral",
        ast::Expr::NumberLiteral(_) => "ExprNumberLiteral",
        ast::Expr::BooleanLiteral(_) => "ExprBooleanLiteral",
        ast::Expr::NoneLiteral(_) => "ExprNoneLiteral",
        ast::Expr::EllipsisLiteral(_) => "ExprEllipsisLiteral",
        ast::Expr::Attribute(_) => "ExprAttribute",
        ast::Expr::Subscript(_) => "ExprSubscript",
        ast::Expr::Starred(_) => "ExprStarred",
        ast::Expr::Name(_) => "ExprName",
        ast::Expr::List(_) => "ExprList",
        ast::Expr::Tuple(_) => "ExprTuple",
        ast::Expr::Slice(_) => "ExprSlice",
        ast::Expr::IpyEscapeCommand(_) => "ExprIpyEscapeCommand",
    }
}
