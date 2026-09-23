use super::*;
use crate::model::{EnumCandidateStatus, EnumCandidateVerification, Field};

pub(super) fn associate_comment_values(
    field: &Field,
    source_path: &str,
    domains: &[Domain],
    patch: &mut SemanticPatch,
) {
    let Some(declared) = &field.declared_values else {
        return;
    };
    if patch.enum_fqn.is_some()
        || field.linked_enum.is_some()
        || field
            .description
            .as_deref()
            .is_some_and(|description| !see_enum_references(description).is_empty())
    {
        return;
    }
    let mut observed = BTreeSet::new();
    for domain in domains {
        if !domain.unknown.is_empty()
            || !domain.external.is_empty()
            || !domain.closure_gaps.is_empty()
            || domain.transformed
            || domain.enum_fqn.is_some()
        {
            return;
        }
        for literal in &domain.literals {
            if literal == "null" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<WireValue>(literal) else {
                return;
            };
            observed.insert(serde_json::to_string(&value).expect("serializable literal"));
        }
    }
    if observed.is_empty() {
        return;
    }
    let documented = declared
        .values
        .iter()
        .map(|item| serde_json::to_string(&item.value).expect("serializable enum value"))
        .collect::<BTreeSet<_>>();
    if documented.len() != declared.values.len() {
        return;
    }
    if !observed.is_subset(&documented) {
        patch.enum_candidate = Some(EnumCandidateVerification {
            status: EnumCandidateStatus::Conflict,
            reason: "operation writes a value missing from the documented field values".to_owned(),
        });
        return;
    }
    patch.status = ProvenanceStatus::Closed;
    patch.enum_associated = true;
    patch.enum_source = Some(source_path.to_owned());
    patch.values = declared.values.clone();
    patch.warning = None;
    patch.enum_candidate = Some(EnumCandidateVerification {
        status: EnumCandidateStatus::Verified,
        reason: "all resolved writes to this field are covered by the documented values".to_owned(),
    });
}

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
                "documentation has no verified value association in this operation; the original scalar type is retained",
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
            && !contains_all_values(&declared.values, values)
        {
            return decision(
                EnumCandidateStatus::Conflict,
                "documented values omit a source enum member; the generated enum uses the complete source declaration",
            );
        }
        decision(
            EnumCandidateStatus::Verified,
            "documented primary enum agrees with the independently verified code association",
        )
    }
}

fn contains_all_values(left: &[CodedValue], right: &[CodedValue]) -> bool {
    let keys = |values: &[CodedValue]| {
        values
            .iter()
            .map(|item| serde_json::to_string(&item.value).expect("serializable enum value"))
            .collect::<BTreeSet<_>>()
    };
    keys(right).is_subset(&keys(left))
}
