use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetIdentity {
    pub app_name: String,
    pub branch: String,
    pub commit: String,
    pub codegraph_version: String,
    pub codegraph_extraction_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractIr {
    pub target: TargetIdentity,
    pub operations: Vec<Operation>,
    pub schemas: BTreeMap<String, Schema>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub key: String,
    pub facade_name: String,
    pub facade_fqn: String,
    pub method_name: String,
    pub signature: String,
    pub description: Option<String>,
    pub contract_source: String,
    pub request: Option<TypeRef>,
    pub response: TypeRef,
    pub request_schema: Option<String>,
    pub response_schema: Option<String>,
    pub service: Option<ServiceOwner>,
    pub route: HttpRoute,
    pub semantic_patches: Vec<SemanticPatch>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceOwner {
    pub class_name: String,
    pub class_fqn: String,
    pub method_name: String,
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRoute {
    pub status: RouteStatus,
    pub source: RouteSource,
    pub method: String,
    pub path: String,
    pub host: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteStatus {
    Placeholder,
    Resolved,
    Cached,
    QueryFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteSource {
    Placeholder,
    Zgateway,
    Cache,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub fqn: String,
    pub name: String,
    pub source_path: String,
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_parameters: Vec<String>,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub java_type: TypeRef,
    pub optional: bool,
    pub description: Option<String>,
    pub declared_values: Option<CodedValues>,
    #[serde(default)]
    pub linked_enum: Option<LinkedEnum>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedEnum {
    pub enum_fqn: String,
    pub enum_source: String,
    pub accessor: String,
    pub values: Vec<CodedValue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeRef {
    pub name: String,
    pub arguments: Vec<TypeRef>,
    pub array_depth: usize,
}

impl TypeRef {
    pub fn simple_name(&self) -> &str {
        self.name
            .rsplit(['.', ':'])
            .find(|part| !part.is_empty())
            .unwrap_or(&self.name)
    }

    pub fn render_java(&self) -> String {
        let mut rendered = self.name.replace("::", ".");
        if !self.arguments.is_empty() {
            rendered.push('<');
            rendered.push_str(
                &self
                    .arguments
                    .iter()
                    .map(Self::render_java)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            rendered.push('>');
        }
        rendered.push_str(&"[]".repeat(self.array_depth));
        rendered
    }

    pub fn substitute(&self, bindings: &BTreeMap<String, TypeRef>) -> Self {
        if self.arguments.is_empty() {
            if let Some(binding) = bindings.get(&self.name) {
                let mut resolved = binding.clone();
                resolved.array_depth += self.array_depth;
                return resolved;
            }
        }
        Self {
            name: self.name.clone(),
            arguments: self
                .arguments
                .iter()
                .map(|argument| argument.substitute(bindings))
                .collect(),
            array_depth: self.array_depth,
        }
    }
}

impl Schema {
    pub fn bindings_for(&self, type_ref: &TypeRef) -> BTreeMap<String, TypeRef> {
        if self.type_parameters.len() != type_ref.arguments.len() {
            return BTreeMap::new();
        }
        self.type_parameters
            .iter()
            .cloned()
            .zip(type_ref.arguments.iter().cloned())
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodedValues {
    pub name: String,
    pub source: CodedValueSource,
    pub values: Vec<CodedValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodedValueSource {
    Comment,
    Annotation,
    ConstantReference,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodedValue {
    pub value: WireValue,
    pub key: Option<String>,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WireValue {
    String(String),
    Number(i64),
    Decimal(serde_json::Number),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProvenanceStatus {
    Closed,
    Known,
    External,
    Unresolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticPatch {
    pub target: FieldTarget,
    pub status: ProvenanceStatus,
    pub enum_fqn: Option<String>,
    pub enum_source: Option<String>,
    pub accessor: Option<String>,
    pub values: Vec<CodedValue>,
    pub evidence: Vec<String>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldTarget {
    #[serde(default)]
    pub source: FieldSource,
    pub operation_key: String,
    pub schema_fqn: String,
    pub field_path: String,
    pub field_name: String,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FieldSource {
    Request,
    #[default]
    Response,
}

impl FieldSource {
    pub fn root(self, operation: &Operation) -> Option<&TypeRef> {
        match self {
            Self::Request => operation.request.as_ref(),
            Self::Response => Some(&operation.response),
        }
    }

    pub fn operation_key(self, operation: &Operation) -> String {
        match self {
            Self::Request => format!("{}:request", operation.key),
            Self::Response => operation.key.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendManifest {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default)]
    pub app_name: String,
    pub branch: String,
    pub commit: String,
    pub openapi_sha256: String,
    pub api_files: Vec<String>,
    pub type_files: Vec<String>,
    #[serde(default)]
    pub enum_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateResult {
    pub status: &'static str,
    pub repo_path: String,
    pub branch: String,
    pub commit: String,
    pub output_dir: String,
    pub openapi: String,
    pub openapi_sha256: String,
    pub contracts: usize,
    pub paths: usize,
    pub schemas: usize,
    pub routes_replaced: usize,
    pub placeholders: usize,
    pub semantic_patches: usize,
    pub closed_enum_patches: usize,
    pub api_files: usize,
    pub type_files: usize,
    pub enum_files: usize,
    pub migration_changed_source_files: usize,
    pub migration_unresolved: usize,
    pub mock_generated: bool,
    pub whistle_rules_updated: bool,
    pub warnings: usize,
    pub report: String,
    pub duration_ms: u128,
}
