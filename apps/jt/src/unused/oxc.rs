use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::{
    AstKind,
    ast::{Expression, TSAccessibility},
};
use oxc_parser::Parser;
use oxc_resolver::{AliasValue, ResolveOptions, Resolver, TsconfigOptions, TsconfigReferences};
use oxc_semantic::{SemanticBuilder, SymbolFlags};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::{
    ExportExportName, ExportImportName, ExportLocalName, ImportImportName,
};

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
        }
    }
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
    pub(crate) language: String,
    pub(crate) qualified_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportSummary {
    pub(crate) module: String,
    pub(crate) import_name: Option<String>,
    pub(crate) local_name: Option<String>,
    pub(crate) local_used: bool,
    pub(crate) reexport_only: bool,
    pub(crate) type_only: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
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
    pub(crate) expression: String,
    pub(crate) static_specifier: Option<String>,
    pub(crate) pattern: Option<DynamicPattern>,
    pub(crate) unbounded: bool,
    pub(crate) start: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModuleSummary {
    pub(crate) path: String,
    pub(crate) imports: Vec<ImportSummary>,
    pub(crate) local_exports: Vec<ExportSummary>,
    pub(crate) reexports: Vec<ExportSummary>,
    pub(crate) star_exports: Vec<StarExportSummary>,
    pub(crate) dynamic_imports: Vec<DynamicImportSummary>,
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
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScanResult {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) modules: Vec<ModuleSummary>,
    pub(crate) used_files: BTreeSet<String>,
    pub(crate) unknown_files: BTreeSet<String>,
    pub(crate) dynamic_unknown: BTreeSet<String>,
    pub(crate) file_reexports: BTreeMap<String, Vec<String>>,
    pub(crate) edges: Vec<GraphEdge>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) coverage_limited: bool,
}

struct ParsedBlock {
    candidates: Vec<Candidate>,
    module: ModuleSummary,
    edges: Vec<GraphEdge>,
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
        result.diagnostics.extend(parsed.diagnostics);
        result.edges.extend(parsed.edges);
        for candidate in parsed.candidates {
            candidates
                .entry(candidate.id.clone())
                .and_modify(|current| {
                    current.local_used |= candidate.local_used;
                    current.exported |= candidate.exported;
                    current.unknown |= candidate.unknown;
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
    let semantic_result = SemanticBuilder::new().build(program);
    let semantic_error_count = semantic_result.errors.len();
    let semantic = semantic_result.semantic;
    let mut candidates = Vec::new();
    let mut symbol_ids = HashMap::<String, String>::new();
    let scoping = semantic.scoping();
    let language = language_name(block, source_type);
    let parse_unknown = parser_panicked || parser_error_count > 0 || semantic_error_count > 0;

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
        let name = scoping.symbol_name(symbol_id).to_owned();
        let span = scoping.symbol_span(symbol_id);
        let start = block.offset + span.start as usize;
        let (line, column) = line_column(&block.content, span.start as usize);
        let id = candidate_id(&path, kind, &name, start);
        let declaration_function = (kind == "function").then(|| {
            semantic
                .nodes()
                .ancestors(semantic.symbol_declaration(symbol_id).id())
                .find_map(|node| match node.kind() {
                    AstKind::Function(function) => Some(function.span),
                    _ => None,
                })
        });
        let local_used = scoping.get_resolved_references(symbol_id).any(|reference| {
            let span = semantic.reference_span(reference);
            (reference.is_read() || reference.is_type())
                && !is_export_reference(span, &module_record)
                && !declaration_function
                    .flatten()
                    .is_some_and(|declaration| declaration.contains_inclusive(span))
        });
        let exported = scoping.get_root_binding(&name) == Some(symbol_id)
            && symbol_exported(&name, &module_record);
        symbol_ids.insert(format!("{symbol_id:?}"), id.clone());
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
            language: language.clone(),
            qualified_name: name,
        });
    }

