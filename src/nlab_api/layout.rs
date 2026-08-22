use std::collections::BTreeSet;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};

use super::model::Operation;
use super::naming::{lower_camel, without_interface_prefix};

pub fn api_output_path(operation: &Operation, contract_roots: &[String]) -> Result<String> {
    let directory = facade_directory(operation, contract_roots)?;
    let file = format!(
        "{}.ts",
        lower_camel(without_interface_prefix(&operation.facade_name))
    );
    Ok(join_optional(&directory, &file))
}

pub fn type_output_path(operation: &Operation, contract_roots: &[String]) -> Result<String> {
    Ok(api_output_path(operation, contract_roots)?
        .trim_end_matches(".ts")
        .to_owned())
}

pub fn facade_directory(operation: &Operation, contract_roots: &[String]) -> Result<String> {
    let source = Path::new(&operation.contract_source);
    let contract_root = contract_roots
        .iter()
        .filter(|root| source.starts_with(Path::new(root)))
        .max_by_key(|root| root.len())
        .with_context(|| {
            format!(
                "Facade declaration {} is outside configured contract roots: {}",
                operation.contract_source,
                contract_roots.join(", ")
            )
        })?;
    let relative = source.strip_prefix(contract_root).with_context(|| {
        format!(
            "Facade declaration {} is outside configured contract root {contract_root}",
            operation.contract_source
        )
    })?;
    let parent = relative
        .parent()
        .context("Facade declaration has no parent directory")?;
    normal_path(parent)
}

pub fn nearest_usage_directory(directories: impl IntoIterator<Item = String>) -> String {
    let directories = directories.into_iter().collect::<BTreeSet<_>>();
    if directories.len() == 1 {
        return directories.into_iter().next().unwrap_or_default();
    }
    let parts = directories
        .iter()
        .map(|directory| {
            directory
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let common_length = parts
        .first()
        .map(|first| {
            first
                .iter()
                .enumerate()
                .take_while(|(index, part)| {
                    parts
                        .iter()
                        .skip(1)
                        .all(|candidate| candidate.get(*index) == Some(part))
                })
                .count()
        })
        .unwrap_or(0);
    let common = parts
        .first()
        .map(|parts| parts[..common_length].join("/"))
        .unwrap_or_default();
    join_optional(&common, "share")
}

pub fn join_path(left: &str, right: &str) -> String {
    match (left.trim_matches('/'), right.trim_matches('/')) {
        ("", right) => right.to_owned(),
        (left, "") => left.to_owned(),
        (left, right) => format!("{left}/{right}"),
    }
}

fn join_optional(left: &str, right: &str) -> String {
    join_path(left, right)
}

fn normal_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("unsafe Facade declaration path: {}", path.display()),
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nlab_api::model::{HttpRoute, RouteSource, RouteStatus, TypeRef};

    fn operation(source: &str) -> Operation {
        Operation {
            key: "IGoodsQueryFacade#query".to_owned(),
            facade_name: "IGoodsQueryFacade".to_owned(),
            facade_fqn: "p.IGoodsQueryFacade".to_owned(),
            method_name: "query".to_owned(),
            signature: String::new(),
            description: None,
            contract_source: source.to_owned(),
            request: None,
            response: TypeRef {
                name: "void".to_owned(),
                arguments: vec![],
                array_depth: 0,
            },
            request_schema: None,
            response_schema: None,
            service: None,
            route: HttpRoute {
                status: RouteStatus::Placeholder,
                source: RouteSource::Placeholder,
                method: "POST".to_owned(),
                path: "/query".to_owned(),
                host: None,
            },
            semantic_patches: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn facade_layout_strips_stable_contract_root() {
        let operation = operation(
            "contract/src/main/java/com/zhuanzhuan/nlabstore/contract/checkapp/IGoodsQueryFacade.java",
        );
        let root = "contract/src/main/java/com/zhuanzhuan/nlabstore/contract";
        assert_eq!(
            api_output_path(&operation, &[root.to_owned()]).unwrap(),
            "checkapp/goodsQueryFacade.ts"
        );
        assert_eq!(
            type_output_path(&operation, &[root.to_owned()]).unwrap(),
            "checkapp/goodsQueryFacade"
        );
    }

    #[test]
    fn shared_types_use_nearest_common_directory() {
        assert_eq!(
            nearest_usage_directory([
                "checkapp/goodsQueryFacade".to_owned(),
                "checkapp/recycleQueryFacade".to_owned(),
            ]),
            "checkapp/share"
        );
        assert_eq!(
            nearest_usage_directory([
                "checkapp/goodsQueryFacade".to_owned(),
                "operations/refurbManageFacade".to_owned(),
            ]),
            "share"
        );
    }
}
