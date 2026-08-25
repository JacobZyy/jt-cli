use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

const MAX_REACHABLE_NODES: usize = 25_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: usize,
    pub start_column: usize,
    pub docstring: Option<String>,
    pub signature: String,
    pub decorators: String,
    pub return_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
    pub line: usize,
    pub column: usize,
    pub metadata: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedRef {
    pub from_node_id: String,
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub column: usize,
    pub file_path: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reachability {
    pub nodes: HashSet<String>,
    pub parent: HashMap<String, String>,
}

pub struct Snapshot {
    pub nodes: HashMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
    outgoing: HashMap<String, Vec<usize>>,
    incoming_calls: HashMap<String, Vec<usize>>,
    unresolved_by_method: HashMap<String, Vec<UnresolvedRef>>,
    nodes_by_simple_name: HashMap<String, Vec<String>>,
    pub version: String,
    pub extraction_version: String,
}

impl Snapshot {
    pub fn load(repo: &Path) -> Result<Self> {
        let database = repo.join(".codegraph/codegraph.db");
        if !database.is_file() {
            bail!(
                "CodeGraph index missing: {}. Run `codegraph init {}` first",
                database.display(),
                repo.display()
            );
        }
        let connection = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open CodeGraph database {}", database.display()))?;
        validate_schema(&connection)?;

        let index_state = metadata(&connection, "index_state")?;
        if index_state != "complete" {
            bail!("CodeGraph index not complete: {index_state}");
        }
        let version = metadata(&connection, "indexed_with_version")?;
        let extraction_version = metadata(&connection, "indexed_with_extraction_version")?;

        let mut node_statement = connection.prepare(
            "SELECT id, kind, name, qualified_name, file_path, start_line, start_column, \
                    docstring, COALESCE(signature, ''), COALESCE(decorators, ''), \
                    COALESCE(return_type, '') FROM nodes WHERE language = 'java' OR kind = 'file'",
        )?;
        let node_rows = node_statement.query_map([], |row| {
            Ok(GraphNode {
                id: row.get(0)?,
                kind: row.get(1)?,
                name: row.get(2)?,
                qualified_name: row.get(3)?,
                file_path: row.get(4)?,
                start_line: row.get::<_, i64>(5)? as usize,
                start_column: row.get::<_, i64>(6)? as usize,
                docstring: row.get(7)?,
                signature: row.get(8)?,
                decorators: row.get(9)?,
                return_type: row.get(10)?,
            })
        })?;
        let mut nodes = HashMap::new();
        let mut nodes_by_simple_name: HashMap<String, Vec<String>> = HashMap::new();
        for row in node_rows {
            let node = row?;
            nodes_by_simple_name
                .entry(node.name.clone())
                .or_default()
                .push(node.id.clone());
            nodes.insert(node.id.clone(), node);
        }
        for ids in nodes_by_simple_name.values_mut() {
            ids.sort_by(|left, right| {
                nodes[left]
                    .qualified_name
                    .cmp(&nodes[right].qualified_name)
                    .then_with(|| nodes[left].file_path.cmp(&nodes[right].file_path))
            });
        }

        let mut edge_statement = connection.prepare(
            "SELECT source, target, kind, COALESCE(line, 0), COALESCE(col, 0), \
                    COALESCE(metadata, ''), COALESCE(provenance, '') \
             FROM edges WHERE kind IN ('calls', 'contains', 'references')",
        )?;
        let edge_rows = edge_statement.query_map([], |row| {
            Ok(GraphEdge {
                source: row.get(0)?,
                target: row.get(1)?,
                kind: row.get(2)?,
                line: row.get::<_, i64>(3)? as usize,
                column: row.get::<_, i64>(4)? as usize,
                metadata: row.get(5)?,
                provenance: row.get(6)?,
            })
        })?;
        let mut edges = Vec::new();
        for row in edge_rows {
            let edge = row?;
            if nodes.contains_key(&edge.source) && nodes.contains_key(&edge.target) {
                edges.push(edge);
            }
        }
        edges.sort_by(|left, right| {
            (
                &left.source,
                &left.kind,
                &left.target,
                left.line,
                left.column,
            )
                .cmp(&(
                    &right.source,
                    &right.kind,
                    &right.target,
                    right.line,
                    right.column,
                ))
        });

        let mut outgoing: HashMap<String, Vec<usize>> = HashMap::new();
        let mut incoming_calls: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, edge) in edges.iter().enumerate() {
            outgoing.entry(edge.source.clone()).or_default().push(index);
            if edge.kind == "calls" {
                incoming_calls
                    .entry(edge.target.clone())
                    .or_default()
                    .push(index);
            }
        }

        let mut unresolved_statement = connection.prepare(
            "SELECT from_node_id, reference_name, reference_kind, line, col, file_path, status \
             FROM unresolved_refs WHERE language = 'java'",
        )?;
        let unresolved_rows = unresolved_statement.query_map([], |row| {
            Ok(UnresolvedRef {
                from_node_id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                line: row.get::<_, i64>(3)? as usize,
                column: row.get::<_, i64>(4)? as usize,
                file_path: row.get(5)?,
                status: row.get(6)?,
            })
        })?;
        let mut unresolved_by_method: HashMap<String, Vec<UnresolvedRef>> = HashMap::new();
        for row in unresolved_rows {
            let item = row?;
            unresolved_by_method
                .entry(item.from_node_id.clone())
                .or_default()
                .push(item);
        }

        Ok(Self {
            nodes,
            edges,
            outgoing,
            incoming_calls,
            unresolved_by_method,
            nodes_by_simple_name,
            version,
            extraction_version,
        })
    }

    pub fn contained(&self, parent_id: &str, kind: &str) -> Vec<&GraphNode> {
        let mut result = self
            .outgoing(parent_id)
            .filter(|edge| edge.kind == "contains")
            .filter_map(|edge| self.nodes.get(&edge.target))
            .filter(|node| node.kind == kind)
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            (left.start_line, left.start_column, &left.qualified_name).cmp(&(
                right.start_line,
                right.start_column,
                &right.qualified_name,
            ))
        });
        result
    }

    pub fn outgoing(&self, node_id: &str) -> impl Iterator<Item = &GraphEdge> {
        self.outgoing
            .get(node_id)
            .into_iter()
            .flatten()
            .map(|index| &self.edges[*index])
    }

    pub fn incoming_calls(&self, node_id: &str) -> impl Iterator<Item = &GraphEdge> {
        self.incoming_calls
            .get(node_id)
            .into_iter()
            .flatten()
            .map(|index| &self.edges[*index])
    }

    pub fn unresolved(&self, node_id: &str) -> &[UnresolvedRef] {
        self.unresolved_by_method
            .get(node_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn candidates(&self, simple_name: &str) -> Vec<&GraphNode> {
        self.nodes_by_simple_name
            .get(simple_name)
            .into_iter()
            .flatten()
            .filter_map(|id| self.nodes.get(id))
            .collect()
    }

    pub fn reachable_calls(&self, root_id: &str) -> Result<Reachability> {
        let mut nodes = HashSet::from([root_id.to_owned()]);
        let mut parent = HashMap::new();
        let mut queue = VecDeque::from([root_id.to_owned()]);
        while let Some(current) = queue.pop_front() {
            for edge in self.outgoing(&current).filter(|edge| edge.kind == "calls") {
                if nodes.insert(edge.target.clone()) {
                    parent.insert(edge.target.clone(), current.clone());
                    queue.push_back(edge.target.clone());
                    if nodes.len() > MAX_REACHABLE_NODES {
                        bail!("operation call graph exceeded {MAX_REACHABLE_NODES} nodes");
                    }
                }
            }
        }
        Ok(Reachability { nodes, parent })
    }
}

#[cfg(test)]
pub(crate) fn test_snapshot(nodes: Vec<GraphNode>, mut edges: Vec<GraphEdge>) -> Snapshot {
    let nodes = nodes
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    edges.sort_by(|left, right| {
        (
            &left.source,
            &left.kind,
            &left.target,
            left.line,
            left.column,
        )
            .cmp(&(
                &right.source,
                &right.kind,
                &right.target,
                right.line,
                right.column,
            ))
    });
    let mut outgoing: HashMap<String, Vec<usize>> = HashMap::new();
    let mut incoming_calls: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, edge) in edges.iter().enumerate() {
        outgoing.entry(edge.source.clone()).or_default().push(index);
        if edge.kind == "calls" {
            incoming_calls
                .entry(edge.target.clone())
                .or_default()
                .push(index);
        }
    }
    let mut nodes_by_simple_name: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes.values() {
        nodes_by_simple_name
            .entry(node.name.clone())
            .or_default()
            .push(node.id.clone());
    }
    Snapshot {
        nodes,
        edges,
        outgoing,
        incoming_calls,
        unresolved_by_method: HashMap::new(),
        nodes_by_simple_name,
        version: "test".to_owned(),
        extraction_version: "test".to_owned(),
    }
}

