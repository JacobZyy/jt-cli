use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tree_sitter::{Node, Parser};

use super::coded_values;
use super::graph::{GraphNode, Snapshot};
use super::model::{
    Field, HttpRoute, Operation, RouteSource, RouteStatus, Schema, TargetIdentity, TypeRef,
};

const RESULT_WRAPPERS: &[&str] = &[
    "ApiResult",
    "Result",
    "Response",
    "CompletableFuture",
    "Future",
];
const CONTEXT_TYPES: &[&str] = &[
    "EmployeeUser",
    "ClientContext",
    "HttpServletRequest",
    "HttpServletResponse",
    "Principal",
];

pub struct JavaProject<'a> {
    graph: &'a Snapshot,
    sources: BTreeMap<String, String>,
    packages: HashMap<String, String>,
    imports: HashMap<String, Vec<String>>,
    type_by_fqn: HashMap<String, String>,
}

impl<'a> JavaProject<'a> {
    pub fn load(repo: &Path, graph: &'a Snapshot) -> Result<Self> {
        let mut paths = graph
            .nodes
            .values()
            .map(|node| node.file_path.clone())
            .filter(|path| path.ends_with(".java"))
            .collect::<BTreeSet<_>>();
        let mut sources = BTreeMap::new();
        let mut packages = HashMap::new();
        let mut imports = HashMap::new();
        for path in &paths {
            let source = fs::read_to_string(repo.join(path))
                .with_context(|| format!("read Java source {path}"))?;
            packages.insert(path.clone(), java_package(&source).unwrap_or_default());
            imports.insert(path.clone(), java_imports(&source));
            sources.insert(path.clone(), source);
        }
        paths.clear();
        let type_by_fqn = graph
            .nodes
            .values()
            .filter(|node| matches!(node.kind.as_str(), "class" | "interface" | "enum"))
            .map(|node| (normalize_fqn(&node.qualified_name), node.id.clone()))
            .collect();
        Ok(Self {
            graph,
            sources,
            packages,
            imports,
            type_by_fqn,
        })
    }

