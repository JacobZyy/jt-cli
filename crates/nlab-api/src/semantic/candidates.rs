use super::*;
use crate::model::{EnumCandidateStatus, EnumCandidateVerification, Field};

impl SemanticAnalyzer<'_> {
    pub(super) fn verify_enum_candidate(
        &self,
        schema: &Schema,
        field: &Field,
        patch: &SemanticPatch,
    ) -> Option<EnumCandidateVerification> {
        if patch
            .enum_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.status == EnumCandidateStatus::Ignored)
        {
            return patch.enum_candidate.clone();
        }
        let description = field.description.as_deref().unwrap_or_default();
        let references = see_enum_references(description);
        if field.declared_values.is_none() && field.linked_enum.is_none() && references.is_empty() {
            return None;
        }
        let decision = |status, reason: &str| {
            Some(EnumCandidateVerification {
                status,
                reason: reason.to_owned(),
            })
        };
        let Some(values) = patch.associated_values() else {
            return decision(
                EnumCandidateStatus::Unverified,
                "documentation has no verified primary enum association in this operation; the original scalar type is retained",
            );
        };
        let documented =
            linked_enum_nodes(self.project, &schema.source_path, &schema.fqn, description);
        if !references.is_empty() && documented.is_empty() {
            return decision(
                EnumCandidateStatus::Unverified,
                "documented enum reference cannot be resolved; code-backed association is retained independently",
            );
        }
        if documented.iter().any(|node| {
            Some(node.qualified_name.replace("::", ".")).as_ref() != patch.enum_fqn.as_ref()
        }) || field.linked_enum.as_ref().is_some_and(|linked| {
            Some(&linked.enum_fqn) != patch.enum_fqn.as_ref()
                || Some(&linked.accessor) != patch.accessor.as_ref()
        }) {
            return decision(
                EnumCandidateStatus::Conflict,
                "documented enum identity differs from the actual code-backed primary enum; code evidence takes precedence",
            );
        }
        if let Some(declared) = &field.declared_values
            && !same_values(&declared.values, values)
        {
            return decision(
                EnumCandidateStatus::Conflict,
                "documented values differ from the complete source enum declaration; the generated enum uses source members, without claiming every member occurs in this response",
            );
        }
        decision(
            EnumCandidateStatus::Verified,
            "documented primary enum agrees with the independently verified code association",
        )
    }
}

fn same_values(left: &[CodedValue], right: &[CodedValue]) -> bool {
    let keys = |values: &[CodedValue]| {
        values
            .iter()
            .map(|item| serde_json::to_string(&item.value).expect("serializable enum value"))
            .collect::<BTreeSet<_>>()
    };
    keys(left) == keys(right)
}
