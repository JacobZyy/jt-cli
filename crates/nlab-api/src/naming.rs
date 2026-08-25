use std::collections::{BTreeMap, BTreeSet};

pub fn shortest_unique_names(seeds: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, String> {
    shortest_unique_names_avoiding(seeds, &BTreeSet::new())
}

pub fn shortest_unique_names_avoiding(
    seeds: &BTreeMap<String, Vec<String>>,
    reserved: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    for id in seeds.keys() {
        unresolved.insert(id.clone());
    }

    let max_depth = seeds.values().map(Vec::len).max().unwrap_or(0);
    for depth in 1..=max_depth {
        let mut candidates = BTreeMap::<String, Vec<String>>::new();
        for id in &unresolved {
            let candidate = suffix_name(&seeds[id], depth);
            candidates.entry(candidate).or_default().push(id.clone());
        }
        for (candidate, ids) in candidates {
            if ids.len() == 1 {
                let id = &ids[0];
                let collides_with_resolved =
                    reserved.contains(&candidate) || names.values().any(|name| name == &candidate);
                if !collides_with_resolved {
                    names.insert(id.clone(), candidate);
                    unresolved.remove(id);
                }
            }
        }
    }

    let mut duplicate_counts = BTreeMap::<String, usize>::new();
    for id in unresolved {
        let base = suffix_name(&seeds[&id], seeds[&id].len());
        let count = duplicate_counts.entry(base.clone()).or_default();
        let candidate = loop {
            *count += 1;
            let candidate = format!("{base}{}", *count);
            if !reserved.contains(&candidate) && !names.values().any(|name| name == &candidate) {
                break candidate;
            }
        };
        names.insert(id, candidate);
    }
    names
}

pub fn fqn_seed(value: &str) -> Vec<String> {
    value
        .replace("::", ".")
        .split('.')
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn upper_camel(value: &str) -> String {
    words(value)
        .into_iter()
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

pub fn lower_camel(value: &str) -> String {
    let value = upper_camel(value);
    let chars = value.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return value;
    }
    let boundary = chars
        .windows(2)
        .position(|pair| pair[0].is_ascii_uppercase() && pair[1].is_ascii_lowercase());
    let prefix_length = match boundary {
        Some(0) => 1,
        Some(index) => index,
        None if chars.iter().all(|character| character.is_ascii_uppercase()) => chars.len(),
        None => 1,
    };
    chars[..prefix_length]
        .iter()
        .flat_map(|character| character.to_lowercase())
        .chain(chars[prefix_length..].iter().copied())
        .collect()
}

pub fn without_interface_prefix(value: &str) -> &str {
    value
        .strip_prefix('I')
        .filter(|rest| {
            rest.chars()
                .next()
                .is_some_and(|first| first.is_ascii_uppercase())
        })
        .unwrap_or(value)
}

pub fn without_enum_suffix(value: &str) -> &str {
    value.strip_suffix("Enum").unwrap_or(value)
}

fn suffix_name(seed: &[String], depth: usize) -> String {
    seed.iter()
        .skip(seed.len().saturating_sub(depth))
        .map(|part| upper_camel(part))
        .collect()
}

fn words(value: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let chars = value.chars().collect::<Vec<_>>();
    for (index, character) in chars.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !current.is_empty() {
                output.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = current.chars().last();
        let next = chars.get(index + 1).copied();
        let starts_word = character.is_ascii_uppercase()
            && previous.is_some_and(|value| value.is_ascii_lowercase())
            || character.is_ascii_uppercase()
                && previous.is_some_and(|value| value.is_ascii_uppercase())
                && next.is_some_and(|value| value.is_ascii_lowercase());
        if starts_word && !current.is_empty() {
            output.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        output.push(current);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_use_shortest_unique_pascal_suffix() {
        let seeds = BTreeMap::from([
            (
                "user-name".to_owned(),
                vec!["user".to_owned(), "name".to_owned()],
            ),
            (
                "user-age".to_owned(),
                vec!["user".to_owned(), "age".to_owned()],
            ),
            (
                "order-name".to_owned(),
                vec!["order".to_owned(), "name".to_owned()],
            ),
            (
                "order-id".to_owned(),
                vec!["order".to_owned(), "id".to_owned()],
            ),
        ]);

        assert_eq!(
            shortest_unique_names(&seeds),
            BTreeMap::from([
                ("order-id".to_owned(), "Id".to_owned()),
                ("order-name".to_owned(), "OrderName".to_owned()),
                ("user-age".to_owned(), "Age".to_owned()),
                ("user-name".to_owned(), "UserName".to_owned()),
            ])
        );
    }

    #[test]
    fn acronym_boundaries_are_stable() {
        assert_eq!(upper_camel("IRetailQRCodeFacade"), "IRetailQRCodeFacade");
        assert_eq!(lower_camel("QRCode"), "qrCode");
        assert_eq!(
            without_interface_prefix("IGoodsQueryFacade"),
            "GoodsQueryFacade"
        );
        assert_eq!(without_interface_prefix("Info"), "Info");
    }
}