    pub fn build_contracts(
        &self,
        target: &TargetIdentity,
    ) -> Result<(Vec<Operation>, BTreeMap<String, Schema>)> {
        let mut schemas = BTreeMap::new();
        let mut operations = Vec::new();
        let mut keys = BTreeSet::new();
        for facade in self.facades() {
            let mut methods = self.graph.contained(&facade.id, "method");
            methods.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.signature.cmp(&right.signature))
            });
            for method in methods {
                let operation = self.operation(target, facade, method, &mut schemas)?;
                if !keys.insert(operation.key.clone()) {
                    bail!(
                        "overloaded Facade operation is unsupported: {} ({})",
                        operation.key,
                        operation.signature
                    );
                }
                operations.push(operation);
            }
        }
        operations.sort_by(|left, right| left.key.cmp(&right.key));
        Ok((operations, schemas))
    }

    pub fn source(&self, path: &str) -> Result<&str> {
        self.sources
            .get(path)
            .map(String::as_str)
            .with_context(|| format!("Java source not indexed: {path}"))
    }

    pub fn graph(&self) -> &Snapshot {
        self.graph
    }

    pub fn node_for_fqn(&self, fqn: &str) -> Option<&GraphNode> {
        self.type_by_fqn
            .get(&fqn.replace("::", "."))
            .and_then(|id| self.graph.nodes.get(id))
    }

    pub fn resolve_type(
        &self,
        file_path: &str,
        owner_fqn: &str,
        type_ref: &TypeRef,
    ) -> Option<&GraphNode> {
        let name = type_ref.name.replace("::", ".");
        if let Some(id) = self.type_by_fqn.get(&name) {
            return self.graph.nodes.get(id);
        }
        let simple = type_ref.simple_name();
        if let Some(import) = self.imports.get(file_path).and_then(|imports| {
            imports
                .iter()
                .find(|value| value.rsplit('.').next() == Some(simple))
        }) {
            if let Some(id) = self.type_by_fqn.get(import) {
                return self.graph.nodes.get(id);
            }
        }
        let mut lexical = owner_fqn.replace("::", ".");
        loop {
            let candidate = format!("{lexical}.{simple}");
            if let Some(id) = self.type_by_fqn.get(&candidate) {
                return self.graph.nodes.get(id);
            }
            let Some((parent, _)) = lexical.rsplit_once('.') else {
                break;
            };
            lexical = parent.to_owned();
        }
        if let Some(package) = self.packages.get(file_path) {
            let candidate = format!("{package}.{simple}");
            if let Some(id) = self.type_by_fqn.get(&candidate) {
                return self.graph.nodes.get(id);
            }
        }
        let candidates = self
            .graph
            .candidates(simple)
            .into_iter()
            .filter(|node| matches!(node.kind.as_str(), "class" | "interface" | "enum"))
            .collect::<Vec<_>>();
        (candidates.len() == 1).then(|| candidates[0])
    }

    fn facades(&self) -> Vec<&GraphNode> {
        let mut result = self
            .graph
            .nodes
            .values()
            .filter(|node| {
                node.kind == "interface"
                    && node.name.ends_with("Facade")
                    && node.file_path.contains("/contract/")
                    && self.sources.get(&node.file_path).is_some_and(|source| {
                        source.contains("@ServiceContract") || source.contains("ServiceContract")
                    })
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
        result
    }

    fn operation(
        &self,
        target: &TargetIdentity,
        facade: &GraphNode,
        method: &GraphNode,
        schemas: &mut BTreeMap<String, Schema>,
    ) -> Result<Operation> {
        let (mut response, parameters) = parse_method_signature(&method.signature)
            .with_context(|| format!("parse operation signature: {}", method.qualified_name))?;
        self.qualify_type(&method.file_path, &facade.qualified_name, &mut response);
        let response = unwrap_result(response);
        let mut request_candidates = Vec::new();
        for mut parameter in parameters {
            self.qualify_type(&method.file_path, &facade.qualified_name, &mut parameter);
            if !CONTEXT_TYPES.contains(&parameter.simple_name())
                && self
                    .resolve_type(&method.file_path, &facade.qualified_name, &parameter)
                    .is_some()
            {
                request_candidates.push(parameter);
            }
        }
        let mut warnings = Vec::new();
        let request = match request_candidates.as_slice() {
            [] => None,
            [request] => Some(request.clone()),
            requests => {
                warnings.push(format!(
                    "multiple request DTO candidates; selected last parameter: {}",
                    requests
                        .iter()
                        .map(TypeRef::render_java)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                requests.last().cloned()
            }
        };
        let request_schema = request
            .as_ref()
            .and_then(|value| self.root_schema(&method.file_path, &facade.qualified_name, value));
        let response_schema =
            self.root_schema(&method.file_path, &facade.qualified_name, &response);
        if let Some(request) = &request {
            self.collect_type_schemas(
                &method.file_path,
                &facade.qualified_name,
                request,
                schemas,
                &mut HashSet::new(),
            )?;
        }
        self.collect_type_schemas(
            &method.file_path,
            &facade.qualified_name,
            &response,
            schemas,
            &mut HashSet::new(),
        )?;

        let facade_fqn = normalize_fqn(&facade.qualified_name);
        let key = format!("{}#{}", facade.name, method.name);
        let placeholder_path = format!(
            "/api/{}/__nlab_pending__/{}/{}",
            encode_path_segment(&target.app_name),
            encode_path_segment(&facade.name),
            encode_path_segment(&method.name)
        );
        Ok(Operation {
            key,
            facade_name: facade.name.clone(),
            facade_fqn,
            method_name: method.name.clone(),
            signature: method.signature.clone(),
            description: method
                .docstring
                .clone()
                .filter(|value| !value.trim().is_empty()),
            contract_source: method.file_path.clone(),
            request,
            response,
            request_schema,
            response_schema,
            service: None,
            route: HttpRoute {
                status: RouteStatus::Placeholder,
                source: RouteSource::Placeholder,
                method: "POST".to_owned(),
                path: placeholder_path,
                host: None,
            },
            semantic_patches: Vec::new(),
            warnings,
        })
    }

    fn root_schema(&self, file_path: &str, owner_fqn: &str, type_ref: &TypeRef) -> Option<String> {
        if is_type_wrapper(type_ref.simple_name()) {
            return type_ref
                .arguments
                .iter()
                .rev()
                .find_map(|argument| self.root_schema(file_path, owner_fqn, argument));
        }
        if let Some(node) = self.resolve_type(file_path, owner_fqn, type_ref) {
            if node.kind != "enum" {
                return Some(normalize_fqn(&node.qualified_name));
            }
        }
        type_ref
            .arguments
            .iter()
            .rev()
            .find_map(|argument| self.root_schema(file_path, owner_fqn, argument))
    }

    fn collect_type_schemas(
        &self,
        file_path: &str,
        owner_fqn: &str,
        type_ref: &TypeRef,
        schemas: &mut BTreeMap<String, Schema>,
        visiting: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(node) = self.resolve_type(file_path, owner_fqn, type_ref) {
            if node.kind == "class"
                && (!is_type_wrapper(type_ref.simple_name())
                    || matches!(type_ref.simple_name(), "PageList" | "Page" | "PageResult"))
            {
                self.collect_schema(&normalize_fqn(&node.qualified_name), schemas, visiting)?;
            }
        }
        for argument in &type_ref.arguments {
            self.collect_type_schemas(file_path, owner_fqn, argument, schemas, visiting)?;
        }
        Ok(())
    }

    fn collect_schema(
        &self,
        fqn: &str,
        schemas: &mut BTreeMap<String, Schema>,
        visiting: &mut HashSet<String>,
    ) -> Result<()> {
        if schemas.contains_key(fqn) || !visiting.insert(fqn.to_owned()) {
            return Ok(());
        }
        let node_id = self
            .type_by_fqn
            .get(fqn)
            .with_context(|| format!("schema type not indexed: {fqn}"))?;
        let node = &self.graph.nodes[node_id];
        let mut fields = Vec::new();
        if let Some(parent) = self.superclass(node)? {
            self.collect_schema(&parent, schemas, visiting)?;
            if let Some(parent_schema) = schemas.get(&parent) {
                fields.extend(parent_schema.fields.clone());
            }
        }
        for field in self.graph.contained(node_id, "field") {
            let mut java_type = declared_field_type(&field.signature, &field.name)
                .and_then(|value| parse_java_type(&value))
                .unwrap_or_else(|| TypeRef {
                    name: "Object".to_owned(),
                    arguments: Vec::new(),
                    array_depth: 0,
                });
            self.qualify_type(&field.file_path, &node.qualified_name, &mut java_type);
            fields.retain(|existing: &Field| existing.name != field.name);
            let description = field
                .docstring
                .clone()
                .filter(|value| !value.trim().is_empty());
            fields.push(Field {
                name: field.name.clone(),
                optional: self.field_optional(field),
                declared_values: coded_values::parse(
                    &field.name,
                    description.as_deref(),
                    &java_type,
                ),
                linked_enum: None,
                description,
                java_type: java_type.clone(),
            });
            self.collect_nested_types(
                &field.file_path,
                &node.qualified_name,
                &java_type,
                schemas,
                visiting,
            )?;
        }
        visiting.remove(fqn);
        schemas.insert(
            fqn.to_owned(),
            Schema {
                fqn: fqn.to_owned(),
                name: schema_name(fqn),
                source_path: node.file_path.clone(),
                description: node
                    .docstring
                    .clone()
                    .filter(|value| !value.trim().is_empty()),
                fields,
            },
        );
        Ok(())
    }

    fn collect_nested_types(
        &self,
        file_path: &str,
        owner_fqn: &str,
        type_ref: &TypeRef,
        schemas: &mut BTreeMap<String, Schema>,
        visiting: &mut HashSet<String>,
    ) -> Result<()> {
        if let Some(node) = self.resolve_type(file_path, owner_fqn, type_ref) {
            if node.kind == "class" {
                self.collect_schema(&normalize_fqn(&node.qualified_name), schemas, visiting)?;
            }
        }
        for argument in &type_ref.arguments {
            self.collect_nested_types(file_path, owner_fqn, argument, schemas, visiting)?;
        }
        Ok(())
    }

    fn qualify_type(&self, file_path: &str, owner_fqn: &str, type_ref: &mut TypeRef) {
        for argument in &mut type_ref.arguments {
            self.qualify_type(file_path, owner_fqn, argument);
        }
        if let Some(node) = self.resolve_type(file_path, owner_fqn, type_ref) {
            type_ref.name = normalize_fqn(&node.qualified_name);
        }
    }

    fn superclass(&self, class: &GraphNode) -> Result<Option<String>> {
        let source = self.source(&class.file_path)?;
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
        let tree = parser
            .parse(source, None)
            .with_context(|| format!("parse Java source {}", class.file_path))?;
        let declaration = descendants(tree.root_node()).into_iter().find(|node| {
            matches!(node.kind(), "class_declaration" | "record_declaration")
                && node.start_position().row + 1 == class.start_line
        });
        let Some(superclass) = declaration.and_then(|node| node.child_by_field_name("superclass"))
        else {
            return Ok(None);
        };
        let superclass_source = text_of(source, superclass);
        let text = superclass_source
            .trim()
            .trim_start_matches("extends")
            .trim();
        let Some(type_ref) = parse_java_type(text) else {
            return Ok(None);
        };
        Ok(self
            .resolve_type(&class.file_path, &class.qualified_name, &type_ref)
            .map(|node| normalize_fqn(&node.qualified_name)))
    }

    fn field_optional(&self, field: &GraphNode) -> bool {
        let Ok(source) = self.source(&field.file_path) else {
            return false;
        };
        let lines = source.lines().collect::<Vec<_>>();
        let start = field.start_line.saturating_sub(8);
        let end = field.start_line.min(lines.len());
        let declaration = lines[start..end].join("\n");
        if [
            "@NotNull",
            "@NonNull",
            "@NotEmpty",
            "@NotBlank",
            "@Size",
            "@Pattern",
            "@Min",
            "@Max",
            "@Email",
        ]
        .iter()
        .any(|annotation| declaration.contains(annotation))
        {
            return false;
        }
        declaration.contains("@Nullable")
            || declaration.contains("= null")
            || field.docstring.as_deref().is_some_and(|comment| {
                ["非必填", "可选", "可空", "可不传", "二选一", "多选一"]
                    .iter()
                    .any(|marker| comment.contains(marker))
            })
            || ["ext", "optional", "opt"]
                .iter()
                .any(|suffix| field.name.to_ascii_lowercase().ends_with(suffix))
    }
}

pub fn parse_java_type(source: &str) -> Option<TypeRef> {
    JavaTypeParser::new(source).parse()
}

struct JavaTypeParser<'a> {
    source: &'a [u8],
    offset: usize,
}

impl<'a> JavaTypeParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            offset: 0,
        }
    }

    fn parse(mut self) -> Option<TypeRef> {
        let result = self.parse_type()?;
        self.skip_space();
        (self.offset == self.source.len()).then_some(result)
    }

    fn parse_type(&mut self) -> Option<TypeRef> {
        self.skip_space();
        while self.peek() == Some(b'?') {
            self.offset += 1;
            self.skip_space();
            for keyword in [b"extends".as_slice(), b"super".as_slice()] {
                if self.source[self.offset..].starts_with(keyword) {
                    self.offset += keyword.len();
                    self.skip_space();
                }
            }
        }
        let start = self.offset;
        while self.peek().is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'.' | b':')
        }) {
            self.offset += 1;
        }
        if start == self.offset {
            return None;
        }
        let name = std::str::from_utf8(&self.source[start..self.offset])
            .ok()?
            .trim_matches(':')
            .to_owned();
        self.skip_space();
        let mut arguments = Vec::new();
        if self.peek() == Some(b'<') {
            self.offset += 1;
            loop {
                arguments.push(self.parse_type()?);
                self.skip_space();
                match self.peek()? {
                    b',' => self.offset += 1,
                    b'>' => {
                        self.offset += 1;
                        break;
                    }
                    _ => return None,
                }
            }
        }
        let mut array_depth = 0;
        loop {
            self.skip_space();
            if self.source.get(self.offset..self.offset + 2) == Some(b"[]") {
                self.offset += 2;
                array_depth += 1;
            } else if self.source.get(self.offset..self.offset + 3) == Some(b"...") {
                self.offset += 3;
                array_depth += 1;
            } else {
                break;
            }
        }
        Some(TypeRef {
            name,
            arguments,
            array_depth,
        })
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.offset).copied()
    }
}

