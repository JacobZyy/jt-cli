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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_arguments: Vec<RequestArgument>,
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
pub struct RequestArgument {
    pub index: usize,
    pub java_name: String,
    pub name: Option<String>,
    pub java_type: TypeRef,
    pub location: InputLocation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputLocation {
    Query,
    Body,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnumCandidateStatus {
    Verified,
    Conflict,
    Unverified,
    Ignored,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnumCandidateVerification {
    pub status: EnumCandidateStatus,
    pub reason: String,
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
    /// Confirmed enum declaration members without proof that the field domain is closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_values: Vec<CodedValue>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enum_associated: bool,
    /// The field is proven to use the enum's serialized primary value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub primary_enum_value: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_candidate: Option<EnumCandidateVerification>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub nullable: bool,
    pub evidence: Vec<String>,
    pub warning: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl SemanticPatch {
    pub fn has_enum_null_branch(&self) -> bool {
        self.nullable && (self.enum_fqn.is_some() || self.associated_values().is_some())
    }

    pub fn associated_values(&self) -> Option<&[CodedValue]> {
        if !self.primary_enum_value && !self.enum_associated {
            return None;
        }
        if self.status == ProvenanceStatus::Closed && !self.values.is_empty() {
            Some(&self.values)
        } else if self.enum_associated && !self.known_values.is_empty() {
            Some(&self.known_values)
        } else {
            None
        }
    }
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
    pub fn roots(self, operation: &Operation) -> Vec<&TypeRef> {
        match self {
            Self::Request if !operation.request_arguments.is_empty() => operation
                .request_arguments
                .iter()
                .map(|argument| &argument.java_type)
                .collect(),
            Self::Request => operation.request.iter().collect(),
            Self::Response => vec![&operation.response],
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
    pub associated_enum_patches: usize,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(
        status: ProvenanceStatus,
        values: Vec<CodedValue>,
        known_values: Vec<CodedValue>,
    ) -> SemanticPatch {
        SemanticPatch {
            target: FieldTarget {
                source: FieldSource::Response,
                operation_key: "Facade#method".to_owned(),
                schema_fqn: "p.Schema".to_owned(),
                field_path: "status".to_owned(),
                field_name: "status".to_owned(),
            },
            status,
            enum_fqn: None,
            enum_source: None,
            accessor: None,
            values,
            known_values,
            enum_associated: false,
            primary_enum_value: false,
            enum_candidate: None,
            nullable: false,
            evidence: Vec::new(),
            warning: None,
        }
    }

    #[test]
    fn associated_values_preserves_closed_and_known_meanings() {
        let value = CodedValue {
            value: WireValue::String("ready".to_owned()),
            key: Some("READY".to_owned()),
            label: "Ready".to_owned(),
        };
        let mut closed = patch(ProvenanceStatus::Closed, vec![value.clone()], vec![]);
        closed.primary_enum_value = true;
        assert_eq!(closed.associated_values(), Some([value.clone()].as_slice()));

        let mut known = patch(ProvenanceStatus::Known, vec![], vec![value.clone()]);
        assert_eq!(known.associated_values(), None);
        known.enum_associated = true;
        known.primary_enum_value = true;
        assert_eq!(known.associated_values(), Some([value].as_slice()));
        assert!(closed.known_values.is_empty());
    }

    #[test]
    fn semantic_patch_new_fields_are_optional_in_old_json() {
        let json = serde_json::json!({
            "target": {
                "operationKey": "Facade#method",
                "schemaFqn": "p.Schema",
                "fieldPath": "status",
                "fieldName": "status"
            },
            "status": "known",
            "enumFqn": null,
            "enumSource": null,
            "accessor": null,
            "values": [],
            "evidence": [],
            "warning": null
        });
        let patch = serde_json::from_value::<SemanticPatch>(json).unwrap();
        assert!(!patch.enum_associated);
        assert!(!patch.primary_enum_value);
        assert!(!patch.nullable);
        assert!(
            !serde_json::to_value(patch)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("enumAssociated")
        );
    }
}
