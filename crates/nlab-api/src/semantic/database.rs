use super::*;

use std::fs;

use regex::Regex;

const MAX_MAPPER_BYTES: u64 = 1_000_000;
// ponytail: cap same-field writers; widen only with an indexed write-site query.
const MAX_FIELD_WRITES: usize = 64;

#[derive(Clone)]
pub(super) struct MapperInfo {
    namespace: String,
    table: String,
    columns: BTreeMap<String, String>,
    writes: BTreeMap<String, BTreeSet<String>>,
}

impl SemanticAnalyzer<'_> {
    pub(super) fn database_field_domain(
        &mut self,
        class: &GraphNode,
        field_name: &str,
    ) -> Result<Option<Domain>> {
        let Some(mapper) = self.mybatis_mapper(class)? else {
            return Ok(None);
        };
        let Some(column) = mapper.columns.get(field_name) else {
            return Ok(None);
        };
        let Some(write_methods) = mapper.writes.get(field_name) else {
            return Ok(None);
        };
        let setter_name = format!("set{}", uppercase_first(field_name));
        let setters = self
            .project
            .graph()
            .contained(&class.id, "method")
            .into_iter()
            .filter(|method| method.name == setter_name)
            .collect::<Vec<_>>();
        let [setter] = setters.as_slice() else {
            return Ok(None);
        };
        let edges = self
            .project
            .graph()
            .incoming_calls(&setter.id)
            .cloned()
            .collect::<Vec<_>>();
        if edges.is_empty() || edges.len() > MAX_FIELD_WRITES {
            return Ok(None);
        }

        let mut domain = Domain::default();
        let mut matched = 0;
        for edge in edges {
            let writer = self.project.graph().nodes[&edge.source].clone();
            let Some(site) = self
                .method_invocations(&writer)?
                .into_iter()
                .filter(|site| site.name == setter_name && site.line == edge.line)
                .min_by_key(|site| site.column.abs_diff(edge.column))
            else {
                domain
                    .unknown
                    .insert("database setter call not indexed".to_owned());
                continue;
            };
            let Some(receiver) = site.receiver.as_deref() else {
                domain
                    .unknown
                    .insert("database setter receiver missing".to_owned());
                continue;
            };
            if !self.receiver_is_class(&writer, receiver, site.offset, &class.id)? {
                continue;
            }
            matched += 1;
            if !self.persisted_by_mapper(&writer, &site, &mapper.namespace, write_methods)? {
                domain.unknown.insert(format!(
                    "database setter persistence unproven:{}:{}",
                    writer.file_path, edge.line
                ));
                continue;
            }
            let Some((expression, source, offset)) = self.setter_argument(&edge, &setter_name)?
            else {
                domain
                    .unknown
                    .insert("database setter value unresolved".to_owned());
                continue;
            };
            let value = match expression {
                Expression::Getter { receiver, accessor } => {
                    if let Some(enum_node) = self.enum_for_receiver(&writer, &receiver, offset)? {
                        self.enum_domain(&enum_node, &accessor)?
                    } else {
                        let mut value = Domain::default();
                        value.unknown.insert(source.clone());
                        value
                    }
                }
                Expression::Literal(value) => {
                    let mut domain = Domain::default();
                    domain.literals.insert(value);
                    domain
                }
                _ => {
                    let mut value = Domain::default();
                    value.unknown.insert(source.clone());
                    value
                }
            };
            merge_domain(&mut domain, value);
            push_unique(
                &mut domain.evidence,
                format!("database-write:{}:{}:{source}", writer.file_path, edge.line),
            );
        }
        if matched == 0 || domain.enum_fqn.is_none() {
            return Ok(None);
        }
        domain.closure_gaps.insert(
            "database values outside indexed source repositories are not proven".to_owned(),
        );
        push_unique(
            &mut domain.evidence,
            format!("database-read:{}.{}", mapper.table, column),
        );
        Ok(Some(domain))
    }

    fn mybatis_mapper(&mut self, class: &GraphNode) -> Result<Option<MapperInfo>> {
        if let Some(mapper) = self.mapper_cache.get(&class.id) {
            return Ok(mapper.clone());
        }
        let mapper = (|| {
            let (module, _) = class.file_path.split_once("/src/main/java/")?;
            let path = format!(
                "{module}/src/main/resources/mappers/{}Mapper.xml",
                class.name
            );
            let path = self.project.source_path(&path);
            if fs::metadata(&path).ok()?.len() > MAX_MAPPER_BYTES {
                return None;
            }
            let source = fs::read_to_string(path).ok()?;
            MapperInfo::parse(&source, class)
        })();
        self.mapper_cache.insert(class.id.clone(), mapper.clone());
        Ok(mapper)
    }

    fn receiver_is_class(
        &mut self,
        writer: &GraphNode,
        receiver: &str,
        offset: usize,
        class_id: &str,
    ) -> Result<bool> {
        let Some(type_name) = self.receiver_type(writer, receiver, offset)? else {
            return Ok(false);
        };
        Ok(parse_java_type(&type_name)
            .and_then(|type_ref| {
                self.project
                    .resolve_type(&writer.file_path, &writer.qualified_name, &type_ref)
            })
            .is_some_and(|class| class.id == class_id))
    }

    fn persisted_by_mapper(
        &mut self,
        writer: &GraphNode,
        setter: &InvocationSite,
        mapper_namespace: &str,
        write_methods: &BTreeSet<String>,
    ) -> Result<bool> {
        let Some(record) = setter.receiver.as_deref() else {
            return Ok(false);
        };
        if !record
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Ok(false);
        }
        for site in self.method_invocations(writer)? {
            if site.offset <= setter.offset
                || !write_methods.contains(&site.name)
                || !matches!(site.arguments.first(), Some(Expression::Identifier(name)) if name == record)
            {
                continue;
            }
            let Some(mapper_receiver) = site.receiver.as_deref() else {
                continue;
            };
            let Some(mapper_type) = self.receiver_type(writer, mapper_receiver, site.offset)?
            else {
                continue;
            };
            let Some(mapper_node) = parse_java_type(&mapper_type).and_then(|type_ref| {
                self.project
                    .resolve_type(&writer.file_path, &writer.qualified_name, &type_ref)
            }) else {
                continue;
            };
            if mapper_node.qualified_name.replace("::", ".") != mapper_namespace {
                continue;
            }
            let parsed = self.parsed(&writer.file_path)?;
            let Some(declaration) = lookup::method_declaration(parsed, writer) else {
                continue;
            };
            let reassigned = descendants(declaration).into_iter().any(|node| {
                node.kind() == "assignment_expression"
                    && setter.offset < node.start_byte()
                    && node.start_byte() < site.offset
                    && node
                        .child_by_field_name("left")
                        .is_some_and(|left| text_of(&parsed.source, left) == record)
            });
            if !reassigned {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl MapperInfo {
    // ponytail: support generator-style MyBatis XML; add an XML parser for custom mappings.
    fn parse(source: &str, class: &GraphNode) -> Option<Self> {
        let source = without_comments(source)?;
        let (_, mapper) = opening_tag(&source, "mapper")?;
        let namespace = attribute(mapper, "namespace")?.to_owned();
        if namespace.rsplit('.').next()? != format!("{}Mapper", class.name) {
            return None;
        }
        let class_name = class.qualified_name.replace("::", ".");
        let (result_map, body) = sections(&source, "resultMap")
            .into_iter()
            .find(|(tag, _)| attribute(tag, "type") == Some(class_name.as_str()))?;
        let result_map_id = attribute(result_map, "id")?;
        let mut columns = BTreeMap::new();
        for tag in inline_tags(body, "result")
            .into_iter()
            .chain(inline_tags(body, "id"))
        {
            if let (Some(property), Some(column)) =
                (attribute(tag, "property"), attribute(tag, "column"))
            {
                columns.insert(property.to_owned(), column.to_owned());
            }
        }
        let from = Regex::new(r"(?i)\bfrom\s+([A-Za-z_][A-Za-z0-9_]*)").ok()?;
        let tables = sections(&source, "select")
            .into_iter()
            .filter(|(tag, _)| attribute(tag, "resultMap") == Some(result_map_id))
            .filter_map(|(_, body)| from.captures(body).map(|match_| match_[1].to_owned()))
            .collect::<BTreeSet<_>>();
        let tables = tables.into_iter().collect::<Vec<_>>();
        let [table] = tables.as_slice() else {
            return None;
        };
        let table = table.clone();
        let write_table =
            Regex::new(r"(?i)\b(?:insert\s+into|update)\s+([A-Za-z_][A-Za-z0-9_]*)").ok()?;
        let mut writes = BTreeMap::<String, BTreeSet<String>>::new();
        for (tag, body) in sections(&source, "insert")
            .into_iter()
            .chain(sections(&source, "update"))
        {
            let Some(method) = attribute(tag, "id") else {
                continue;
            };
            if write_table
                .captures(body)
                .is_none_or(|match_| !match_[1].eq_ignore_ascii_case(&table))
            {
                continue;
            }
            for (field, column) in &columns {
                if maps_field(tag, body, field, column) {
                    writes
                        .entry(field.clone())
                        .or_default()
                        .insert(method.to_owned());
                }
            }
        }
        Some(Self {
            namespace,
            table,
            columns,
            writes,
        })
    }
}

fn opening_tag<'a>(source: &'a str, name: &str) -> Option<(usize, &'a str)> {
    let start = source.find(&format!("<{name} "))?;
    let tag = &source[start..];
    Some((start, &tag[..=tag.find('>')?]))
}

fn sections<'a>(source: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let mut sections = Vec::new();
    let mut rest = source;
    let close = format!("</{name}>");
    while let Some((start, tag)) = opening_tag(rest, name) {
        let body = &rest[start + tag.len()..];
        let Some(end) = body.find(&close) else {
            break;
        };
        sections.push((tag, &body[..end]));
        rest = &body[end + close.len()..];
    }
    sections
}