fn metadata(connection: &Connection, key: &str) -> Result<String> {
    connection
        .query_row(
            "SELECT value FROM project_metadata WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .with_context(|| format!("CodeGraph metadata missing: {key}"))
}

fn validate_schema(connection: &Connection) -> Result<()> {
    for (table, columns) in [
        (
            "nodes",
            &[
                "id",
                "kind",
                "name",
                "qualified_name",
                "file_path",
                "start_line",
                "start_column",
                "signature",
            ][..],
        ),
        ("edges", &["source", "target", "kind", "line", "col"][..]),
        (
            "unresolved_refs",
            &[
                "from_node_id",
                "reference_name",
                "reference_kind",
                "file_path",
                "status",
            ][..],
        ),
    ] {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let found = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<HashSet<_>, _>>()?;
        let missing = columns
            .iter()
            .filter(|column| !found.contains(**column))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "unsupported CodeGraph schema: {table} missing {}",
                missing.join(", ")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachable_calls_are_bounded_and_deterministic() {
        let nodes = ["root", "a", "b"]
            .into_iter()
            .map(|id| {
                (
                    id.to_owned(),
                    GraphNode {
                        id: id.to_owned(),
                        kind: "method".to_owned(),
                        name: id.to_owned(),
                        qualified_name: id.to_owned(),
                        file_path: format!("{id}.java"),
                        start_line: 1,
                        start_column: 0,
                        docstring: None,
                        signature: String::new(),
                        decorators: String::new(),
                        return_type: String::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let edges = vec![
            GraphEdge {
                source: "root".to_owned(),
                target: "a".to_owned(),
                kind: "calls".to_owned(),
                line: 1,
                column: 0,
                metadata: String::new(),
                provenance: String::new(),
            },
            GraphEdge {
                source: "a".to_owned(),
                target: "b".to_owned(),
                kind: "calls".to_owned(),
                line: 1,
                column: 0,
                metadata: String::new(),
                provenance: String::new(),
            },
            GraphEdge {
                source: "b".to_owned(),
                target: "root".to_owned(),
                kind: "calls".to_owned(),
                line: 1,
                column: 0,
                metadata: String::new(),
                provenance: String::new(),
            },
        ];
        let mut outgoing: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, edge) in edges.iter().enumerate() {
            outgoing.entry(edge.source.clone()).or_default().push(index);
        }
        let graph = Snapshot {
            nodes,
            edges,
            outgoing,
            incoming_calls: HashMap::new(),
            unresolved_by_method: HashMap::new(),
            nodes_by_simple_name: HashMap::new(),
            version: "test".to_owned(),
            extraction_version: "test".to_owned(),
        };

        let reachable = graph.reachable_calls("root").unwrap();

        assert_eq!(
            reachable.nodes,
            HashSet::from(["root".into(), "a".into(), "b".into()])
        );
        assert_eq!(reachable.parent["a"], "root");
        assert_eq!(reachable.parent["b"], "a");
    }
}
