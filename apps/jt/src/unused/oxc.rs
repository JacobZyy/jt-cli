use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::{
    AstKind,
    ast::{
        ArrayExpressionElement, Declaration, ExportDefaultDeclarationKind, Expression,
        ObjectPropertyKind, Program, Statement, TSAccessibility,
    },
};
use oxc_parser::Parser;
use oxc_resolver::{AliasValue, ResolveOptions, Resolver, TsconfigOptions, TsconfigReferences};
use oxc_semantic::{SemanticBuilder, SymbolFlags};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::{
    ExportExportName, ExportImportName, ExportLocalName, ImportImportName,
};
use oxc_syntax::node::NodeId;

/// Source passed to the Oxc scanner.
///
/// `offset` is the byte offset of `content` in the original file. It is zero for
/// normal JavaScript/TypeScript files and non-zero for a Vue script block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceBlock {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) offset: usize,
    pub(crate) lang: String,
    pub(crate) collect_candidates: bool,
}

impl SourceBlock {
    pub(crate) fn new<P, C, L>(path: P, content: C, offset: usize, lang: L) -> Self
    where
        P: Into<String>,
        C: Into<String>,
        L: Into<String>,
    {
        Self {
            path: path.into(),
            content: content.into(),
            offset,
            lang: lang.into(),
            collect_candidates: true,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn consumer_only<P, C, L>(path: P, content: C, offset: usize, lang: L) -> Self
    where
        P: Into<String>,
        C: Into<String>,
        L: Into<String>,
    {
        Self {
            path: path.into(),
            content: content.into(),
            offset,
            lang: lang.into(),
            collect_candidates: false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_candidate_collection(mut self, collect_candidates: bool) -> Self {
        self.collect_candidates = collect_candidates;
        self
    }
}

/// Runtime owner for references and calls.
///
/// Owners which are not finding candidates (for example a constructor, field
/// initializer, or callback) still participate in reachability analysis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionOwner {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) finding_candidate_id: Option<String>,
    pub(crate) parent_id: Option<String>,
}

/// Symbol candidate found in one source block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) local_used: bool,
    pub(crate) exported: bool,
    pub(crate) reexport_locations: Vec<String>,
    pub(crate) unknown: bool,
    pub(crate) coverage_reason: Option<String>,
    pub(crate) language: String,
    pub(crate) qualified_name: String,
    pub(crate) top_level: bool,
    pub(crate) callable: bool,
    pub(crate) initializer_owner: Option<String>,
    pub(crate) initializer_effect: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportSummary {
    pub(crate) module: String,
    pub(crate) import_name: Option<String>,
    pub(crate) local_name: Option<String>,
    pub(crate) local_used: bool,
    pub(crate) usages: Vec<ImportUsage>,
    pub(crate) reexport_only: bool,
    pub(crate) type_only: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportUsage {
    pub(crate) source: String,
    pub(crate) member_name: Option<String>,
    pub(crate) dynamic_member: bool,
    pub(crate) kind: String,
    pub(crate) mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportSummary {
    pub(crate) export_name: Option<String>,
    pub(crate) local_name: Option<String>,
    pub(crate) imported_name: Option<String>,
    pub(crate) module: Option<String>,
    pub(crate) local_id: Option<String>,
    pub(crate) type_only: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StarExportSummary {
    pub(crate) module: String,
    pub(crate) type_only: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DynamicPattern {
    pub(crate) prefix: String,
    pub(crate) suffix: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DynamicImportSummary {
    pub(crate) source: String,
    pub(crate) expression: String,
    pub(crate) static_specifier: Option<String>,
    pub(crate) pattern: Option<DynamicPattern>,
    pub(crate) unbounded: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequireSummary {
    pub(crate) source: String,
    pub(crate) static_specifier: Option<String>,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModuleSummary {
    pub(crate) path: String,
    pub(crate) default_export_owner: Option<String>,
    pub(crate) imports: Vec<ImportSummary>,
    pub(crate) local_exports: Vec<ExportSummary>,
    pub(crate) reexports: Vec<ExportSummary>,
    pub(crate) star_exports: Vec<StarExportSummary>,
    pub(crate) dynamic_imports: Vec<DynamicImportSummary>,
    pub(crate) requires: Vec<RequireSummary>,
    pub(crate) has_parse_errors: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GraphEdge {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) confidence: String,
    pub(crate) mode: String,
    pub(crate) provenance: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScanResult {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) modules: Vec<ModuleSummary>,
    pub(crate) used_files: BTreeSet<String>,
    pub(crate) unknown_files: BTreeSet<String>,
    pub(crate) dynamic_unknown: BTreeSet<String>,
    pub(crate) file_reexports: BTreeMap<String, Vec<ReexportLocation>>,
    pub(crate) public_exports: BTreeMap<String, BTreeSet<String>>,
    pub(crate) file_effects: BTreeMap<String, String>,
    pub(crate) edges: Vec<GraphEdge>,
    pub(crate) execution_owners: Vec<ExecutionOwner>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) coverage_boundaries: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReexportLocation {
    pub(crate) source: String,
    pub(crate) line: usize,
    pub(crate) type_only: bool,
}

impl ReexportLocation {
    pub(crate) fn display(&self) -> String {
        format!("{}:{}", self.source, self.line)
    }
}

struct ParsedBlock {
    candidates: Vec<Candidate>,
    module: ModuleSummary,
    edges: Vec<GraphEdge>,
    execution_owners: Vec<ExecutionOwner>,
    file_effect: String,
    diagnostics: Vec<String>,
}

/// Parse supplied JavaScript, TypeScript, and Vue script blocks, then link the
/// module graph using Oxc's resolver.
pub(crate) fn scan(root: &Path, blocks: &[SourceBlock]) -> ScanResult {
    let mut result = ScanResult::default();
    let mut modules = HashMap::<String, ModuleSummary>::new();
    let mut candidates = HashMap::<String, Candidate>::new();

    for block in blocks {
        let parsed = parse_block(root, block);
        result
            .file_effects
            .entry(parsed.module.path.clone())
            .and_modify(|current| *current = merge_initializer_effect(current, &parsed.file_effect))
            .or_insert_with(|| parsed.file_effect.clone());
        result.diagnostics.extend(parsed.diagnostics);
        result.edges.extend(parsed.edges);
        result.execution_owners.extend(parsed.execution_owners);
        for candidate in parsed.candidates {
            candidates
                .entry(candidate.id.clone())
                .and_modify(|current| {
                    current.local_used |= candidate.local_used;
                    current.exported |= candidate.exported;
                    current.unknown |= candidate.unknown;
                    if current.coverage_reason.is_none() {
                        current.coverage_reason = candidate.coverage_reason.clone();
                    }
                    current.callable |= candidate.callable;
                    current.top_level |= candidate.top_level;
                    if current.initializer_owner.is_none() {
                        current.initializer_owner = candidate.initializer_owner.clone();
                    }
                    current.initializer_effect = merge_initializer_effect(
                        &current.initializer_effect,
                        &candidate.initializer_effect,
                    )
                })
                .or_insert(candidate);
        }
        modules
            .entry(parsed.module.path.clone())
            .and_modify(|current| merge_module(current, parsed.module.clone()))
            .or_insert(parsed.module);
    }

    result.candidates = candidates.into_values().collect();
    result.modules = modules.into_values().collect();
    result.candidates.sort_by(|left, right| {
        (
            left.path.as_str(),
            left.start,
            left.column,
            left.kind.as_str(),
            left.name.as_str(),
        )
            .cmp(&(
                right.path.as_str(),
                right.start,
                right.column,
                right.kind.as_str(),
                right.name.as_str(),
            ))
    });
    result
        .modules
        .sort_by(|left, right| left.path.cmp(&right.path));
    result.execution_owners.sort_by(|left, right| {
        (
            left.path.as_str(),
            left.start,
            left.kind.as_str(),
            left.id.as_str(),
        )
            .cmp(&(
                right.path.as_str(),
                right.start,
                right.kind.as_str(),
                right.id.as_str(),
            ))
    });
    result
        .execution_owners
        .dedup_by(|left, right| left.id == right.id);
    link_modules(root, &mut result);
    result.edges.sort_by(|left, right| {
        (
            left.source.as_str(),
            left.target.as_str(),
            left.kind.as_str(),
            left.path.as_str(),
            left.start,
        )
            .cmp(&(
                right.source.as_str(),
                right.target.as_str(),
                right.kind.as_str(),
                right.path.as_str(),
                right.start,
            ))
    });
    result.edges.dedup();
    result
}

fn parse_block(root: &Path, block: &SourceBlock) -> ParsedBlock {
    let path = normalize_path(root, Path::new(&block.path));
    let source_type = source_type(block);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &block.content, source_type).parse();
    let parser_error_count = parsed.errors.len();
    let parser_panicked = parsed.panicked;
    let module_record = parsed.module_record;
    let program = allocator.alloc(parsed.program);
    let file_effect = top_level_effect(program);
    let semantic_result = SemanticBuilder::new().build(program);
    let semantic_error_count = semantic_result.errors.len();
    let semantic = semantic_result.semantic;
    let mut candidates = Vec::new();
    let mut symbol_ids = HashMap::<String, String>::new();
    let mut symbol_owner_ids = HashMap::<String, String>::new();
    let mut node_owner_ids = HashMap::<NodeId, String>::new();
    let mut execution_owners = Vec::new();
    let scoping = semantic.scoping();
    let language = language_name(block, source_type);
    let parse_unknown = parser_panicked || parser_error_count > 0 || semantic_error_count > 0;
    let file_owner_id = format!("file::{path}");
    execution_owners.push(execution_owner(
        file_owner_id.clone(),
        "file-top-level",
        block,
        &path,
        0,
        None,
        None,
    ));

    for symbol_id in scoping.symbol_ids() {
        let flags = scoping.symbol_flags(symbol_id);
        if flags.intersects(SymbolFlags::Import | SymbolFlags::TypeImport | SymbolFlags::Ambient) {
            continue;
        }
        let kind = if flags.contains(SymbolFlags::Function) {
            "function"
        } else if flags.contains(SymbolFlags::Class) {
            "class"
        } else if flags.intersects(SymbolFlags::Variable) {
            "variable"
        } else {
            continue;
        };
        let declaration_id = semantic.symbol_declaration(symbol_id).id();
        if !block.collect_candidates
            || !is_finding_declaration(&semantic, symbol_id, declaration_id, kind)
        {
            continue;
        }
        let name = scoping.symbol_name(symbol_id).to_owned();
        let span = scoping.symbol_span(symbol_id);
        let start = block.offset + span.start as usize;
        let (line, column) = line_column(&block.content, span.start as usize);
        let id = candidate_id(&path, kind, &name, start);
        let declaration_function = (kind == "function")
            .then(|| runtime_function_span(&semantic, symbol_id))
            .flatten();
        let local_used = scoping.get_resolved_references(symbol_id).any(|reference| {
            let span = semantic.reference_span(reference);
            (reference.is_read() || reference.is_type())
                && !is_export_reference(span, &module_record)
                && !declaration_function
                    .is_some_and(|declaration| declaration.contains_inclusive(span))
        });
        let exported = scoping.get_root_binding(&name) == Some(symbol_id)
            && symbol_exported(&name, &module_record);
        let top_level = scoping.get_root_binding(&name) == Some(symbol_id);
        symbol_ids.insert(format!("{symbol_id:?}"), id.clone());
        let declaration_kind = semantic.nodes().kind(declaration_id);
        let (callable, initializer_owner, initializer_effect) = match declaration_kind {
            AstKind::Function(function) => {
                let owner_id = id.clone();
                let runtime_span = runtime_function_node(&semantic, symbol_id).map(
                    |(runtime_id, runtime_function)| {
                        node_owner_ids.insert(runtime_id, owner_id.clone());
                        runtime_function
                    },
                );
                node_owner_ids.insert(declaration_id, owner_id.clone());
                execution_owners.push(execution_owner(
                    owner_id,
                    "function",
                    block,
                    &path,
                    runtime_span.map_or(function.span.start as usize, |span| span.start as usize),
                    Some(id.clone()),
                    Some(file_owner_id.clone()),
                ));
                (true, None, "none".to_owned())
            }
            AstKind::Class(class) => {
                let owner_id = id.clone();
                node_owner_ids.insert(declaration_id, owner_id.clone());
                execution_owners.push(execution_owner(
                    owner_id,
                    "class",
                    block,
                    &path,
                    class.span.start as usize,
                    Some(id.clone()),
                    Some(file_owner_id.clone()),
                ));
                (false, None, "none".to_owned())
            }
            AstKind::VariableDeclarator(declarator) => {
                let initializer_owner = declarator.init.as_ref().map(|initializer| {
                    let owner_id = execution_owner_id(
                        &path,
                        "variable-initializer",
                        block.offset + initializer.span().start as usize,
                    );
                    node_owner_ids.insert(declaration_id, owner_id.clone());
                    execution_owners.push(execution_owner(
                        owner_id.clone(),
                        "variable-initializer",
                        block,
                        &path,
                        initializer.span().start as usize,
                        Some(id.clone()),
                        Some(file_owner_id.clone()),
                    ));
                    owner_id
                });
                if let Some(owner_id) = initializer_owner.as_ref() {
                    symbol_owner_ids.insert(format!("{symbol_id:?}"), owner_id.clone());
                }
                let callable = declarator
                    .init
                    .as_ref()
                    .is_some_and(is_callable_initializer);
                let effect = initializer_effect(declarator.init.as_ref());
                (callable, initializer_owner, effect.to_owned())
            }
            _ => (false, None, "none".to_owned()),
        };
        candidates.push(Candidate {
            id,
            kind: kind.to_owned(),
            name: name.clone(),
            path: path.clone(),
            start,
            line,
            column,
            local_used,
            exported,
            reexport_locations: Vec::new(),
            unknown: parse_unknown,
            coverage_reason: None,
            language: language.clone(),
            qualified_name: name,
            top_level,
            callable,
            initializer_owner,
            initializer_effect,
        });
    }

    for node in semantic.nodes() {
        let AstKind::MethodDefinition(method) = node.kind() else {
            continue;
        };
        if !block.collect_candidates
            || method.value.body.is_none()
            || method.r#type == oxc_ast::ast::MethodDefinitionType::TSAbstractMethodDefinition
            || method.value.is_typescript_syntax()
        {
            continue;
        }
        let name = method
            .key
            .name()
            .map_or_else(|| "<computed>".to_owned(), |name| name.into_owned());
        let kind = if name == "constructor" {
            "constructor"
        } else {
            "method"
        };
        let owner_class = semantic
            .nodes()
            .ancestors(node.id())
            .find_map(|ancestor| match ancestor.kind() {
                AstKind::Class(class) => Some(class),
                _ => None,
            });
        let owner = owner_class.and_then(|class| class.id.as_ref()).or_else(|| {
            semantic
                .nodes()
                .ancestors(node.id())
                .find_map(|ancestor| match ancestor.kind() {
                    AstKind::VariableDeclarator(declarator) => {
                        declarator.id.get_binding_identifier()
                    }
                    _ => None,
                })
        });
        let owner_name = owner.map(|identifier| identifier.name.as_str());
        let named_owner_exported = owner.is_some_and(|identifier| {
            scoping
                .get_root_binding(identifier.name.as_str())
                .is_some_and(|symbol_id| scoping.symbol_span(symbol_id) == identifier.span)
                && symbol_exported(identifier.name.as_str(), &module_record)
        });
        let anonymous_default_exported = owner_class.is_some_and(|class| {
            owner.is_none()
                && module_record.local_export_entries.iter().any(|entry| {
                    export_name(&entry.export_name).as_deref() == Some("default")
                        && entry.span.contains_inclusive(class.span)
                })
        });
        let externally_visible = !method.key.is_private_identifier()
            && !method
                .accessibility
                .is_some_and(TSAccessibility::is_private);
        let exported = externally_visible && (named_owner_exported || anonymous_default_exported);
        let span = method.key.span();
        let start = block.offset + span.start as usize;
        let (line, column) = line_column(&block.content, span.start as usize);
        let id = candidate_id(&path, kind, &name, start);
        node_owner_ids.insert(node.id(), id.clone());
        execution_owners.push(execution_owner(
            id.clone(),
            kind,
            block,
            &path,
            method.span.start as usize,
            Some(id.clone()),
            Some(file_owner_id.clone()),
        ));
        candidates.push(Candidate {
            id,
            kind: kind.to_owned(),
            name: name.clone(),
            path: path.clone(),
            start,
            line,
            column,
            local_used: false,
            exported,
            reexport_locations: Vec::new(),
            unknown: true,
            coverage_reason: (!method.decorators.is_empty()
                || (method.computed && method.key.name().is_none()))
            .then(|| "runtime-dispatch-ambiguous".to_owned()),
            language: language.clone(),
            qualified_name: owner_name.map_or_else(
                || format!("<default>.{name}"),
                |owner| format!("{owner}.{name}"),
            ),
            top_level: false,
            callable: true,
            initializer_owner: None,
            initializer_effect: "none".to_owned(),
        });
    }

    let mut object_member_edges = Vec::new();
    for node in semantic.nodes() {
        let AstKind::ObjectProperty(property) = node.kind() else {
            continue;
        };
        if !block.collect_candidates
            || !(property.method || property.kind.is_accessor())
            || !is_callable_expression_with_body(&property.value)
        {
            continue;
        }
        let name = property
            .key
            .name()
            .map_or_else(|| "<computed>".to_owned(), |name| name.into_owned());
        let kind = "method";
        let start = block.offset + property.key.span().start as usize;
        let (line, column) = line_column(&block.content, property.key.span().start as usize);
        let id = candidate_id(&path, kind, &name, start);
        let container_id = semantic.nodes().ancestors(node.id()).find_map(|ancestor| {
            let AstKind::VariableDeclarator(declarator) = ancestor.kind() else {
                return None;
            };
            declarator
                .id
                .get_binding_identifier()
                .and_then(|identifier| identifier.symbol_id.get())
                .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
                .cloned()
        });
        if let Some(container_id) = container_id {
            object_member_edges.push(GraphEdge {
                source: container_id,
                target: id.clone(),
                kind: "reference".to_owned(),
                path: path.clone(),
                start,
                line,
                column,
                confidence: "potential".to_owned(),
                mode: "runtime".to_owned(),
                provenance: "oxc".to_owned(),
            });
        } else if let Some(source) = semantic.nodes().ancestors(node.id()).find_map(|ancestor| {
            let AstKind::CallExpression(call) = ancestor.kind() else {
                return None;
            };
            call.arguments
                .iter()
                .any(|argument| argument.span().contains_inclusive(property.span))
                .then(|| source_owner(&semantic, ancestor.id(), &path, &node_owner_ids))
        }) {
            object_member_edges.push(GraphEdge {
                source,
                target: id.clone(),
                kind: "callback".to_owned(),
                path: path.clone(),
                start,
                line,
                column,
                confidence: "potential".to_owned(),
                mode: "runtime".to_owned(),
                provenance: "oxc".to_owned(),
            });
        }
        node_owner_ids.insert(node.id(), id.clone());
        execution_owners.push(execution_owner(
            id.clone(),
            kind,
            block,
            &path,
            property.span.start as usize,
            Some(id.clone()),
            Some(file_owner_id.clone()),
        ));
        candidates.push(Candidate {
            id,
            kind: kind.to_owned(),
            name: name.clone(),
            path: path.clone(),
            start,
            line,
            column,
            local_used: false,
            exported: false,
            reexport_locations: Vec::new(),
            unknown: true,
            coverage_reason: (property.computed && property.key.name().is_none())
                .then(|| "runtime-dispatch-ambiguous".to_owned()),
            language: language.clone(),
            qualified_name: format!("<object>.{name}"),
            top_level: false,
            callable: true,
            initializer_owner: None,
            initializer_effect: "none".to_owned(),
        });
    }

    collect_non_candidate_owners(
        &semantic,
        block,
        &path,
        &file_owner_id,
        &symbol_ids,
        &symbol_owner_ids,
        &mut node_owner_ids,
        &mut execution_owners,
    );
    collect_default_export_expression_owner(
        program,
        &semantic,
        block,
        &path,
        &file_owner_id,
        &mut node_owner_ids,
        &mut execution_owners,
    );
    repair_owner_parents(
        &semantic,
        &file_owner_id,
        &node_owner_ids,
        &mut execution_owners,
    );

    let mut edges = local_call_edges(
        &semantic,
        &module_record,
        block,
        &path,
        &symbol_ids,
        &node_owner_ids,
        parse_unknown,
    );
    edges.extend(object_member_edges);
    edges.extend(owner_activation_edges(&execution_owners, &path));
    edges.extend(commonjs_export_edges(
        &semantic,
        block,
        &path,
        &symbol_ids,
        &node_owner_ids,
    ));

    let mut module = ModuleSummary {
        path: path.clone(),
        default_export_owner: default_export_owner(program, &semantic, &node_owner_ids),
        imports: Vec::new(),
        local_exports: Vec::new(),
        reexports: Vec::new(),
        star_exports: Vec::new(),
        dynamic_imports: Vec::new(),
        requires: Vec::new(),
        has_parse_errors: parse_unknown,
    };

    for entry in &module_record.import_entries {
        let local_name = entry.local_name.name.to_string();
        let symbol_id = scoping.get_root_binding(&local_name);
        let usages = symbol_id
            .map(|symbol_id| {
                scoping
                    .get_resolved_references(symbol_id)
                    .filter(|reference| {
                        (reference.is_read() || reference.is_type())
                            && !is_export_reference(
                                semantic.reference_span(reference),
                                &module_record,
                            )
                    })
                    .map(|reference| import_usage(&semantic, reference, &path, &node_owner_ids))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let local_used = !usages.is_empty();
        let any_reference = symbol_id
            .is_some_and(|symbol_id| !scoping.get_resolved_references(symbol_id).next().is_none());
        let (line, column) = line_column(&block.content, entry.statement_span.start as usize);
        module.imports.push(ImportSummary {
            module: entry.module_request.name.to_string(),
            import_name: import_name(&entry.import_name),
            local_name: Some(local_name),
            local_used,
            usages,
            reexport_only: any_reference && !local_used,
            type_only: entry.is_type,
            start: block.offset + entry.statement_span.start as usize,
            line,
            column,
        });
    }

    for (module_name, requests) in &module_record.requested_modules {
        for request in requests {
            if !request.is_import {
                continue;
            }
            let has_entry = module.imports.iter().any(|import| {
                import.module == module_name.as_str()
                    && import.start == block.offset + request.statement_span.start as usize
            });
            if has_entry {
                continue;
            }
            let (line, column) = line_column(&block.content, request.statement_span.start as usize);
            module.imports.push(ImportSummary {
                module: module_name.to_string(),
                import_name: None,
                local_name: None,
                local_used: false,
                usages: Vec::new(),
                reexport_only: false,
                type_only: request.is_type,
                start: block.offset + request.statement_span.start as usize,
                line,
                column,
            });
        }
    }

    for entry in &module_record.local_export_entries {
        let local_name = entry.local_name.name().map(|name| name.to_string());
        let local_id = local_name.as_deref().and_then(|name| {
            scoping
                .get_root_binding(name)
                .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
                .cloned()
        });
        let (line, column) = line_column(&block.content, entry.span.start as usize);
        module.local_exports.push(ExportSummary {
            export_name: export_name(&entry.export_name),
            local_name,
            imported_name: None,
            module: None,
            local_id,
            type_only: entry.is_type,
            start: block.offset + entry.span.start as usize,
            line,
            column,
        });
    }

    for entry in &module_record.indirect_export_entries {
        let (Some(module_name), imported_name) = (
            entry
                .module_request
                .as_ref()
                .map(|request| request.name.to_string()),
            export_import_name(&entry.import_name),
        ) else {
            continue;
        };
        let (line, column) = line_column(&block.content, entry.span.start as usize);
        module.reexports.push(ExportSummary {
            export_name: export_name(&entry.export_name),
            local_name: None,
            imported_name,
            module: Some(module_name),
            local_id: None,
            type_only: entry.is_type,
            start: block.offset + entry.span.start as usize,
            line,
            column,
        });
    }

    for entry in &module_record.star_export_entries {
        let Some(module_name) = entry
            .module_request
            .as_ref()
            .map(|request| request.name.to_string())
        else {
            continue;
        };
        let (line, column) = line_column(&block.content, entry.span.start as usize);
        module.star_exports.push(StarExportSummary {
            module: module_name,
            type_only: entry.is_type,
            start: block.offset + entry.span.start as usize,
            line,
            column,
        });
    }

    for dynamic in &module_record.dynamic_imports {
        let source = semantic
            .nodes()
            .iter()
            .find_map(|node| match node.kind() {
                AstKind::ImportExpression(expression)
                    if expression.source.span().start == dynamic.module_request.start
                        && expression.source.span().end == dynamic.module_request.end =>
                {
                    Some(source_owner(&semantic, node.id(), &path, &node_owner_ids))
                }
                _ => None,
            })
            .unwrap_or_else(|| file_owner_id.clone());
        module.dynamic_imports.push(dynamic_import(
            &block.content,
            *dynamic,
            block.offset,
            source,
        ));
    }
    for node in semantic.nodes() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        let Expression::Identifier(callee) = &call.callee else {
            continue;
        };
        if callee.name != "require"
            || callee.reference_id.get().is_some_and(|reference_id| {
                semantic
                    .scoping()
                    .get_reference(reference_id)
                    .symbol_id()
                    .is_some()
            })
        {
            continue;
        }
        let static_specifier = call
            .arguments
            .first()
            .and_then(|argument| argument.as_expression())
            .and_then(|argument| match argument {
                Expression::StringLiteral(literal) => Some(literal.value.to_string()),
                _ => None,
            });
        let start = call.span.start as usize;
        let (line, column) = line_column(&block.content, start);
        module.requires.push(RequireSummary {
            source: source_owner(&semantic, node.id(), &path, &node_owner_ids),
            static_specifier,
            start: block.offset + start,
            line,
            column,
        });
    }

    module
        .imports
        .sort_by_key(|import| (import.start, import.module.clone()));
    module
        .local_exports
        .sort_by_key(|export| (export.start, export.export_name.clone()));
    module
        .reexports
        .sort_by_key(|export| (export.start, export.export_name.clone()));
    module.star_exports.sort_by_key(|export| export.start);
    module.dynamic_imports.sort_by_key(|dynamic| dynamic.start);
    module.requires.sort_by_key(|require| require.start);

    let mut diagnostics = Vec::new();
    if parser_error_count > 0 {
        diagnostics.push(format!(
            "{}: Oxc parser reported {} syntax error{}",
            block.path,
            parser_error_count,
            if parser_error_count == 1 { "" } else { "s" }
        ));
    }
    if semantic_error_count > 0 {
        diagnostics.push(format!(
            "{}: Oxc semantic analysis reported {} error{}",
            block.path,
            semantic_error_count,
            if semantic_error_count == 1 { "" } else { "s" }
        ));
    }
    ParsedBlock {
        candidates,
        module,
        edges,
        execution_owners,
        file_effect,
        diagnostics,
    }
}

fn local_call_edges(
    semantic: &oxc_semantic::Semantic<'_>,
    module_record: &oxc_syntax::module_record::ModuleRecord<'_>,
    block: &SourceBlock,
    path: &str,
    symbol_ids: &HashMap<String, String>,
    node_owner_ids: &HashMap<NodeId, String>,
    parse_unknown: bool,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    for node in semantic.nodes() {
        let (callee, span, kind) = match node.kind() {
            AstKind::CallExpression(call) => (&call.callee, call.span, "call"),
            AstKind::NewExpression(new_expression) => {
                (&new_expression.callee, new_expression.span, "instantiates")
            }
            _ => continue,
        };
        let Expression::Identifier(callee) = callee else {
            continue;
        };
        let Some(reference_id) = callee.reference_id.get() else {
            continue;
        };
        let Some(symbol_id) = semantic.scoping().get_reference(reference_id).symbol_id() else {
            continue;
        };
        let Some(target) = symbol_ids.get(&format!("{symbol_id:?}")).cloned() else {
            continue;
        };
        let source = source_owner(semantic, node.id(), path, node_owner_ids);
        if source == target {
            continue;
        }
        let start = span.start as usize;
        let (line, column) = line_column(&block.content, start);
        edges.push(GraphEdge {
            source,
            target,
            kind: kind.to_owned(),
            path: path.to_owned(),
            start: block.offset + start,
            line,
            column,
            confidence: if parse_unknown { "potential" } else { "exact" }.to_owned(),
            mode: "runtime".to_owned(),
            provenance: "oxc".to_owned(),
        });
    }

    for symbol_id in semantic.scoping().symbol_ids() {
        let Some(target) = symbol_ids.get(&format!("{symbol_id:?}")).cloned() else {
            continue;
        };
        for reference in semantic.scoping().get_resolved_references(symbol_id) {
            if !(reference.is_read() || reference.is_type() || reference.flags().is_value_as_type())
                || is_export_reference(semantic.reference_span(reference), module_record)
                || is_commonjs_export_reference(semantic, reference.node_id(), &block.content)
            {
                continue;
            }
            if reference_call_kind(semantic, reference.node_id()).is_some() {
                continue;
            }
            let source = source_owner(semantic, reference.node_id(), path, node_owner_ids);
            if source == target {
                continue;
            }
            let start = semantic.reference_span(reference).start as usize;
            let (line, column) = line_column(&block.content, start);
            let mode = if reference.is_type() || reference.flags().is_value_as_type() {
                "type"
            } else {
                "runtime"
            };
            edges.push(GraphEdge {
                source,
                target: target.clone(),
                kind: "reference".to_owned(),
                path: path.to_owned(),
                start: block.offset + start,
                line,
                column,
                confidence: if parse_unknown { "potential" } else { "exact" }.to_owned(),
                mode: mode.to_owned(),
                provenance: "oxc".to_owned(),
            });
        }
    }
    edges
}

fn is_commonjs_export_reference(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    content: &str,
) -> bool {
    semantic
        .nodes()
        .ancestors(node_id)
        .skip(1)
        .find_map(|ancestor| match ancestor.kind() {
            AstKind::AssignmentExpression(assignment) => {
                let span = assignment.left.span();
                let target = content
                    .get(span.start as usize..span.end as usize)
                    .unwrap_or_default()
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>();
                Some(is_commonjs_export_target(&target))
            }
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn is_commonjs_export_target(target: &str) -> bool {
    target == "exports"
        || target.starts_with("exports.")
        || target.starts_with("exports[")
        || target == "module.exports"
        || target.starts_with("module.exports.")
        || target.starts_with("module.exports[")
        || target.starts_with("module['exports']")
        || target.starts_with("module[\"exports\"]")
}

fn commonjs_export_edges(
    semantic: &oxc_semantic::Semantic<'_>,
    block: &SourceBlock,
    path: &str,
    symbol_ids: &HashMap<String, String>,
    node_owner_ids: &HashMap<NodeId, String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    for assignment_node in semantic.nodes() {
        let AstKind::AssignmentExpression(assignment) = assignment_node.kind() else {
            continue;
        };
        let left = assignment.left.span();
        let target = block
            .content
            .get(left.start as usize..left.end as usize)
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if !is_commonjs_export_target(&target) {
            continue;
        }
        let right = assignment.right.span();
        let mut targets = BTreeSet::new();
        for node in semantic.nodes() {
            if !right.contains_inclusive(node.kind().span()) {
                continue;
            }
            let direct_value = node.kind().span() == right
                || semantic.nodes().parent_node(node.id()).is_some_and(|parent| {
                    matches!(parent.kind(), AstKind::ObjectExpression(object) if object.span == right)
                        || matches!(parent.kind(), AstKind::ObjectProperty(property) if semantic
                            .nodes()
                            .parent_node(parent.id())
                            .is_some_and(|grandparent| matches!(grandparent.kind(), AstKind::ObjectExpression(object) if object.span == right))
                            && property.value.span() == node.kind().span())
                });
            if !direct_value {
                continue;
            }
            if let Some(owner) = node_owner_ids.get(&node.id())
                && matches!(
                    node.kind(),
                    AstKind::Function(_)
                        | AstKind::ArrowFunctionExpression(_)
                        | AstKind::Class(_)
                        | AstKind::ObjectProperty(_)
                )
            {
                targets.insert(owner.clone());
            }
            if let AstKind::IdentifierReference(identifier) = node.kind()
                && let Some(reference_id) = identifier.reference_id.get()
                && let Some(symbol_id) = semantic.scoping().get_reference(reference_id).symbol_id()
                && let Some(candidate) = symbol_ids.get(&format!("{symbol_id:?}"))
            {
                targets.insert(candidate.clone());
            }
        }
        let start = assignment.span.start as usize;
        let (line, column) = line_column(&block.content, start);
        let source = source_owner(semantic, assignment_node.id(), path, node_owner_ids);
        edges.extend(targets.into_iter().map(|target| GraphEdge {
            source: source.clone(),
            target,
            kind: "commonjs-export".to_owned(),
            path: path.to_owned(),
            start: block.offset + start,
            line,
            column,
            confidence: "exact".to_owned(),
            mode: "runtime".to_owned(),
            provenance: "oxc".to_owned(),
        }));
    }
    edges
}

fn owner_activation_edges(owners: &[ExecutionOwner], path: &str) -> Vec<GraphEdge> {
    owners
        .iter()
        .filter_map(|owner| {
            let (source, kind) = match owner.kind.as_str() {
                "call-callback" => (owner.parent_id.clone()?, "callback"),
                "iife" => (owner.parent_id.clone()?, "immediate"),
                "callback" => {
                    let parent = owner
                        .parent_id
                        .as_deref()
                        .and_then(|parent_id| owners.iter().find(|item| item.id == parent_id))?;
                    let source = match parent.kind.as_str() {
                        "variable-initializer" => parent.finding_candidate_id.clone()?,
                        "export-initializer" => parent.id.clone(),
                        _ => return None,
                    };
                    (source, "callback")
                }
                _ => return None,
            };
            Some(GraphEdge {
                source,
                target: owner.id.clone(),
                kind: kind.to_owned(),
                path: path.to_owned(),
                start: owner.start,
                line: owner.line,
                column: owner.column,
                confidence: if kind == "callback" {
                    "potential"
                } else {
                    "exact"
                }
                .to_owned(),
                mode: "runtime".to_owned(),
                provenance: "oxc".to_owned(),
            })
        })
        .collect()
}

fn is_finding_declaration(
    semantic: &oxc_semantic::Semantic<'_>,
    symbol_id: oxc_syntax::symbol::SymbolId,
    declaration_id: NodeId,
    kind: &str,
) -> bool {
    if has_non_finding_ancestor(semantic, declaration_id) {
        return false;
    }
    match kind {
        "function" => runtime_function_span(semantic, symbol_id).is_some(),
        "class" => matches!(
            semantic.nodes().kind(declaration_id),
            AstKind::Class(class) if class.is_declaration() && !class.is_typescript_syntax()
        ),
        "variable" => matches!(
            semantic.nodes().kind(declaration_id),
            AstKind::VariableDeclarator(declarator)
                if semantic
                    .nodes()
                    .parent_kind(declaration_id)
                    .is_some_and(|parent| matches!(parent, AstKind::VariableDeclaration(declaration) if !declaration.declare))
                    && declarator.id.get_binding_identifiers().iter().any(|identifier| {
                        identifier
                            .symbol_id
                            .get()
                            .is_some_and(|id| id == symbol_id)
                    })
        ),
        _ => false,
    }
}

fn has_non_finding_ancestor(semantic: &oxc_semantic::Semantic<'_>, node_id: NodeId) -> bool {
    semantic.nodes().ancestor_kinds(node_id).any(|kind| {
        matches!(
            kind,
            AstKind::FormalParameter(_)
                | AstKind::BindingRestElement(_)
                | AstKind::CatchParameter(_)
                | AstKind::PropertyDefinition(_)
                | AstKind::AccessorProperty(_)
                | AstKind::ImportDeclaration(_)
                | AstKind::ImportSpecifier(_)
                | AstKind::ImportDefaultSpecifier(_)
                | AstKind::ImportNamespaceSpecifier(_)
                | AstKind::TSImportEqualsDeclaration(_)
        )
    })
}

fn runtime_function_span(
    semantic: &oxc_semantic::Semantic<'_>,
    symbol_id: oxc_syntax::symbol::SymbolId,
) -> Option<Span> {
    runtime_function_node(semantic, symbol_id).map(|(_, span)| span)
}

fn runtime_function_node(
    semantic: &oxc_semantic::Semantic<'_>,
    symbol_id: oxc_syntax::symbol::SymbolId,
) -> Option<(NodeId, Span)> {
    let declaration_ids = std::iter::once(semantic.scoping().symbol_declaration(symbol_id)).chain(
        semantic
            .scoping()
            .symbol_redeclarations(symbol_id)
            .iter()
            .map(|redeclaration| redeclaration.declaration),
    );
    declaration_ids
        .filter_map(|node_id| match semantic.nodes().kind(node_id) {
            AstKind::Function(function)
                if function.is_function_declaration()
                    && function.body.is_some()
                    && !function.declare
                    && !function.is_typescript_syntax() =>
            {
                Some((node_id, function.span))
            }
            _ => None,
        })
        .next()
}

fn is_callable_initializer(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ArrowFunctionExpression(_) => true,
        Expression::FunctionExpression(function) => function.body.is_some(),
        _ => false,
    }
}

fn is_callable_expression_with_body(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ArrowFunctionExpression(_) => true,
        Expression::FunctionExpression(function) => {
            function.body.is_some() && !function.is_typescript_syntax()
        }
        _ => false,
    }
}

fn initializer_effect(expression: Option<&Expression<'_>>) -> &'static str {
    let Some(expression) = expression else {
        return "none";
    };
    match expression {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::MetaProperty(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => "side-effect-free",
        // Reads can throw through TDZ/cycles; prove them only with future flow analysis.
        Expression::Identifier(_) | Expression::Super(_) => "unknown",
        Expression::ArrayExpression(array) => {
            array
                .elements
                .iter()
                .fold("side-effect-free", |current, element| match element {
                    ArrayExpressionElement::Elision(_) => current,
                    // Iteration can execute user code even when the spread expression is a read.
                    ArrayExpressionElement::SpreadElement(_) => "unknown",
                    element => merge_effect(current, initializer_effect(element.as_expression())),
                })
        }
        Expression::ObjectExpression(object) => {
            object
                .properties
                .iter()
                .fold("side-effect-free", |current, property| match property {
                    // Property enumeration can execute proxy traps and getters.
                    ObjectPropertyKind::SpreadProperty(_) => "unknown",
                    ObjectPropertyKind::ObjectProperty(property) => {
                        let key_effect = if property.computed {
                            initializer_effect(property.key.as_expression())
                        } else {
                            "side-effect-free"
                        };
                        merge_effect(
                            merge_effect(current, key_effect),
                            initializer_effect(Some(&property.value)),
                        )
                    }
                })
        }
        Expression::ParenthesizedExpression(expression) => {
            initializer_effect(Some(&expression.expression))
        }
        Expression::TSAsExpression(expression) => initializer_effect(Some(&expression.expression)),
        Expression::TSSatisfiesExpression(expression) => {
            initializer_effect(Some(&expression.expression))
        }
        Expression::TSNonNullExpression(expression) => {
            initializer_effect(Some(&expression.expression))
        }
        Expression::TSTypeAssertion(expression) => initializer_effect(Some(&expression.expression)),
        Expression::CallExpression(_)
        | Expression::NewExpression(_)
        | Expression::ImportExpression(_)
        | Expression::AwaitExpression(_)
        | Expression::AssignmentExpression(_)
        | Expression::UpdateExpression(_)
        | Expression::YieldExpression(_) => "side-effectful",
        _ => "unknown",
    }
}

fn merge_effect(current: &'static str, incoming: &'static str) -> &'static str {
    if effect_rank(incoming) > effect_rank(current) {
        incoming
    } else {
        current
    }
}

fn top_level_effect(program: &Program<'_>) -> String {
    program
        .body
        .iter()
        .fold("side-effect-free".to_owned(), |current, statement| {
            merge_initializer_effect(&current, statement_effect(statement))
        })
}

fn statement_effect(statement: &Statement<'_>) -> &'static str {
    use oxc_ast::ast::ImportOrExportKind;

    match statement {
        Statement::EmptyStatement(_)
        | Statement::FunctionDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSModuleDeclaration(_)
        | Statement::TSNamespaceExportDeclaration(_) => "side-effect-free",
        Statement::VariableDeclaration(declaration) => {
            declaration
                .declarations
                .iter()
                .fold("side-effect-free", |current, declarator| {
                    let incoming = initializer_effect(declarator.init.as_ref());
                    if effect_rank(incoming) > effect_rank(current) {
                        incoming
                    } else {
                        current
                    }
                })
        }
        Statement::ImportDeclaration(declaration) => {
            if declaration.import_kind == ImportOrExportKind::Type {
                "side-effect-free"
            } else {
                "side-effectful"
            }
        }
        Statement::ExportAllDeclaration(declaration) => {
            if declaration.export_kind == ImportOrExportKind::Type {
                "side-effect-free"
            } else {
                "side-effectful"
            }
        }
        Statement::ExportNamedDeclaration(export) => {
            if export.export_kind == ImportOrExportKind::Type {
                "side-effect-free"
            } else if export.source.is_some() {
                "side-effectful"
            } else {
                export
                    .declaration
                    .as_ref()
                    .map_or("side-effect-free", declaration_effect)
            }
        }
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function)
                if function.body.is_some() && !function.is_typescript_syntax() =>
            {
                "side-effect-free"
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => "side-effect-free",
            declaration => declaration
                .as_expression()
                .map_or("unknown", |expression| initializer_effect(Some(expression))),
        },
        Statement::ClassDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSImportEqualsDeclaration(_)
        | Statement::TSExportAssignment(_) => "unknown",
        _ => "side-effectful",
    }
}

fn declaration_effect(declaration: &Declaration<'_>) -> &'static str {
    match declaration {
        Declaration::FunctionDeclaration(function)
            if function.body.is_some() && !function.is_typescript_syntax() =>
        {
            "side-effect-free"
        }
        Declaration::VariableDeclaration(declaration) => {
            declaration
                .declarations
                .iter()
                .fold("side-effect-free", |current, declarator| {
                    let incoming = initializer_effect(declarator.init.as_ref());
                    if effect_rank(incoming) > effect_rank(current) {
                        incoming
                    } else {
                        current
                    }
                })
        }
        Declaration::TSTypeAliasDeclaration(_)
        | Declaration::TSInterfaceDeclaration(_)
        | Declaration::TSModuleDeclaration(_) => "side-effect-free",
        _ => "unknown",
    }
}

fn effect_rank(effect: &str) -> u8 {
    match effect {
        "none" => 0,
        "side-effect-free" => 1,
        "side-effectful" => 2,
        _ => 3,
    }
}

fn merge_initializer_effect(current: &str, incoming: &str) -> String {
    if effect_rank(incoming) > effect_rank(current) {
        incoming.to_owned()
    } else {
        current.to_owned()
    }
}

fn execution_owner_id(path: &str, kind: &str, start: usize) -> String {
    format!("owner::{path}::{kind}::{start}")
}

fn execution_owner(
    id: String,
    kind: &str,
    block: &SourceBlock,
    path: &str,
    local_start: usize,
    finding_candidate_id: Option<String>,
    parent_id: Option<String>,
) -> ExecutionOwner {
    let (line, column) = line_column(&block.content, local_start);
    ExecutionOwner {
        id,
        kind: kind.to_owned(),
        path: path.to_owned(),
        start: block.offset + local_start,
        line,
        column,
        finding_candidate_id,
        parent_id,
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_non_candidate_owners(
    semantic: &oxc_semantic::Semantic<'_>,
    block: &SourceBlock,
    path: &str,
    file_owner_id: &str,
    symbol_ids: &HashMap<String, String>,
    symbol_owner_ids: &HashMap<String, String>,
    node_owner_ids: &mut HashMap<NodeId, String>,
    execution_owners: &mut Vec<ExecutionOwner>,
) {
    for node in semantic.nodes() {
        match node.kind() {
            AstKind::Class(class) if class.id.is_none() && !class.declare => {
                let owner_id = execution_owner_id(path, "class", class.span.start as usize);
                node_owner_ids.insert(node.id(), owner_id.clone());
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        "class",
                        block,
                        path,
                        class.span.start as usize,
                        None,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            AstKind::VariableDeclarator(declarator)
                if declarator.init.is_some()
                    && !has_non_finding_ancestor(semantic, node.id())
                    && semantic.nodes().parent_kind(node.id()).is_some_and(|parent| {
                        matches!(parent, AstKind::VariableDeclaration(declaration) if !declaration.declare)
                    }) =>
            {
                let owner_id = node_owner_ids.get(&node.id()).cloned().or_else(|| {
                    declarator
                        .init
                        .as_ref()
                        .map(|initializer| {
                            execution_owner_id(
                                path,
                                "variable-initializer",
                                block.offset + initializer.span().start as usize,
                            )
                        })
                });
                let Some(owner_id) = owner_id else {
                    continue;
                };
                let finding_candidate_id = declarator
                    .id
                    .get_binding_identifiers()
                    .into_iter()
                    .find_map(|identifier| {
                        identifier
                            .symbol_id
                            .get()
                            .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
                            .cloned()
                    });
                node_owner_ids.insert(node.id(), owner_id.clone());
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        "variable-initializer",
                        block,
                        path,
                        declarator
                            .init
                            .as_ref()
                            .map_or(0, |initializer| initializer.span().start as usize),
                        finding_candidate_id,
                        Some(file_owner_id.to_owned()),
                    ),
                );
            }
            AstKind::Function(function)
                if function.body.is_some() && !function.is_typescript_syntax() =>
            {
                let owner_id = function_owner_id(
                    semantic,
                    node.id(),
                    function,
                    block,
                    path,
                    symbol_ids,
                    symbol_owner_ids,
                    node_owner_ids,
                );
                node_owner_ids.insert(node.id(), owner_id.clone());
                let kind = owner_kind(semantic, node.id(), function);
                let finding_candidate_id = symbol_candidate_id(function, symbol_ids).or_else(|| {
                    symbol_ids
                        .values()
                        .find(|candidate_id| **candidate_id == owner_id)
                        .cloned()
                });
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        kind,
                        block,
                        path,
                        function.span.start as usize,
                        finding_candidate_id,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            AstKind::ArrowFunctionExpression(arrow) => {
                let owner_id = function_owner_id(
                    semantic,
                    node.id(),
                    arrow,
                    block,
                    path,
                    symbol_ids,
                    symbol_owner_ids,
                    node_owner_ids,
                );
                node_owner_ids.insert(node.id(), owner_id.clone());
                let kind = owner_kind(semantic, node.id(), arrow);
                let finding_candidate_id = symbol_ids
                    .values()
                    .find(|candidate_id| **candidate_id == owner_id)
                    .cloned();
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        kind,
                        block,
                        path,
                        arrow.span.start as usize,
                        finding_candidate_id,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            AstKind::StaticBlock(block_node) => {
                let owner_id = execution_owner_id(path, "static-block", block_node.span.start as usize);
                node_owner_ids.insert(node.id(), owner_id.clone());
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        "static-block",
                        block,
                        path,
                        block_node.span.start as usize,
                        None,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            AstKind::PropertyDefinition(property)
                if property.value.is_some()
                    && property.r#type
                        != oxc_ast::ast::PropertyDefinitionType::TSAbstractPropertyDefinition
                    && !property.declare =>
            {
                let kind = if property.r#static {
                    "static-field-initializer"
                } else {
                    "field-initializer"
                };
                let owner_id = execution_owner_id(path, kind, property.span.start as usize);
                node_owner_ids.insert(node.id(), owner_id.clone());
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        kind,
                        block,
                        path,
                        property.span.start as usize,
                        None,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            AstKind::AccessorProperty(property)
                if property.value.is_some()
                    && property.r#type
                        != oxc_ast::ast::AccessorPropertyType::TSAbstractAccessorProperty =>
            {
                let kind = if property.r#static {
                    "static-field-initializer"
                } else {
                    "field-initializer"
                };
                let owner_id = execution_owner_id(path, kind, property.span.start as usize);
                node_owner_ids.insert(node.id(), owner_id.clone());
                push_execution_owner(
                    execution_owners,
                    execution_owner(
                        owner_id,
                        kind,
                        block,
                        path,
                        property.span.start as usize,
                        None,
                        parent_owner_id(semantic, node.id(), file_owner_id, node_owner_ids),
                    ),
                );
            }
            _ => {}
        }
    }
}

fn collect_default_export_expression_owner(
    program: &Program<'_>,
    semantic: &oxc_semantic::Semantic<'_>,
    block: &SourceBlock,
    path: &str,
    file_owner_id: &str,
    node_owner_ids: &mut HashMap<NodeId, String>,
    execution_owners: &mut Vec<ExecutionOwner>,
) {
    for statement in &program.body {
        let Statement::ExportDefaultDeclaration(export) = statement else {
            continue;
        };
        let Some(expression) = export.declaration.as_expression() else {
            continue;
        };
        let span = expression.span();
        let Some(node) = semantic
            .nodes()
            .iter()
            .find(|node| node.kind().span() == span)
        else {
            continue;
        };
        if node_owner_ids.contains_key(&node.id()) {
            continue;
        }
        let owner_id = execution_owner_id(
            path,
            "export-initializer",
            block.offset + span.start as usize,
        );
        node_owner_ids.insert(node.id(), owner_id.clone());
        push_execution_owner(
            execution_owners,
            execution_owner(
                owner_id,
                "export-initializer",
                block,
                path,
                span.start as usize,
                None,
                Some(file_owner_id.to_owned()),
            ),
        );
    }
}

fn repair_owner_parents(
    semantic: &oxc_semantic::Semantic<'_>,
    file_owner_id: &str,
    node_owner_ids: &HashMap<NodeId, String>,
    execution_owners: &mut [ExecutionOwner],
) {
    let mut parents = HashMap::<String, String>::new();
    for node in semantic.nodes() {
        let Some(owner_id) = node_owner_ids.get(&node.id()) else {
            continue;
        };
        let parent = semantic
            .nodes()
            .ancestors(node.id())
            .skip(1)
            .filter_map(|ancestor| node_owner_ids.get(&ancestor.id()))
            .find(|candidate| *candidate != owner_id)
            .cloned()
            .unwrap_or_else(|| file_owner_id.to_owned());
        parents
            .entry(owner_id.clone())
            .and_modify(|current| {
                if current == file_owner_id && parent != file_owner_id {
                    *current = parent.clone();
                }
            })
            .or_insert(parent);
    }
    for owner in execution_owners {
        if owner.id != file_owner_id {
            owner.parent_id = parents
                .get(&owner.id)
                .cloned()
                .or_else(|| Some(file_owner_id.to_owned()));
        }
    }
}

fn default_export_owner(
    program: &Program<'_>,
    semantic: &oxc_semantic::Semantic<'_>,
    node_owner_ids: &HashMap<NodeId, String>,
) -> Option<String> {
    let span = program.body.iter().find_map(|statement| {
        let Statement::ExportDefaultDeclaration(export) = statement else {
            return None;
        };
        Some(export.declaration.span())
    })?;
    semantic.nodes().iter().find_map(|node| {
        (node.kind().span() == span)
            .then(|| node_owner_ids.get(&node.id()).cloned())
            .flatten()
    })
}

fn symbol_candidate_id(
    function: &oxc_ast::ast::Function<'_>,
    symbol_ids: &HashMap<String, String>,
) -> Option<String> {
    function.id.as_ref().and_then(|identifier| {
        identifier
            .symbol_id
            .get()
            .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
            .cloned()
    })
}

#[allow(clippy::too_many_arguments)]
fn function_owner_id(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    function: &impl FunctionLike,
    block: &SourceBlock,
    path: &str,
    symbol_ids: &HashMap<String, String>,
    symbol_owner_ids: &HashMap<String, String>,
    node_owner_ids: &HashMap<NodeId, String>,
) -> String {
    if let Some(owner_id) = node_owner_ids.get(&node_id) {
        return owner_id.clone();
    }
    if let Some(parent) = semantic.nodes().parent_node(node_id) {
        if let AstKind::VariableDeclarator(declarator) = parent.kind() {
            if let Some(identifier) = declarator.id.get_binding_identifier() {
                if let Some(symbol_id) = identifier.symbol_id.get() {
                    if let Some(candidate_id) = symbol_ids.get(&format!("{symbol_id:?}")) {
                        return candidate_id.clone();
                    }
                    if let Some(owner_id) = symbol_owner_ids.get(&format!("{symbol_id:?}")) {
                        return owner_id.clone();
                    }
                }
            }
        }
        if let Some(owner_id) = node_owner_ids.get(&parent.id()) {
            return owner_id.clone();
        }
    }
    if function.is_declaration() {
        if let Some(symbol_id) = function.symbol_id() {
            if let Some(candidate_id) = symbol_ids.get(&format!("{symbol_id:?}")) {
                return candidate_id.clone();
            }
        }
    }
    let kind = if semantic
        .nodes()
        .parent_kind(node_id)
        .is_some_and(|parent| matches!(parent, AstKind::CallExpression(call) if call.callee.span().contains_inclusive(function.span())))
    {
        "iife"
    } else {
        "callback"
    };
    execution_owner_id(path, kind, block.offset + function.span().start as usize)
}

trait FunctionLike {
    fn span(&self) -> Span;
    fn symbol_id(&self) -> Option<oxc_syntax::symbol::SymbolId>;
    fn is_declaration(&self) -> bool;
}

impl FunctionLike for oxc_ast::ast::Function<'_> {
    fn span(&self) -> Span {
        self.span
    }

    fn symbol_id(&self) -> Option<oxc_syntax::symbol::SymbolId> {
        self.id
            .as_ref()
            .and_then(|identifier| identifier.symbol_id.get())
    }

    fn is_declaration(&self) -> bool {
        self.is_function_declaration()
    }
}

impl FunctionLike for oxc_ast::ast::ArrowFunctionExpression<'_> {
    fn span(&self) -> Span {
        self.span
    }

    fn symbol_id(&self) -> Option<oxc_syntax::symbol::SymbolId> {
        None
    }

    fn is_declaration(&self) -> bool {
        false
    }
}

fn owner_kind(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    function: &impl FunctionLike,
) -> &'static str {
    if function.is_declaration() {
        return "function";
    }
    if let Some(parent) = semantic.nodes().parent_node(node_id) {
        match parent.kind() {
            AstKind::MethodDefinition(_) => return "method",
            AstKind::ObjectProperty(property) if property.method => return "method",
            AstKind::VariableDeclarator(_) => return "function",
            AstKind::CallExpression(call)
                if call.callee.span().contains_inclusive(function.span()) =>
            {
                return "iife";
            }
            _ => {}
        }
    }
    if is_call_argument_callback(semantic, node_id, function.span()) {
        return "call-callback";
    }
    "callback"
}

fn is_call_argument_callback(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    function_span: Span,
) -> bool {
    for ancestor in semantic.nodes().ancestors(node_id).skip(1) {
        match ancestor.kind() {
            AstKind::CallExpression(call) => {
                return call
                    .arguments
                    .iter()
                    .any(|argument| argument.span().contains_inclusive(function_span));
            }
            AstKind::VariableDeclarator(_)
            | AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_) => return false,
            _ => {}
        }
    }
    false
}

fn parent_owner_id(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    file_owner_id: &str,
    node_owner_ids: &HashMap<NodeId, String>,
) -> Option<String> {
    semantic
        .nodes()
        .ancestors(node_id)
        .skip(1)
        .find_map(|ancestor| node_owner_ids.get(&ancestor.id()).cloned())
        .or_else(|| Some(file_owner_id.to_owned()))
}

fn push_execution_owner(owners: &mut Vec<ExecutionOwner>, owner: ExecutionOwner) {
    if owners.iter().all(|existing| existing.id != owner.id) {
        owners.push(owner);
    }
}

fn source_owner(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    path: &str,
    node_owner_ids: &HashMap<NodeId, String>,
) -> String {
    let mut initializer_owner = None;
    for ancestor in semantic.nodes().ancestors(node_id) {
        let Some(owner_id) = node_owner_ids.get(&ancestor.id()).cloned() else {
            continue;
        };
        if owner_id.contains("::variable-initializer::") {
            initializer_owner = Some(owner_id);
        } else {
            return owner_id;
        }
    }
    initializer_owner.unwrap_or_else(|| format!("file::{path}"))
}

fn import_usage(
    semantic: &oxc_semantic::Semantic<'_>,
    reference: &oxc_syntax::reference::Reference,
    path: &str,
    node_owner_ids: &HashMap<NodeId, String>,
) -> ImportUsage {
    let reference_span = semantic.reference_span(reference);
    let mut operation_span = reference_span;
    let mut member_name = None;
    let mut dynamic_member = false;
    if let Some(parent) = semantic.nodes().parent_node(reference.node_id()) {
        match parent.kind() {
            AstKind::StaticMemberExpression(member)
                if member.object.span().contains_inclusive(reference_span) =>
            {
                operation_span = member.span;
                member_name = Some(member.property.name.to_string());
            }
            AstKind::ComputedMemberExpression(member)
                if member.object.span().contains_inclusive(reference_span) =>
            {
                operation_span = member.span;
                member_name = match &member.expression {
                    Expression::StringLiteral(literal) => Some(literal.value.to_string()),
                    Expression::NumericLiteral(literal) => Some(literal.value.to_string()),
                    _ => None,
                };
                dynamic_member = member_name.is_none();
            }
            _ => {}
        }
    }
    let kind = reference_operation_kind(semantic, reference.node_id(), operation_span);
    ImportUsage {
        source: source_owner(semantic, reference.node_id(), path, node_owner_ids),
        member_name,
        dynamic_member,
        kind: kind.to_owned(),
        mode: if reference.is_type() {
            "type"
        } else {
            "runtime"
        }
        .to_owned(),
    }
}

fn reference_operation_kind(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
    operation_span: Span,
) -> &'static str {
    semantic
        .nodes()
        .ancestors(node_id)
        .skip(1)
        .find_map(|ancestor| match ancestor.kind() {
            AstKind::CallExpression(call) if call.callee.span() == operation_span => Some("call"),
            AstKind::NewExpression(new_expression)
                if new_expression.callee.span() == operation_span =>
            {
                Some("instantiates")
            }
            _ => None,
        })
        .unwrap_or("reference")
}

fn reference_call_kind(
    semantic: &oxc_semantic::Semantic<'_>,
    node_id: NodeId,
) -> Option<&'static str> {
    let reference_span = semantic.nodes().kind(node_id).span();
    match reference_operation_kind(semantic, node_id, reference_span) {
        "reference" => None,
        kind => Some(kind),
    }
}

fn merge_module(current: &mut ModuleSummary, mut incoming: ModuleSummary) {
    if current.default_export_owner.is_none() {
        current.default_export_owner = incoming.default_export_owner.take();
    }
    current.imports.append(&mut incoming.imports);
    current.local_exports.append(&mut incoming.local_exports);
    current.reexports.append(&mut incoming.reexports);
    current.star_exports.append(&mut incoming.star_exports);
    current
        .dynamic_imports
        .append(&mut incoming.dynamic_imports);
    current.requires.append(&mut incoming.requires);
    current.has_parse_errors |= incoming.has_parse_errors;
    dedup_by_key(&mut current.imports, |item| {
        (item.start, item.module.clone(), item.local_name.clone())
    });
    dedup_by_key(&mut current.local_exports, |item| {
        (
            item.start,
            item.export_name.clone(),
            item.local_name.clone(),
        )
    });
    dedup_by_key(&mut current.reexports, |item| {
        (item.start, item.export_name.clone(), item.module.clone())
    });
    dedup_by_key(&mut current.star_exports, |item| {
        (item.start, item.module.clone())
    });
    dedup_by_key(&mut current.dynamic_imports, |item| {
        (item.start, item.expression.clone())
    });
    dedup_by_key(&mut current.requires, |item| item.start);
}

fn dedup_by_key<T, K, F>(items: &mut Vec<T>, mut key: F)
where
    K: Eq + std::hash::Hash,
    F: FnMut(&T) -> K,
{
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(key(item)));
}

fn link_modules(root: &Path, result: &mut ScanResult) {
    let resolver = make_resolver(root);
    let modules = result
        .modules
        .iter()
        .map(|module| (module.path.clone(), module.clone()))
        .collect::<HashMap<_, _>>();
    let module_paths = modules.keys().cloned().collect::<BTreeSet<_>>();
    let mut candidate_ids = HashMap::<String, usize>::new();
    let mut module_candidates = HashMap::<String, Vec<usize>>::new();
    for (index, candidate) in result.candidates.iter().enumerate() {
        candidate_ids.insert(candidate.id.clone(), index);
        module_candidates
            .entry(candidate.path.clone())
            .or_default()
            .push(index);
    }
    for module in modules.values() {
        result
            .public_exports
            .entry(module.path.clone())
            .or_default()
            .extend(
                module
                    .local_exports
                    .iter()
                    .filter_map(|export| export.local_id.clone()),
            );
    }

    for module in modules.values() {
        if is_test_module(&module.path) {
            continue;
        }
        for import in &module.imports {
            let Some(target) =
                resolve_request(root, &resolver, &module_paths, &module.path, &import.module)
            else {
                unresolved_request(result, module, &import.module);
                continue;
            };
            if import.reexport_only {
                continue;
            }
            if import.type_only {
                for usage in &import.usages {
                    result.edges.push(GraphEdge {
                        source: usage.source.clone(),
                        target: format!("file::{target}"),
                        kind: "type-import".to_owned(),
                        path: module.path.clone(),
                        start: import.start,
                        line: import.line,
                        column: import.column,
                        confidence: "exact".to_owned(),
                        mode: "type".to_owned(),
                        provenance: "oxc".to_owned(),
                    });
                }
            } else {
                result.edges.push(GraphEdge {
                    source: format!("file::{}", module.path),
                    target: format!("file::{target}"),
                    kind: "import".to_owned(),
                    path: module.path.clone(),
                    start: import.start,
                    line: import.line,
                    column: import.column,
                    confidence: "exact".to_owned(),
                    mode: "runtime".to_owned(),
                    provenance: "oxc".to_owned(),
                });
            }
            if !import.type_only || import.local_used {
                result.used_files.insert(target.clone());
            }
            if !import.local_used {
                continue;
            }
            for usage in &import.usages {
                let (targets, target_files, potential) =
                    if let Some(import_name) = import.import_name.as_deref() {
                        (
                            collect_export_targets(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                import_name,
                                &mut HashSet::new(),
                            ),
                            collect_export_target_files(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                import_name,
                                &mut HashSet::new(),
                            ),
                            false,
                        )
                    } else if let Some(member_name) = usage.member_name.as_deref() {
                        (
                            collect_export_targets(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                member_name,
                                &mut HashSet::new(),
                            ),
                            collect_export_target_files(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                member_name,
                                &mut HashSet::new(),
                            ),
                            usage.dynamic_member,
                        )
                    } else {
                        (
                            collect_module_export_targets(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                &mut HashSet::new(),
                            ),
                            collect_module_export_files(
                                root,
                                &resolver,
                                &module_paths,
                                &modules,
                                &target,
                                &mut HashSet::new(),
                            ),
                            true,
                        )
                    };
                let confidence = if potential || targets.len() != 1 {
                    "potential"
                } else {
                    "exact"
                };
                if targets.is_empty()
                    && import.import_name.as_deref() == Some("default")
                    && let Some(owner) = modules
                        .get(&target)
                        .and_then(|module| module.default_export_owner.clone())
                {
                    result.edges.push(GraphEdge {
                        source: usage.source.clone(),
                        target: owner,
                        kind: if usage.kind == "reference" {
                            "callback".to_owned()
                        } else {
                            usage.kind.clone()
                        },
                        path: module.path.clone(),
                        start: import.start,
                        line: import.line,
                        column: import.column,
                        confidence: "potential".to_owned(),
                        mode: usage.mode.clone(),
                        provenance: "oxc".to_owned(),
                    });
                }
                for id in targets {
                    if let Some(index) = candidate_ids.get(&id).copied() {
                        result.candidates[index].local_used = true;
                    }
                    result.edges.push(GraphEdge {
                        source: usage.source.clone(),
                        target: id,
                        kind: usage.kind.clone(),
                        path: module.path.clone(),
                        start: import.start,
                        line: import.line,
                        column: import.column,
                        confidence: confidence.to_owned(),
                        mode: usage.mode.clone(),
                        provenance: "oxc".to_owned(),
                    });
                }
                for target_file in target_files {
                    result.used_files.insert(target_file.clone());
                    result.edges.push(GraphEdge {
                        source: usage.source.clone(),
                        target: format!("file::{target_file}"),
                        kind: "reference".to_owned(),
                        path: module.path.clone(),
                        start: import.start,
                        line: import.line,
                        column: import.column,
                        confidence: confidence.to_owned(),
                        mode: usage.mode.clone(),
                        provenance: "oxc".to_owned(),
                    });
                }
            }
        }

        for dynamic in &module.dynamic_imports {
            if let Some(specifier) = dynamic.static_specifier.as_deref() {
                if let Some(target) =
                    resolve_request(root, &resolver, &module_paths, &module.path, specifier)
                {
                    result.used_files.insert(target.clone());
                    mark_dynamic_candidates(&module_candidates, result, &target);
                    result.edges.push(GraphEdge {
                        source: dynamic.source.clone(),
                        target: format!("file::{target}"),
                        kind: "dynamic-import".to_owned(),
                        path: module.path.clone(),
                        start: dynamic.start,
                        line: dynamic.line,
                        column: dynamic.column,
                        confidence: "exact".to_owned(),
                        mode: "runtime".to_owned(),
                        provenance: "oxc".to_owned(),
                    });
                } else if is_internal_specifier(specifier) {
                    unresolved_request(result, module, specifier);
                }
                continue;
            }
            if let Some(pattern) = &dynamic.pattern {
                let targets = module_paths
                    .iter()
                    .filter(|target| pattern_matches(root, &module.path, pattern, target))
                    .cloned()
                    .collect::<Vec<_>>();
                for target in targets {
                    result.used_files.insert(target.clone());
                    mark_dynamic_candidates(&module_candidates, result, &target);
                    result.edges.push(GraphEdge {
                        source: dynamic.source.clone(),
                        target: format!("file::{target}"),
                        kind: "dynamic-import".to_owned(),
                        path: module.path.clone(),
                        start: dynamic.start,
                        line: dynamic.line,
                        column: dynamic.column,
                        confidence: "potential".to_owned(),
                        mode: "runtime".to_owned(),
                        provenance: "oxc".to_owned(),
                    });
                }
                continue;
            }
            result
                .coverage_boundaries
                .insert(dynamic.source.clone(), "dynamic-import-boundary".to_owned());
            result.diagnostics.push(format!(
                "{}: dynamic import has no statically bounded target",
                module.path
            ));
        }
        for require in &module.requires {
            let Some(specifier) = require.static_specifier.as_deref() else {
                result.coverage_boundaries.insert(
                    require.source.clone(),
                    "dynamic-require-boundary".to_owned(),
                );
                result.diagnostics.push(format!(
                    "{}: CommonJS require has no statically bounded target",
                    module.path
                ));
                continue;
            };
            let Some(target) =
                resolve_request(root, &resolver, &module_paths, &module.path, specifier)
            else {
                if is_internal_specifier(specifier) {
                    unresolved_request(result, module, specifier);
                }
                continue;
            };
            result.used_files.insert(target.clone());
            mark_dynamic_candidates(&module_candidates, result, &target);
            result.edges.push(GraphEdge {
                source: require.source.clone(),
                target: format!("file::{target}"),
                kind: "require".to_owned(),
                path: module.path.clone(),
                start: require.start,
                line: require.line,
                column: require.column,
                confidence: "exact".to_owned(),
                mode: "runtime".to_owned(),
                provenance: "oxc".to_owned(),
            });
        }
    }

    for module in modules.values() {
        for export in &module.reexports {
            let Some(target) = export.module.as_deref().and_then(|specifier| {
                resolve_request(root, &resolver, &module_paths, &module.path, specifier)
            }) else {
                continue;
            };
            add_file_reexport(result, &target, &module.path, export.line, export.type_only);
            result.edges.push(GraphEdge {
                source: format!("file::{}", module.path),
                target: format!("file::{target}"),
                kind: "reexport".to_owned(),
                path: module.path.clone(),
                start: export.start,
                line: export.line,
                column: export.column,
                confidence: "exact".to_owned(),
                mode: if export.type_only { "type" } else { "runtime" }.to_owned(),
                provenance: "oxc".to_owned(),
            });
            let Some(imported_name) = export.imported_name.as_deref() else {
                let targets = collect_module_export_targets(
                    root,
                    &resolver,
                    &module_paths,
                    &modules,
                    &target,
                    &mut HashSet::new(),
                );
                result
                    .public_exports
                    .entry(module.path.clone())
                    .or_default()
                    .extend(targets);
                add_module_reexports(
                    &module_candidates,
                    result,
                    &target,
                    &module.path,
                    export.line,
                );
                continue;
            };
            let targets = collect_export_targets(
                root,
                &resolver,
                &module_paths,
                &modules,
                &target,
                imported_name,
                &mut HashSet::new(),
            );
            result
                .public_exports
                .entry(module.path.clone())
                .or_default()
                .extend(targets.iter().cloned());
            for id in targets {
                if let Some(index) = candidate_ids.get(&id).copied() {
                    result.candidates[index]
                        .reexport_locations
                        .push(format!("{}:{}", module.path, export.line));
                }
            }
        }
        for export in &module.star_exports {
            let Some(target) =
                resolve_request(root, &resolver, &module_paths, &module.path, &export.module)
            else {
                continue;
            };
            add_file_reexport(result, &target, &module.path, export.line, export.type_only);
            result.edges.push(GraphEdge {
                source: format!("file::{}", module.path),
                target: format!("file::{target}"),
                kind: "reexport".to_owned(),
                path: module.path.clone(),
                start: export.start,
                line: export.line,
                column: export.column,
                confidence: "exact".to_owned(),
                mode: if export.type_only { "type" } else { "runtime" }.to_owned(),
                provenance: "oxc".to_owned(),
            });
            let targets = collect_module_export_targets(
                root,
                &resolver,
                &module_paths,
                &modules,
                &target,
                &mut HashSet::new(),
            );
            result
                .public_exports
                .entry(module.path.clone())
                .or_default()
                .extend(targets);
            add_module_reexports(
                &module_candidates,
                result,
                &target,
                &module.path,
                export.line,
            );
        }
    }
    for candidate in &mut result.candidates {
        candidate.reexport_locations.sort();
        candidate.reexport_locations.dedup();
    }
    for locations in result.file_reexports.values_mut() {
        locations.sort();
        locations.dedup();
    }
    expand_public_class_members(result, &modules);
}

fn expand_public_class_members(result: &mut ScanResult, modules: &HashMap<String, ModuleSummary>) {
    let candidates = result.candidates.clone();
    for (path, ids) in &mut result.public_exports {
        let mut owner_names = BTreeSet::new();
        for id in ids.iter() {
            if let Some(candidate) = candidates.iter().find(|candidate| candidate.id == *id)
                && candidate.kind == "class"
            {
                owner_names.insert(candidate.name.clone());
            }
        }
        let anonymous_default = modules.get(path).is_some_and(|module| {
            module.local_exports.iter().any(|export| {
                export.export_name.as_deref() == Some("default") && export.local_id.is_none()
            })
        });
        ids.extend(candidates.iter().filter_map(|candidate| {
            if candidate.path != *path || candidate.kind != "method" || !candidate.exported {
                return None;
            }
            let named_owner = owner_names
                .iter()
                .any(|owner| candidate.qualified_name.starts_with(&format!("{owner}.")));
            (named_owner
                || (anonymous_default && candidate.qualified_name.starts_with("<default>.")))
            .then(|| candidate.id.clone())
        }));
    }
}

fn mark_dynamic_candidates(
    module_candidates: &HashMap<String, Vec<usize>>,
    result: &mut ScanResult,
    target: &str,
) {
    let Some(indices) = module_candidates.get(target) else {
        return;
    };
    for &index in indices {
        if !result.candidates[index].exported {
            continue;
        }
        result
            .dynamic_unknown
            .insert(result.candidates[index].id.clone());
    }
}

fn add_file_reexport(
    result: &mut ScanResult,
    target: &str,
    source: &str,
    line: usize,
    type_only: bool,
) {
    result
        .file_reexports
        .entry(target.to_owned())
        .or_default()
        .push(ReexportLocation {
            source: source.to_owned(),
            line,
            type_only,
        });
}

fn add_module_reexports(
    module_candidates: &HashMap<String, Vec<usize>>,
    result: &mut ScanResult,
    target: &str,
    source: &str,
    line: usize,
) {
    if let Some(indices) = module_candidates.get(target) {
        for &index in indices {
            result.candidates[index]
                .reexport_locations
                .push(format!("{}:{}", source, line));
        }
    }
}

#[allow(dead_code, clippy::too_many_arguments)]
fn mark_export_use(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    modules: &HashMap<String, ModuleSummary>,
    candidate_ids: &HashMap<String, usize>,
    module_candidates: &HashMap<String, Vec<usize>>,
    result: &mut ScanResult,
    module_path: &str,
    export_name: &str,
    visited: &mut HashSet<(String, String)>,
) {
    if !visited.insert((module_path.to_owned(), export_name.to_owned())) {
        return;
    }
    result.used_files.insert(module_path.to_owned());
    let Some(module) = modules.get(module_path) else {
        return;
    };
    let local_exports = module
        .local_exports
        .iter()
        .filter(|export| export.export_name.as_deref() == Some(export_name))
        .cloned()
        .collect::<Vec<_>>();
    for export in local_exports {
        if let Some(id) = export.local_id {
            if let Some(index) = candidate_ids.get(&id).copied() {
                result.candidates[index].local_used = true;
            }
        } else if let Some(local_name) = export.local_name {
            if let Some(indices) = module_candidates.get(module_path) {
                for &index in indices {
                    if result.candidates[index].name == local_name {
                        result.candidates[index].local_used = true;
                    }
                }
            }
        }
    }

    let reexports = module
        .reexports
        .iter()
        .filter(|export| export.export_name.as_deref() == Some(export_name))
        .cloned()
        .collect::<Vec<_>>();
    for export in reexports {
        let Some(specifier) = export.module.as_deref() else {
            continue;
        };
        let Some(target) = resolve_request(root, resolver, module_paths, module_path, specifier)
        else {
            continue;
        };
        if let Some(imported_name) = export.imported_name.as_deref() {
            mark_export_use(
                root,
                resolver,
                module_paths,
                modules,
                candidate_ids,
                module_candidates,
                result,
                &target,
                imported_name,
                visited,
            );
        } else if let Some(indices) = module_candidates.get(&target) {
            result.used_files.insert(target.clone());
            for &index in indices {
                result.candidates[index].local_used = true;
            }
        }
    }

    let star_exports = module.star_exports.clone();
    if export_name == "default" {
        return;
    }
    for export in star_exports {
        let Some(target) =
            resolve_request(root, resolver, module_paths, module_path, &export.module)
        else {
            continue;
        };
        mark_export_use(
            root,
            resolver,
            module_paths,
            modules,
            candidate_ids,
            module_candidates,
            result,
            &target,
            export_name,
            visited,
        );
    }
}

fn collect_export_targets(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    modules: &HashMap<String, ModuleSummary>,
    module_path: &str,
    export_name: &str,
    visited: &mut HashSet<(String, String)>,
) -> Vec<String> {
    if !visited.insert((module_path.to_owned(), export_name.to_owned())) {
        return Vec::new();
    }
    let Some(module) = modules.get(module_path) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for export in &module.local_exports {
        if export.export_name.as_deref() == Some(export_name) {
            if let Some(id) = export.local_id.clone() {
                ids.push(id);
            }
        }
    }
    for export in &module.reexports {
        if export.export_name.as_deref() != Some(export_name) {
            continue;
        }
        let Some(specifier) = export.module.as_deref() else {
            continue;
        };
        let Some(target) = resolve_request(root, resolver, module_paths, module_path, specifier)
        else {
            continue;
        };
        if let Some(imported_name) = export.imported_name.as_deref() {
            ids.extend(collect_export_targets(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                imported_name,
                visited,
            ));
        } else if let Some(target_module) = modules.get(&target) {
            ids.extend(
                target_module
                    .local_exports
                    .iter()
                    .filter_map(|export| export.local_id.clone()),
            );
        }
    }
    if export_name != "default" {
        for export in &module.star_exports {
            let Some(target) =
                resolve_request(root, resolver, module_paths, module_path, &export.module)
            else {
                continue;
            };
            ids.extend(collect_export_targets(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                export_name,
                visited,
            ));
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn collect_module_export_targets(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    modules: &HashMap<String, ModuleSummary>,
    module_path: &str,
    visited: &mut HashSet<String>,
) -> Vec<String> {
    if !visited.insert(module_path.to_owned()) {
        return Vec::new();
    }
    let Some(module) = modules.get(module_path) else {
        return Vec::new();
    };
    let mut ids = module
        .local_exports
        .iter()
        .filter_map(|export| export.local_id.clone())
        .collect::<Vec<_>>();
    for export in &module.reexports {
        let Some(specifier) = export.module.as_deref() else {
            continue;
        };
        let Some(target) = resolve_request(root, resolver, module_paths, module_path, specifier)
        else {
            continue;
        };
        if let Some(imported_name) = export.imported_name.as_deref() {
            ids.extend(collect_export_targets(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                imported_name,
                &mut HashSet::new(),
            ));
        } else {
            ids.extend(collect_module_export_targets(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                visited,
            ));
        }
    }
    for export in &module.star_exports {
        let Some(target) =
            resolve_request(root, resolver, module_paths, module_path, &export.module)
        else {
            continue;
        };
        ids.extend(collect_module_export_targets(
            root,
            resolver,
            module_paths,
            modules,
            &target,
            visited,
        ));
    }
    ids.sort();
    ids.dedup();
    ids
}

fn collect_export_target_files(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    modules: &HashMap<String, ModuleSummary>,
    module_path: &str,
    export_name: &str,
    visited: &mut HashSet<(String, String)>,
) -> Vec<String> {
    if !visited.insert((module_path.to_owned(), export_name.to_owned())) {
        return Vec::new();
    }
    let Some(module) = modules.get(module_path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    if module
        .local_exports
        .iter()
        .any(|export| export.export_name.as_deref() == Some(export_name))
    {
        files.push(module_path.to_owned());
    }
    for export in &module.reexports {
        if export.export_name.as_deref() != Some(export_name) {
            continue;
        }
        let Some(specifier) = export.module.as_deref() else {
            continue;
        };
        let Some(target) = resolve_request(root, resolver, module_paths, module_path, specifier)
        else {
            continue;
        };
        files.push(target.clone());
        if let Some(imported_name) = export.imported_name.as_deref() {
            files.extend(collect_export_target_files(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                imported_name,
                visited,
            ));
        } else {
            files.extend(collect_module_export_files(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                &mut HashSet::new(),
            ));
        }
    }
    if export_name != "default" {
        for export in &module.star_exports {
            let Some(target) =
                resolve_request(root, resolver, module_paths, module_path, &export.module)
            else {
                continue;
            };
            files.extend(collect_export_target_files(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                export_name,
                visited,
            ));
        }
    }
    files.sort();
    files.dedup();
    files
}

fn collect_module_export_files(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    modules: &HashMap<String, ModuleSummary>,
    module_path: &str,
    visited: &mut HashSet<String>,
) -> Vec<String> {
    if !visited.insert(module_path.to_owned()) {
        return Vec::new();
    }
    let Some(module) = modules.get(module_path) else {
        return Vec::new();
    };
    let mut files = vec![module_path.to_owned()];
    for export in &module.reexports {
        let Some(specifier) = export.module.as_deref() else {
            continue;
        };
        let Some(target) = resolve_request(root, resolver, module_paths, module_path, specifier)
        else {
            continue;
        };
        files.push(target.clone());
        if let Some(imported_name) = export.imported_name.as_deref() {
            files.extend(collect_export_target_files(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                imported_name,
                &mut HashSet::new(),
            ));
        } else {
            files.extend(collect_module_export_files(
                root,
                resolver,
                module_paths,
                modules,
                &target,
                visited,
            ));
        }
    }
    for export in &module.star_exports {
        let Some(target) =
            resolve_request(root, resolver, module_paths, module_path, &export.module)
        else {
            continue;
        };
        files.extend(collect_module_export_files(
            root,
            resolver,
            module_paths,
            modules,
            &target,
            visited,
        ));
    }
    files.sort();
    files.dedup();
    files
}

fn unresolved_request(result: &mut ScanResult, module: &ModuleSummary, specifier: &str) {
    if !is_internal_specifier(specifier) || !is_source_specifier(specifier) {
        return;
    }
    result.unknown_files.insert(module.path.clone());
    result.diagnostics.push(format!(
        "{}: cannot resolve internal module specifier {specifier:?}",
        module.path
    ));
}

fn is_source_specifier(specifier: &str) -> bool {
    Path::new(specifier)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "cjs" | "js" | "jsx" | "mjs" | "ts" | "tsx" | "mts" | "cts" | "vue"
            )
        })
}

fn make_resolver(root: &Path) -> Resolver {
    let mut options = ResolveOptions {
        extensions: [
            ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".vue", ".json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        extension_alias: vec![
            (
                ".js".to_owned(),
                vec![
                    ".ts".to_owned(),
                    ".tsx".to_owned(),
                    ".js".to_owned(),
                    ".jsx".to_owned(),
                    ".vue".to_owned(),
                ],
            ),
            (
                ".jsx".to_owned(),
                vec![".tsx".to_owned(), ".jsx".to_owned(), ".vue".to_owned()],
            ),
        ],
        ..ResolveOptions::default()
    };
    let tsconfig = ["tsconfig.json", "jsconfig.json"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    if let Some(config_file) = tsconfig {
        options.tsconfig = Some(TsconfigOptions {
            config_file,
            references: TsconfigReferences::Auto,
        });
    } else {
        let src = root.join("src");
        if src.is_dir() {
            options.alias = vec![(
                "@".to_owned(),
                vec![AliasValue::Path(src.to_string_lossy().into_owned())],
            )];
        }
    }
    Resolver::new(options)
}

fn resolve_request(
    root: &Path,
    resolver: &Resolver,
    module_paths: &BTreeSet<String>,
    importer: &str,
    specifier: &str,
) -> Option<String> {
    let importer_path = root.join(importer);
    let directory = importer_path.parent().unwrap_or(root);
    if let Ok(resolution) = resolver.resolve(directory, specifier) {
        let path = normalize_path(root, resolution.path());
        if module_paths.contains(&path) {
            return Some(path);
        }
    }
    resolve_manual(root, module_paths, importer, specifier)
}

fn resolve_manual(
    root: &Path,
    module_paths: &BTreeSet<String>,
    importer: &str,
    specifier: &str,
) -> Option<String> {
    let base = if let Some(rest) = specifier.strip_prefix("@/") {
        PathBuf::from("src").join(rest)
    } else if specifier.starts_with('.') {
        Path::new(importer)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(specifier)
    } else {
        PathBuf::from(specifier.strip_prefix('/')?)
    };
    let base = normalize_path(root, &base);
    let mut candidates = vec![base.clone()];
    if Path::new(&base).extension().is_none() {
        for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue"] {
            candidates.push(format!("{base}.{extension}"));
        }
        candidates.push(format!("{base}/index.ts"));
        candidates.push(format!("{base}/index.tsx"));
        candidates.push(format!("{base}/index.js"));
        candidates.push(format!("{base}/index.jsx"));
        candidates.push(format!("{base}/index.vue"));
    }
    candidates
        .into_iter()
        .find(|candidate| module_paths.contains(candidate))
}

fn pattern_matches(root: &Path, importer: &str, pattern: &DynamicPattern, target: &str) -> bool {
    let prefix = dynamic_path(root, importer, &pattern.prefix);
    let suffix = pattern.suffix.replace('\\', "/");
    target.starts_with(&prefix)
        && target.ends_with(&suffix)
        && target.len() >= prefix.len() + suffix.len()
}

fn dynamic_path(root: &Path, importer: &str, path: &str) -> String {
    let base = if let Some(rest) = path.strip_prefix("@/") {
        PathBuf::from("src").join(rest)
    } else if path.starts_with('.') {
        Path::new(importer)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    } else if let Some(rest) = path.strip_prefix('/') {
        PathBuf::from(rest)
    } else {
        PathBuf::from(path)
    };
    normalize_path(root, &base)
}

fn dynamic_import(
    content: &str,
    dynamic: oxc_syntax::module_record::DynamicImport,
    offset: usize,
    source: String,
) -> DynamicImportSummary {
    let start = dynamic.module_request.start as usize;
    let end = dynamic.module_request.end as usize;
    let expression = content
        .get(start..end)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (static_specifier, pattern, unbounded) = parse_dynamic_expression(&expression);
    let (line, column) = line_column(content, start);
    DynamicImportSummary {
        source,
        expression,
        static_specifier,
        pattern,
        unbounded,
        start: offset + start,
        line,
        column,
    }
}

fn parse_dynamic_expression(expression: &str) -> (Option<String>, Option<DynamicPattern>, bool) {
    if expression.len() >= 2 && expression.starts_with('`') && expression.ends_with('`') {
        let value = &expression[1..expression.len() - 1];
        if !value.contains("${") {
            return (Some(value.to_owned()), None, false);
        }
        let prefix_end = value.find("${").unwrap_or(0);
        let suffix_start = value.rfind('}').map_or(value.len(), |index| index + 1);
        return (
            None,
            Some(DynamicPattern {
                prefix: value[..prefix_end].to_owned(),
                suffix: value[suffix_start..].to_owned(),
            }),
            false,
        );
    }
    if expression.len() >= 2
        && matches!(
            (
                expression.as_bytes()[0],
                expression.as_bytes()[expression.len() - 1]
            ),
            (b'"', b'"') | (b'\'', b'\'')
        )
    {
        return (
            unescape_string(&expression[1..expression.len() - 1]),
            None,
            false,
        );
    }
    (None, None, true)
}

fn unescape_string(value: &str) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let escaped = chars.next()?;
        output.push(match escaped {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            other => other,
        });
    }
    Some(output)
}

fn source_type(block: &SourceBlock) -> SourceType {
    match block.lang.to_ascii_lowercase().as_str() {
        "tsx" => SourceType::tsx(),
        "jsx" => SourceType::jsx(),
        "ts" | "typescript" => SourceType::ts(),
        "js" | "javascript" | "mjs" | "cjs" => SourceType::mjs(),
        _ => SourceType::from_path(&block.path).unwrap_or_else(|_| SourceType::ts()),
    }
}

fn language_name(block: &SourceBlock, source_type: SourceType) -> String {
    if block.path.ends_with(".vue") {
        return "vue".to_owned();
    }
    if !block.lang.is_empty() && block.lang != "vue" {
        return block.lang.to_ascii_lowercase();
    }
    if source_type.is_typescript() {
        "typescript".to_owned()
    } else {
        "javascript".to_owned()
    }
}

fn candidate_id(path: &str, kind: &str, name: &str, start: usize) -> String {
    format!("{path}::{kind}::{start}:{name}")
}

fn symbol_exported(
    name: &str,
    module_record: &oxc_syntax::module_record::ModuleRecord<'_>,
) -> bool {
    module_record
        .exported_bindings
        .keys()
        .any(|exported| exported.as_str() == name)
        || module_record.local_export_entries.iter().any(|entry| {
            entry
                .local_name
                .name()
                .is_some_and(|local| local.as_str() == name)
        })
}

fn import_name(name: &ImportImportName<'_>) -> Option<String> {
    match name {
        ImportImportName::Name(name) => Some(name.name.to_string()),
        ImportImportName::Default(_) => Some("default".to_owned()),
        ImportImportName::NamespaceObject => None,
    }
}

fn export_import_name(name: &ExportImportName<'_>) -> Option<String> {
    match name {
        ExportImportName::Name(name) => Some(name.name.to_string()),
        ExportImportName::All | ExportImportName::AllButDefault => None,
        ExportImportName::Null => None,
    }
}

fn export_name(name: &ExportExportName<'_>) -> Option<String> {
    match name {
        ExportExportName::Name(name) => Some(name.name.to_string()),
        ExportExportName::Default(_) => Some("default".to_owned()),
        ExportExportName::Null => None,
    }
}

fn is_export_reference(
    span: Span,
    module_record: &oxc_syntax::module_record::ModuleRecord<'_>,
) -> bool {
    module_record.local_export_entries.iter().any(|entry| {
        export_local_span(entry)
            .is_some_and(|local_span| span.start == local_span.start && span.end == local_span.end)
    })
}

fn export_local_span(entry: &oxc_syntax::module_record::ExportEntry<'_>) -> Option<Span> {
    match &entry.local_name {
        ExportLocalName::Name(name) | ExportLocalName::Default(name) => Some(name.span),
        ExportLocalName::Null => None,
    }
}

fn line_column(content: &str, start: usize) -> (usize, usize) {
    let start = start.min(content.len());
    let prefix = &content.as_bytes()[..start];
    let line = prefix.iter().filter(|&&byte| byte == b'\n').count() + 1;
    let column = prefix
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(start, |index| start - index - 1)
        + 1;
    (line, column)
}

fn normalize_path(root: &Path, path: &Path) -> String {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let absolute = fs::canonicalize(&absolute).unwrap_or(absolute);
    let relative = absolute.strip_prefix(root).unwrap_or(&absolute);
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned()
}

fn is_internal_specifier(specifier: &str) -> bool {
    specifier.starts_with('.') || specifier.starts_with("@/") || specifier.starts_with('/')
}

fn is_test_module(path: &str) -> bool {
    let file = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    Path::new(path).components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some("test" | "tests" | "__tests__" | "e2e" | "cypress")
        )
    }) || file.contains(".test.")
        || file.contains(".spec.")
        || file.starts_with("test_")
        || file.ends_with("_test.rs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!("jt-unused-oxc-{suffix}"));
            fs::create_dir_all(root.join("src/views")).expect("create fixture");
            Self { root }
        }

        fn file(&self, path: &str, content: &str) {
            let path = self.root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, content).expect("write fixture");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn block(path: &str, content: &str) -> SourceBlock {
        SourceBlock::new(path, content, 0, "ts")
    }

    fn candidate<'a>(result: &'a ScanResult, name: &str) -> &'a Candidate {
        result
            .candidates
            .iter()
            .find(|candidate| candidate.name == name && candidate.kind == "function")
            .expect("candidate")
    }

    #[test]
    fn export_alone_is_not_usage() {
        let fixture = Fixture::new();
        fixture.file("src/decl.ts", "export function onlyExport() {}\n");
        let result = scan(
            &fixture.root,
            &[block("src/decl.ts", "export function onlyExport() {}\n")],
        );
        assert!(!candidate(&result, "onlyExport").local_used);
        assert!(candidate(&result, "onlyExport").exported);
    }

    #[test]
    fn reexport_alone_is_not_usage() {
        let fixture = Fixture::new();
        fixture.file("src/value.ts", "export function value() {}\n");
        fixture.file("src/index.ts", "export { value } from './value';\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/value.ts", "export function value() {}\n"),
                block("src/index.ts", "export { value } from './value';\n"),
            ],
        );
        let value = candidate(&result, "value");
        assert!(!value.local_used);
        assert_eq!(value.reexport_locations, ["src/index.ts:1"]);
    }

    #[test]
    fn consumer_through_reexport_marks_target_used() {
        let fixture = Fixture::new();
        fixture.file("src/value.ts", "export function value() {}\n");
        fixture.file("src/index.ts", "export { value } from './value';\n");
        fixture.file(
            "src/consumer.ts",
            "import { value } from './index'; value();\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/value.ts", "export function value() {}\n"),
                block("src/index.ts", "export { value } from './value';\n"),
                block(
                    "src/consumer.ts",
                    "import { value } from './index'; value();\n",
                ),
            ],
        );
        assert!(candidate(&result, "value").local_used);
        assert!(result.used_files.contains("src/value.ts"));
    }

    #[test]
    fn consumer_through_default_barrel_marks_vue_file_used() {
        let fixture = Fixture::new();
        fixture.file("src/component.vue", "<script setup lang=\"ts\"></script>\n");
        fixture.file(
            "src/index.ts",
            "export { default } from './component.vue';\n",
        );
        fixture.file(
            "src/consumer.ts",
            "import Component from './index'; void Component;\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/component.vue", ""),
                block(
                    "src/index.ts",
                    "export { default } from './component.vue';\n",
                ),
                block(
                    "src/consumer.ts",
                    "import Component from './index'; void Component;\n",
                ),
            ],
        );
        assert!(result.used_files.contains("src/component.vue"));
    }

    #[test]
    fn default_vue_reexport_records_file_location_without_usage() {
        let fixture = Fixture::new();
        fixture.file("src/component.vue", "<script setup lang=\"ts\"></script>\n");
        fixture.file(
            "src/index.ts",
            "export { default } from './component.vue';\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/component.vue", ""),
                block(
                    "src/index.ts",
                    "export { default } from './component.vue';\n",
                ),
            ],
        );
        assert!(!result.used_files.contains("src/component.vue"));
        assert_eq!(
            result
                .file_reexports
                .get("src/component.vue")
                .map(|locations| locations
                    .iter()
                    .map(ReexportLocation::display)
                    .collect::<Vec<_>>()),
            Some(vec!["src/index.ts:1".to_owned()])
        );
    }

    #[test]
    fn unused_import_does_not_mark_symbol_used() {
        let fixture = Fixture::new();
        fixture.file("src/value.ts", "export function value() {}\n");
        fixture.file("src/consumer.ts", "import { value } from './value';\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/value.ts", "export function value() {}\n"),
                block("src/consumer.ts", "import { value } from './value';\n"),
            ],
        );
        assert!(!candidate(&result, "value").local_used);
        assert!(result.used_files.contains("src/value.ts"));
    }

    #[test]
    fn self_reference_does_not_count_as_external_use() {
        let fixture = Fixture::new();
        fixture.file(
            "src/recursive.ts",
            "export function recursive() { recursive(); }\n",
        );
        let result = scan(
            &fixture.root,
            &[block(
                "src/recursive.ts",
                "export function recursive() { recursive(); }\n",
            )],
        );
        assert!(!candidate(&result, "recursive").local_used);
    }

    #[test]
    fn registry_value_counts_as_function_use() {
        let fixture = Fixture::new();
        let source = "function handler() {}\nconst handlers = { handler };\nhandlers[action]();\n";
        fixture.file("src/registry.ts", source);
        let result = scan(&fixture.root, &[block("src/registry.ts", source)]);
        assert!(candidate(&result, "handler").local_used);
    }

    #[test]
    fn local_call_records_directed_exact_edge() {
        let fixture = Fixture::new();
        let source = "function callee() {}\nfunction caller() { callee(); }\n";
        fixture.file("src/calls.ts", source);
        let result = scan(&fixture.root, &[block("src/calls.ts", source)]);
        let caller = candidate(&result, "caller");
        let callee = candidate(&result, "callee");
        assert!(result.edges.iter().any(|edge| {
            edge.source == caller.id
                && edge.target == callee.id
                && edge.kind == "call"
                && edge.confidence == "exact"
        }));
    }

    #[test]
    fn assigned_result_is_not_the_calling_symbol() {
        let fixture = Fixture::new();
        let source =
            "function callee() {}\nfunction caller() { const result = callee(); return result; }\n";
        fixture.file("src/calls.ts", source);
        let result = scan(&fixture.root, &[block("src/calls.ts", source)]);
        let caller = candidate(&result, "caller");
        let callee = candidate(&result, "callee");
        let assigned = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "variable" && candidate.name == "result")
            .expect("assigned variable");
        assert!(result.edges.iter().any(|edge| {
            edge.source == caller.id && edge.target == callee.id && edge.kind == "call"
        }));
        assert!(
            !result
                .edges
                .iter()
                .any(|edge| edge.source == assigned.id && edge.target == callee.id)
        );
    }

    #[test]
    fn new_expression_records_class_instantiation() {
        let fixture = Fixture::new();
        let source = "class Service {}\nfunction create() { return new Service(); }\n";
        fixture.file("src/service.ts", source);
        let result = scan(&fixture.root, &[block("src/service.ts", source)]);
        let caller = candidate(&result, "create");
        let class = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "class" && candidate.name == "Service")
            .expect("class");
        assert!(result.edges.iter().any(|edge| {
            edge.source == caller.id
                && edge.target == class.id
                && edge.kind == "instantiates"
                && edge.confidence == "exact"
        }));
    }

    #[test]
    fn type_only_import_has_distinct_graph_edge() {
        let fixture = Fixture::new();
        fixture.file("src/model.ts", "export interface Model {}\n");
        fixture.file(
            "src/consumer.ts",
            "import type { Model } from './model';\nconst value: Model | null = null;\n",
        );
        fixture.file(
            "src/unused-consumer.ts",
            "import type { Model } from './model';\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/model.ts", "export interface Model {}\n"),
                block(
                    "src/consumer.ts",
                    "import type { Model } from './model';\nconst value: Model | null = null;\n",
                ),
                block(
                    "src/unused-consumer.ts",
                    "import type { Model } from './model';\n",
                ),
            ],
        );
        assert!(result.edges.iter().any(|edge| {
            edge.target == "file::src/model.ts" && edge.kind == "type-import" && edge.mode == "type"
        }));
        assert!(
            !result
                .edges
                .iter()
                .any(|edge| edge.source == "file::src/unused-consumer.ts"
                    && edge.target == "file::src/model.ts")
        );
    }

    #[test]
    fn type_star_reexport_stays_type_only() {
        let fixture = Fixture::new();
        fixture.file("src/model.ts", "export interface Model {}\n");
        fixture.file("src/index.ts", "export type * from './model';\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/model.ts", "export interface Model {}\n"),
                block("src/index.ts", "export type * from './model';\n"),
            ],
        );
        assert!(result.edges.iter().any(|edge| {
            edge.source == "file::src/index.ts"
                && edge.target == "file::src/model.ts"
                && edge.kind == "reexport"
                && edge.mode == "type"
        }));
        assert!(
            result
                .file_reexports
                .get("src/model.ts")
                .is_some_and(|locations| locations.iter().all(|location| location.type_only))
        );
    }

    #[test]
    fn constructor_call_is_owned_by_constructor() {
        let fixture = Fixture::new();
        let source = "function init() {}\nclass Service { constructor() { init(); } }\n";
        fixture.file("src/service.ts", source);
        let result = scan(&fixture.root, &[block("src/service.ts", source)]);
        let constructor = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "constructor")
            .expect("constructor");
        let init = candidate(&result, "init");
        assert!(result.edges.iter().any(|edge| {
            edge.source == constructor.id
                && edge.target == init.id
                && edge.kind == "call"
                && edge.confidence == "exact"
        }));
    }

    #[test]
    fn write_only_variable_is_unused() {
        let fixture = Fixture::new();
        let source = "let value: number;\nvalue = 1;\n";
        fixture.file("src/value.ts", source);
        let result = scan(&fixture.root, &[block("src/value.ts", source)]);
        let value = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "variable" && candidate.name == "value")
            .expect("variable");
        assert!(!value.local_used);
    }

    #[test]
    fn namespace_static_reference_uses_only_selected_export() {
        let fixture = Fixture::new();
        fixture.file(
            "src/value.ts",
            "export function one() {}\nexport function two() {}\n",
        );
        fixture.file(
            "src/consumer.ts",
            "import * as values from './value'; values.one();\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block(
                    "src/value.ts",
                    "export function one() {}\nexport function two() {}\n",
                ),
                block(
                    "src/consumer.ts",
                    "import * as values from './value'; values.one();\n",
                ),
            ],
        );
        assert!(candidate(&result, "one").local_used);
        assert!(!candidate(&result, "two").local_used);
    }

    #[test]
    fn namespace_dynamic_reference_protects_potential_exports() {
        let fixture = Fixture::new();
        fixture.file(
            "src/value.ts",
            "export function one() {}\nexport function two() {}\n",
        );
        fixture.file(
            "src/consumer.ts",
            "import * as values from './value'; values[action]();\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block(
                    "src/value.ts",
                    "export function one() {}\nexport function two() {}\n",
                ),
                block(
                    "src/consumer.ts",
                    "import * as values from './value'; values[action]();\n",
                ),
            ],
        );
        for name in ["one", "two"] {
            let target = candidate(&result, name);
            assert!(target.local_used);
            assert!(result.edges.iter().any(|edge| {
                edge.target == target.id && edge.kind == "call" && edge.confidence == "potential"
            }));
        }
    }

    #[test]
    fn commonjs_require_keeps_runtime_owner() {
        let fixture = Fixture::new();
        fixture.file("src/target.ts", "export function target() {}\n");
        fixture.file(
            "src/consumer.ts",
            "export function dead() { require('./target'); }\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/target.ts", "export function target() {}\n"),
                block(
                    "src/consumer.ts",
                    "export function dead() { require('./target'); }\n",
                ),
            ],
        );
        let dead = candidate(&result, "dead");
        assert!(result.edges.iter().any(|edge| {
            edge.source == dead.id && edge.target == "file::src/target.ts" && edge.kind == "require"
        }));
    }

    #[test]
    fn inner_same_name_is_not_exported() {
        let fixture = Fixture::new();
        fixture.file(
            "src/value.ts",
            "export const value = 1;\nfunction scope() { const value = 2; return value; }\n",
        );
        let result = scan(
            &fixture.root,
            &[block(
                "src/value.ts",
                "export const value = 1;\nfunction scope() { const value = 2; return value; }\n",
            )],
        );
        let values = result
            .candidates
            .iter()
            .filter(|candidate| candidate.name == "value")
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 2);
        assert_eq!(
            values.iter().filter(|candidate| candidate.exported).count(),
            1
        );
    }

    #[test]
    fn exported_class_marks_method_as_library_api() {
        let fixture = Fixture::new();
        fixture.file(
            "src/service.ts",
            "export class Service { run() {} private hidden() {} #secret() {} }\n",
        );
        let result = scan(
            &fixture.root,
            &[block(
                "src/service.ts",
                "export class Service { run() {} private hidden() {} #secret() {} }\n",
            )],
        );
        let method = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "method" && candidate.name == "run")
            .expect("method");
        assert!(method.exported);
        assert_eq!(method.qualified_name, "Service.run");
        for name in ["hidden", "secret"] {
            let private_method = result
                .candidates
                .iter()
                .find(|candidate| candidate.kind == "method" && candidate.name == name)
                .expect("private method");
            assert!(!private_method.exported);
        }
    }

    #[test]
    fn anonymous_default_class_marks_public_method_as_library_api() {
        let fixture = Fixture::new();
        fixture.file("src/service.ts", "export default class { run() {} }\n");
        let result = scan(
            &fixture.root,
            &[block(
                "src/service.ts",
                "export default class { run() {} }\n",
            )],
        );
        let method = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "method" && candidate.name == "run")
            .expect("method");
        assert!(method.exported);
        assert_eq!(method.qualified_name, "<default>.run");
    }

    #[test]
    fn default_exported_class_expression_marks_public_method_as_library_api() {
        let fixture = Fixture::new();
        let source = "const Service = class { run() {} };\nexport default Service;\n";
        fixture.file("src/service.ts", source);
        let result = scan(&fixture.root, &[block("src/service.ts", source)]);
        let method = result
            .candidates
            .iter()
            .find(|candidate| candidate.kind == "method" && candidate.name == "run")
            .expect("method");
        assert!(method.exported);
        assert_eq!(method.qualified_name, "Service.run");
    }

    #[test]
    fn alias_dynamic_import_marks_matching_vue_files_used() {
        let fixture = Fixture::new();
        fixture.file(
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        fixture.file("src/views/One.vue", "<script setup lang=\"ts\"></script>\n");
        fixture.file(
            "src/loader.ts",
            "export const load = (name: string) => import(`@/views/${name}.vue`);\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/views/One.vue", ""),
                block(
                    "src/loader.ts",
                    "export const load = (name: string) => import(`@/views/${name}.vue`);\n",
                ),
            ],
        );
        assert!(result.used_files.contains("src/views/One.vue"));
        assert!(result.coverage_boundaries.is_empty());
    }

    #[test]
    fn static_alias_dynamic_import_marks_exact_vue_file_used() {
        let fixture = Fixture::new();
        fixture.file(
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        fixture.file("src/views/One.vue", "<script setup lang=\"ts\"></script>\n");
        fixture.file("src/loader.ts", "void import('@/views/One.vue');\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/views/One.vue", ""),
                block("src/loader.ts", "void import('@/views/One.vue');\n"),
            ],
        );
        assert!(result.used_files.contains("src/views/One.vue"));
    }

    #[test]
    fn static_dynamic_import_protects_unresolved_exports() {
        let fixture = Fixture::new();
        fixture.file("src/target.ts", "export function run() {}\n");
        fixture.file("src/loader.ts", "void import('./target');\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/target.ts", "export function run() {}\n"),
                block("src/loader.ts", "void import('./target');\n"),
            ],
        );
        let run = candidate(&result, "run");
        assert!(result.dynamic_unknown.contains(&run.id));
    }

    #[test]
    fn unbounded_dynamic_import_marks_coverage_without_fabricating_file_usage() {
        let fixture = Fixture::new();
        fixture.file("src/one.ts", "export const one = 1;\n");
        fixture.file("src/two.ts", "export const two = 2;\n");
        fixture.file("src/loader.ts", "void import(runtimePath);\n");
        let result = scan(
            &fixture.root,
            &[
                block("src/one.ts", "export const one = 1;\n"),
                block("src/two.ts", "export const two = 2;\n"),
                block("src/loader.ts", "void import(runtimePath);\n"),
            ],
        );
        assert_eq!(
            result
                .coverage_boundaries
                .values()
                .next()
                .map(String::as_str),
            Some("dynamic-import-boundary")
        );
        assert!(!result.used_files.contains("src/one.ts"));
        assert!(!result.used_files.contains("src/two.ts"));
    }

    #[test]
    fn consumer_only_block_keeps_module_and_usage_evidence() {
        let fixture = Fixture::new();
        fixture.file("src/value.ts", "export function value() {}\n");
        fixture.file(
            "src/consumer.ts",
            "import { value } from './value'; value();\n",
        );
        let result = scan(
            &fixture.root,
            &[
                block("src/value.ts", "export function value() {}\n"),
                SourceBlock::consumer_only(
                    "src/consumer.ts",
                    "import { value } from './value'; value();\n",
                    0,
                    "ts",
                ),
            ],
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|candidate| candidate.path == "src/consumer.ts")
                .count(),
            0
        );
        assert!(
            result
                .modules
                .iter()
                .any(|module| module.path == "src/consumer.ts")
        );
        assert!(candidate(&result, "value").local_used);
    }

    #[test]
    fn parameter_catch_and_class_field_are_not_candidates() {
        let fixture = Fixture::new();
        let source = "function outer(parameter) { let local = parameter; try {} catch (caught) { return caught; } }\nclass Service { field = outer; }\n";
        fixture.file("src/boundaries.ts", source);
        let result = scan(&fixture.root, &[block("src/boundaries.ts", source)]);
        assert!(
            result
                .candidates
                .iter()
                .any(|candidate| candidate.name == "outer")
        );
        assert!(
            result
                .candidates
                .iter()
                .any(|candidate| candidate.name == "local")
        );
        for name in ["parameter", "caught", "field"] {
            assert!(
                !result
                    .candidates
                    .iter()
                    .any(|candidate| candidate.name == name)
            );
        }
    }

    #[test]
    fn runtime_only_functions_and_object_accessors_are_collected() {
        let fixture = Fixture::new();
        let source = "declare function declared(): void;\nfunction overload(value: string): void;\nfunction overload(value: number) {}\nabstract class Abstract { abstract run(): void; }\nconst handlers = { run() {}, get value() { return 1; }, set value(next) {} };\n";
        fixture.file("src/runtime.ts", source);
        let result = scan(&fixture.root, &[block("src/runtime.ts", source)]);
        assert!(
            !result
                .candidates
                .iter()
                .any(|candidate| candidate.name == "declared")
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|candidate| candidate.name == "overload" && candidate.kind == "function")
                .count(),
            1
        );
        assert!(
            !result
                .candidates
                .iter()
                .any(|candidate| candidate.qualified_name == "Abstract.run")
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|candidate| candidate.name == "value" && candidate.kind == "method")
                .count(),
            2
        );
        assert!(
            result
                .candidates
                .iter()
                .any(|candidate| candidate.name == "run" && candidate.kind == "method")
        );
    }

    #[test]
    fn reference_edges_include_owner_mode_and_provenance() {
        let fixture = Fixture::new();
        let source = "function target() {}\nfunction owner() { const value = target; let typed: typeof target; return value; }\n";
        fixture.file("src/owners.ts", source);
        let result = scan(&fixture.root, &[block("src/owners.ts", source)]);
        let owner = candidate(&result, "owner");
        let target = candidate(&result, "target");
        assert!(result.edges.iter().any(|edge| {
            edge.source == owner.id
                && edge.target == target.id
                && edge.kind == "reference"
                && edge.mode == "runtime"
                && edge.provenance == "oxc"
        }));
        assert!(result.edges.iter().any(|edge| {
            edge.source == owner.id
                && edge.target == target.id
                && edge.kind == "reference"
                && edge.mode == "type"
        }));
    }

    #[test]
    fn candidates_record_callable_initializer_owner_and_effect() {
        let fixture = Fixture::new();
        let source = "function target() {} const pure = 1; const alias = target; const frozen = { active: true, nested: [1, null] } as const; const setup = register(); const callback = () => { target(); return pure; };\n";
        fixture.file("src/initializers.ts", source);
        let result = scan(&fixture.root, &[block("src/initializers.ts", source)]);
        let pure = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "pure")
            .expect("pure");
        let setup = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "setup")
            .expect("setup");
        let alias = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "alias")
            .expect("alias");
        let frozen = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "frozen")
            .expect("frozen");
        let callback = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "callback")
            .expect("callback");
        assert_eq!(pure.initializer_effect, "side-effect-free");
        assert_eq!(alias.initializer_effect, "unknown");
        assert_eq!(frozen.initializer_effect, "side-effect-free");
        assert_eq!(setup.initializer_effect, "side-effectful");
        assert!(!pure.callable);
        assert!(callback.callable);
        assert!(setup.initializer_owner.is_some());
        assert!(result.execution_owners.iter().any(|owner| {
            owner.id == callback.initializer_owner.as_deref().unwrap_or_default()
                && owner.kind == "variable-initializer"
        }));
        let target = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "target")
            .expect("target");
        assert!(result.edges.iter().any(|edge| {
            edge.source == callback.id && edge.target == target.id && edge.kind == "call"
        }));
    }

    #[test]
    fn decorated_and_dynamic_computed_methods_are_unknown() {
        let fixture = Fixture::new();
        let source = "declare function register(...args: unknown[]): MethodDecorator; declare function key(): string; class Service { @register decorated() {} [key()]() {} }\n";
        fixture.file("src/decorated.ts", source);
        let result = scan(&fixture.root, &[block("src/decorated.ts", source)]);

        for name in ["decorated", "<computed>"] {
            let method = result
                .candidates
                .iter()
                .find(|candidate| candidate.kind == "method" && candidate.name == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(
                method.coverage_reason.as_deref(),
                Some("runtime-dispatch-ambiguous")
            );
        }
    }
}