    for node in semantic.nodes() {
        let AstKind::MethodDefinition(method) = node.kind() else {
            continue;
        };
        let Some(name) = method.key.name() else {
            continue;
        };
        let name = name.into_owned();
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
        candidates.push(Candidate {
            id: candidate_id(&path, kind, &name, start),
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
            language: language.clone(),
            qualified_name: owner_name.map_or_else(
                || format!("<default>.{name}"),
                |owner| format!("{owner}.{name}"),
            ),
        });
    }

    let edges = local_call_edges(&semantic, block, &path, &symbol_ids, parse_unknown);

    let mut module = ModuleSummary {
        path,
        imports: Vec::new(),
        local_exports: Vec::new(),
        reexports: Vec::new(),
        star_exports: Vec::new(),
        dynamic_imports: Vec::new(),
        has_parse_errors: parse_unknown,
    };

    for entry in &module_record.import_entries {
        let local_name = entry.local_name.name.to_string();
        let symbol_id = scoping.get_root_binding(&local_name);
        let local_used = symbol_id.is_some_and(|symbol_id| {
            scoping.get_resolved_references(symbol_id).any(|reference| {
                (reference.is_read() || reference.is_type())
                    && !is_export_reference(semantic.reference_span(reference), &module_record)
            })
        });
        let any_reference = symbol_id
            .is_some_and(|symbol_id| !scoping.get_resolved_references(symbol_id).next().is_none());
        let (line, column) = line_column(&block.content, entry.statement_span.start as usize);
        module.imports.push(ImportSummary {
            module: entry.module_request.name.to_string(),
            import_name: import_name(&entry.import_name),
            local_name: Some(local_name),
            local_used,
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
            start: block.offset + entry.span.start as usize,
            line,
            column,
        });
    }

    for dynamic in &module_record.dynamic_imports {
        module
            .dynamic_imports
            .push(dynamic_import(&block.content, *dynamic, block.offset));
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
        diagnostics,
    }
}

fn local_call_edges(
    semantic: &oxc_semantic::Semantic<'_>,
    block: &SourceBlock,
    path: &str,
    symbol_ids: &HashMap<String, String>,
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
        let source = semantic
            .nodes()
            .ancestors(node.id())
            .find_map(|ancestor| match ancestor.kind() {
                AstKind::Function(function) => function
                    .id
                    .as_ref()
                    .and_then(|identifier| identifier.symbol_id.get())
                    .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
                    .cloned(),
                AstKind::MethodDefinition(method) => method.key.name().map(|name| {
                    let kind = if name == "constructor" {
                        "constructor"
                    } else {
                        "method"
                    };
                    candidate_id(
                        path,
                        kind,
                        &name,
                        block.offset + method.key.span().start as usize,
                    )
                }),
                AstKind::VariableDeclarator(declarator) => declarator
                    .init
                    .as_ref()
                    .is_some_and(|initializer| {
                        matches!(
                            initializer,
                            Expression::ArrowFunctionExpression(_)
                                | Expression::FunctionExpression(_)
                        )
                    })
                    .then(|| declarator.id.get_binding_identifier())
                    .flatten()
                    .and_then(|identifier| identifier.symbol_id.get())
                    .and_then(|symbol_id| symbol_ids.get(&format!("{symbol_id:?}")))
                    .cloned(),
                _ => None,
            })
            .unwrap_or_else(|| format!("file::{path}"));
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
        });
    }
    edges
}