fn parse_method_signature(signature: &str) -> Option<(TypeRef, Vec<TypeRef>)> {
    let open = signature.find('(')?;
    let close = signature.rfind(')')?;
    let return_type = signature[..open].trim();
    let response = parse_java_type(return_type)?;
    let parameters = split_top_level(&signature[open + 1..close], ',')
        .into_iter()
        .filter_map(|parameter| {
            let value = parameter.trim();
            let boundary = value.rfind(char::is_whitespace)?;
            parse_java_type(value[..boundary].trim())
        })
        .collect();
    Some((response, parameters))
}

fn unwrap_result(mut value: TypeRef) -> TypeRef {
    while RESULT_WRAPPERS.contains(&value.simple_name()) && value.arguments.len() == 1 {
        value = value.arguments.remove(0);
    }
    value
}

fn declared_field_type(signature: &str, name: &str) -> Option<String> {
    let signature = signature.trim().trim_end_matches(';').trim();
    let position = signature.rfind(name)?;
    let suffix = signature[position + name.len()..].trim();
    if !suffix.is_empty() && !suffix.starts_with('=') {
        return None;
    }
    Some(signature[..position].trim().to_owned())
}

fn split_top_level(value: &str, separator: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            current if current == separator && depth == 0 => {
                result.push(&value[start..index]);
                start = index + current.len_utf8();
            }
            _ => {}
        }
    }
    if start < value.len() {
        result.push(&value[start..]);
    }
    result
}