fn inline_tags<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let mut tags = Vec::new();
    let mut rest = source;
    while let Some((start, tag)) = opening_tag(rest, name) {
        tags.push(tag);
        rest = &rest[start + tag.len()..];
    }
    tags
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let value = tag.split_once(&format!(" {name}=\""))?.1;
    Some(value.split_once('"')?.0)
}

fn has_parameter(body: &str, field: &str) -> bool {
    body.split("#{")
        .skip(1)
        .filter_map(|part| part.split_once('}'))
        .map(|(parameter, _)| parameter.split(',').next().unwrap_or_default().trim())
        .any(|parameter| parameter == field || parameter.strip_prefix("record.") == Some(field))
}

fn maps_field(tag: &str, body: &str, field: &str, column: &str) -> bool {
    let column_pattern = match Regex::new(&format!(r"\b{}\b", regex::escape(column))) {
        Ok(pattern) => pattern,
        Err(_) => return false,
    };
    if tag.starts_with("<update ") {
        return Regex::new(&format!(
            r"\b{}\s*=\s*#\{{(?:record\.)?{}(?:,|\}})",
            regex::escape(column),
            regex::escape(field)
        ))
        .is_ok_and(|pattern| pattern.is_match(body));
    }
    let matching = sections(body, "if")
        .into_iter()
        .filter(|(tag, _)| {
            attribute(tag, "test").is_some_and(|test| {
                test == format!("{field} != null") || test == format!("record.{field} != null")
            })
        })
        .map(|(_, body)| body)
        .collect::<Vec<_>>();
    matching.iter().any(|body| column_pattern.is_match(body))
        && matching.iter().any(|body| has_parameter(body, field))
}