fn merge_module(current: &mut ModuleSummary, mut incoming: ModuleSummary) {
    current.imports.append(&mut incoming.imports);
    current.local_exports.append(&mut incoming.local_exports);
    current.reexports.append(&mut incoming.reexports);
    current.star_exports.append(&mut incoming.star_exports);
    current
        .dynamic_imports
        .append(&mut incoming.dynamic_imports);
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
            result.edges.push(GraphEdge {
                source: format!("file::{}", module.path),
                target: format!("file::{target}"),
                kind: if import.type_only {
                    "type-import"
                } else {
                    "import"
                }
                .to_owned(),
                path: module.path.clone(),
                start: import.start,
                line: import.line,
                column: import.column,
                confidence: "exact".to_owned(),
            });
            if import.reexport_only {
                continue;
            }
            result.used_files.insert(target.clone());
            if import.type_only {
                continue;
            }
            if !import.local_used {
                continue;
            }
            let mut visited = HashSet::new();
            if let Some(import_name) = import.import_name.as_deref() {
                mark_export_use(
                    root,
                    &resolver,
                    &module_paths,
                    &modules,
                    &candidate_ids,
                    &module_candidates,
                    result,
                    &target,
                    import_name,
                    &mut visited,
                );
            } else if let Some(indices) = module_candidates.get(&target) {
                for &index in indices {
                    result.candidates[index].unknown = true;
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
                        source: format!("file::{}", module.path),
                        target: format!("file::{target}"),
                        kind: "dynamic-import".to_owned(),
                        path: module.path.clone(),
                        start: dynamic.start,
                        line: dynamic.line,
                        column: dynamic.column,
                        confidence: "exact".to_owned(),
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
                        source: format!("file::{}", module.path),
                        target: format!("file::{target}"),
                        kind: "dynamic-import".to_owned(),
                        path: module.path.clone(),
                        start: dynamic.start,
                        line: dynamic.line,
                        column: dynamic.column,
                        confidence: "potential".to_owned(),
                    });
                }
                continue;
            }
            result.coverage_limited = true;
            for target in &module_paths {
                result.used_files.insert(target.clone());
                mark_dynamic_candidates(&module_candidates, result, target);
            }
            result.diagnostics.push(format!(
                "{}: dynamic import has no statically bounded target",
                module.path
            ));
        }
    }

    for module in modules.values() {
        for export in &module.reexports {
            let Some(target) = export.module.as_deref().and_then(|specifier| {
                resolve_request(root, &resolver, &module_paths, &module.path, specifier)
            }) else {
                continue;
            };
            add_file_reexport(result, &target, &module.path, export.line);
            result.edges.push(GraphEdge {
                source: format!("file::{}", module.path),
                target: format!("file::{target}"),
                kind: "reexport".to_owned(),
                path: module.path.clone(),
                start: export.start,
                line: export.line,
                column: export.column,
                confidence: "exact".to_owned(),
            });
            let Some(imported_name) = export.imported_name.as_deref() else {
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
            add_file_reexport(result, &target, &module.path, export.line);
            result.edges.push(GraphEdge {
                source: format!("file::{}", module.path),
                target: format!("file::{target}"),
                kind: "reexport".to_owned(),
                path: module.path.clone(),
                start: export.start,
                line: export.line,
                column: export.column,
                confidence: "exact".to_owned(),
            });
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

fn add_file_reexport(result: &mut ScanResult, target: &str, source: &str, line: usize) {
    result
        .file_reexports
        .entry(target.to_owned())
        .or_default()
        .push(format!("{source}:{line}"));
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

#[allow(clippy::too_many_arguments)]
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
            result.file_reexports.get("src/component.vue"),
            Some(&vec!["src/index.ts:1".to_owned()])
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
        let result = scan(
            &fixture.root,
            &[
                block("src/model.ts", "export interface Model {}\n"),
                block(
                    "src/consumer.ts",
                    "import type { Model } from './model';\nconst value: Model | null = null;\n",
                ),
            ],
        );
        assert!(result.edges.iter().any(|edge| {
            edge.source == "file::src/consumer.ts"
                && edge.target == "file::src/model.ts"
                && edge.kind == "type-import"
        }));
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
    fn namespace_reference_does_not_use_every_export() {
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
        assert!(!candidate(&result, "one").local_used);
        assert!(!candidate(&result, "two").local_used);
        assert!(candidate(&result, "one").unknown);
        assert!(candidate(&result, "two").unknown);
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
        assert!(!result.coverage_limited);
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
    fn unbounded_dynamic_import_conservatively_uses_all_files() {
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
        assert!(result.coverage_limited);
        assert!(result.used_files.contains("src/one.ts"));
        assert!(result.used_files.contains("src/two.ts"));
        let one = result
            .candidates
            .iter()
            .find(|candidate| candidate.name == "one")
            .expect("one");
        assert!(result.dynamic_unknown.contains(&one.id));
    }
}