fn java_package(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        line.trim()
            .strip_prefix("package ")
            .and_then(|value| value.strip_suffix(';'))
            .map(str::trim)
            .map(str::to_owned)
    })
}

fn java_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("import ")
                .and_then(|value| value.strip_suffix(';'))
                .map(str::trim)
                .filter(|value| !value.ends_with(".*") && !value.starts_with("static "))
                .map(str::to_owned)
        })
        .collect()
}

fn descendants(root: Node<'_>) -> Vec<Node<'_>> {
    let mut result = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        result.push(node);
        let mut cursor = node.walk();
        let mut children = node.children(&mut cursor).collect::<Vec<_>>();
        children.reverse();
        stack.extend(children);
    }
    result
}

fn text_of(source: &str, node: Node<'_>) -> String {
    source[node.byte_range()].to_owned()
}

fn normalize_fqn(value: &str) -> String {
    value.replace("::", ".")
}

fn schema_name(fqn: &str) -> String {
    fqn.rsplit('.').next().unwrap_or(fqn).to_owned()
}

fn is_type_wrapper(name: &str) -> bool {
    RESULT_WRAPPERS.contains(&name)
        || matches!(
            name,
            "PageList"
                | "Page"
                | "PageResult"
                | "List"
                | "Set"
                | "Collection"
                | "ArrayList"
                | "LinkedList"
                | "HashSet"
                | "Iterable"
                | "Map"
                | "HashMap"
                | "LinkedHashMap"
                | "TreeMap"
                | "Optional"
        )
}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                vec![byte as char]
            } else {
                format!("%{byte:02X}").chars().collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_type_parser_preserves_nested_generics_and_arrays() {
        let parsed = parse_java_type("ApiResult<PageList<DetailVO[]>>").unwrap();
        assert_eq!(parsed.simple_name(), "ApiResult");
        assert_eq!(parsed.arguments[0].simple_name(), "PageList");
        assert_eq!(parsed.arguments[0].arguments[0].name, "DetailVO");
        assert_eq!(parsed.arguments[0].arguments[0].array_depth, 1);
        assert_eq!(parsed.render_java(), "ApiResult<PageList<DetailVO[]>>");
    }

    #[test]
    fn signature_parser_ignores_parameter_names() {
        let (response, parameters) = parse_method_signature(
            "ApiResult<PageList<DetailVO>> (EmployeeUser employee, QueryReq req)",
        )
        .unwrap();
        assert_eq!(response.simple_name(), "ApiResult");
        assert_eq!(parameters[0].simple_name(), "EmployeeUser");
        assert_eq!(parameters[1].simple_name(), "QueryReq");
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(
            encode_path_segment("a b/中文"),
            "a%20b%2F%E4%B8%AD%E6%96%87"
        );
    }
}