fn without_comments(source: &str) -> Option<String> {
    let mut result = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("<!--") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 4..];
        rest = &rest[rest.find("-->")? + 3..];
    }
    result.push_str(rest);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::test_snapshot;
    use crate::semantic::tests::{call, contains, node, write};

    #[test]
    fn mapped_database_writes_identify_enum_without_claiming_closed_domain() {
        let root = tempfile::tempdir().unwrap();
        let java = "service/src/main/java/p";
        write(
            root.path(),
            &format!("{java}/Record.java"),
            "package p;\nclass Record {\nInteger auditStatus;\nvoid setAuditStatus(Integer value) { auditStatus=value; }\nInteger getAuditStatus() { return auditStatus; }\n}\n",
        );
        write(
            root.path(),
            &format!("{java}/RecordMapper.java"),
            "package p;\ninterface RecordMapper {\nvoid insertSelective(Record record);\nvoid updateByExampleSelective(Record record, Object example);\n}\n",
        );
        write(
            root.path(),
            &format!("{java}/Status.java"),
            "package p;\nenum Status {\nWAIT(10), PASS(80), REJECT(90);\nfinal Integer status;\nStatus(Integer status) { this.status=status; }\nInteger getStatus() { return status; }\n}\n",
        );
        let repository = "package p;\nclass Repo {\nRecordMapper mapper;\nvoid create() {\nRecord record = new Record();\nrecord.setAuditStatus(Status.WAIT.getStatus());\nmapper.insertSelective(record);\n}\nvoid audit(Status targetStatus) {\nRecord update = new Record();\nupdate.setAuditStatus(targetStatus.getStatus());\nmapper.updateByExampleSelective(update, null);\n}\n}\n";
        write(root.path(), &format!("{java}/Repo.java"), repository);
        write(
            root.path(),
            "service/src/main/resources/mappers/RecordMapper.xml",
            "<mapper namespace=\"p.RecordMapper\">\n<resultMap id=\"Base\" type=\"p.Record\">\n<result column=\"audit_status\" property=\"auditStatus\" />\n</resultMap>\n<select id=\"selectByExample\" resultMap=\"Base\">\nselect audit_status from audit_order\n</select>\n<insert id=\"insertSelective\">\ninsert into audit_order\n<trim><if test=\"auditStatus != null\">audit_status,</if></trim>\n<trim><if test=\"auditStatus != null\">#{auditStatus},</if></trim>\n</insert>\n<update id=\"updateByExampleSelective\">\nupdate audit_order set audit_status = #{record.auditStatus}\n</update>\n</mapper>\n",
        );
        let graph = test_snapshot(
            vec![
                node(
                    "record",
                    "class",
                    "Record",
                    "p::Record",
                    &format!("{java}/Record.java"),
                    2,
                    "",
                ),
                node(
                    "field",
                    "field",
                    "auditStatus",
                    "p::Record::auditStatus",
                    &format!("{java}/Record.java"),
                    3,
                    "Integer auditStatus",
                ),
                node(
                    "setter",
                    "method",
                    "setAuditStatus",
                    "p::Record::setAuditStatus",
                    &format!("{java}/Record.java"),
                    4,
                    "void (Integer value)",
                ),
                node(
                    "mapper",
                    "interface",
                    "RecordMapper",
                    "p::RecordMapper",
                    &format!("{java}/RecordMapper.java"),
                    2,
                    "",
                ),
                node(
                    "status",
                    "enum",
                    "Status",
                    "p::Status",
                    &format!("{java}/Status.java"),
                    2,
                    "",
                ),
                node(
                    "repo",
                    "class",
                    "Repo",
                    "p::Repo",
                    &format!("{java}/Repo.java"),
                    2,
                    "",
                ),
                node(
                    "mapper-field",
                    "field",
                    "mapper",
                    "p::Repo::mapper",
                    &format!("{java}/Repo.java"),
                    3,
                    "RecordMapper mapper",
                ),
                node(
                    "create",
                    "method",
                    "create",
                    "p::Repo::create",
                    &format!("{java}/Repo.java"),
                    4,
                    "void ()",
                ),
                node(
                    "audit",
                    "method",
                    "audit",
                    "p::Repo::audit",
                    &format!("{java}/Repo.java"),
                    9,
                    "void (Status targetStatus)",
                ),
            ],
            vec![
                contains("record", "field"),
                contains("record", "setter"),
                contains("repo", "mapper-field"),
                contains("repo", "create"),
                contains("repo", "audit"),
                call("create", "setter", 6),
                call("audit", "setter", 11),
            ],
        );
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let mut analyzer = SemanticAnalyzer::new(&project);
        let domain = analyzer
            .database_field_domain(&graph.nodes["record"], "auditStatus")
            .unwrap()
            .unwrap();
        assert_eq!(domain.enum_fqn.as_deref(), Some("p.Status"));
        assert_eq!(domain.values.len(), 3);
        assert!(domain.unknown.is_empty(), "{domain:#?}");
        assert!(!domain.closure_gaps.is_empty());
        let patch = classify_patch(
            FieldTarget {
                source: FieldSource::Response,
                operation_key: "query".to_owned(),
                schema_fqn: "p.Record".to_owned(),
                field_path: "auditStatus".to_owned(),
                field_name: "auditStatus".to_owned(),
            },
            vec![domain],
        );
        assert_eq!(patch.status, ProvenanceStatus::Known);
        assert_eq!(patch.associated_values().unwrap().len(), 3);
        assert!(
            analyzer
                .database_field_domain(&graph.nodes["record"], "statusDesc")
                .unwrap()
                .is_none()
        );

        write(
            root.path(),
            &format!("{java}/Repo.java"),
            &repository.replace("mapper.updateByExampleSelective(update, null);", ""),
        );
        let project = JavaProject::load(root.path(), &graph).unwrap();
        let domain = SemanticAnalyzer::new(&project)
            .database_field_domain(&graph.nodes["record"], "auditStatus")
            .unwrap()
            .unwrap();
        assert!(!domain.unknown.is_empty(), "{domain:#?}");
    }
}
