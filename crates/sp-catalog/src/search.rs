//! Literal, field-aware Library search. User text is never SQL syntax.

use crate::{CatalogPhoto, Flag};

pub fn keyword_matches(keyword: &str, ancestor: &str) -> bool {
    let keyword = keyword.to_lowercase();
    let ancestor = ancestor.to_lowercase();
    keyword == ancestor
        || keyword
            .strip_prefix(&ancestor)
            .is_some_and(|tail| tail.starts_with('|'))
}

fn tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => quoted = !quoted,
            value if value.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            value => token.push(value),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens.retain(|token| token.chars().any(char::is_alphanumeric));
    tokens
}

fn number_matches(value: Option<u32>, expression: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    for (prefix, kind) in [(">=", 0), ("<=", 1), (">", 2), ("<", 3), ("=", 4), ("", 4)] {
        if let Some(number) = expression
            .strip_prefix(prefix)
            .and_then(|text| text.parse::<u32>().ok())
        {
            return match kind {
                0 => value >= number,
                1 => value <= number,
                2 => value > number,
                3 => value < number,
                _ => value == number,
            };
        }
    }
    false
}

pub fn matches(photo: &CatalogPhoto, query: &str) -> bool {
    let tokens = tokens(query);
    if tokens.is_empty() {
        return true;
    }
    tokens
        .split(|token| token.eq_ignore_ascii_case("OR"))
        .any(|group| {
            !group.is_empty()
                && group.iter().all(|token| {
                    let (negative, token) = token
                        .strip_prefix('-')
                        .map_or((false, token.as_str()), |value| (true, value));
                    let contains = |text: &str, needle: &str| {
                        text.to_lowercase().contains(&needle.to_lowercase())
                    };
                    let found = if let Some((field, value)) = token.split_once(':') {
                        match field.to_ascii_lowercase().as_str() {
                            "keyword" | "tag" => photo
                                .keywords
                                .iter()
                                .any(|keyword| keyword_matches(keyword, value)),
                            "camera" => contains(&photo.camera_model, value),
                            "lens" => contains(&photo.lens_model, value),
                            "caption" => contains(&photo.caption, value),
                            "path" | "name" => contains(&photo.path.to_string_lossy(), value),
                            "type" => photo.file_type.eq_ignore_ascii_case(value),
                            "rating" => {
                                number_matches(Some(u32::from(photo.annotations.rating)), value)
                            }
                            "iso" => number_matches(photo.iso, value),
                            "flag" => match value.to_ascii_lowercase().as_str() {
                                "pick" => photo.annotations.flag == Flag::Pick,
                                "reject" => photo.annotations.flag == Flag::Reject,
                                "none" => photo.annotations.flag == Flag::None,
                                _ => false,
                            },
                            _ => false,
                        }
                    } else {
                        contains(
                            &format!(
                                "{} {} {} {} {}",
                                photo.path.display(),
                                photo.caption,
                                photo.keywords.join(" "),
                                photo.camera_model,
                                photo.lens_model
                            ),
                            token,
                        )
                    };
                    found != negative
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hierarchy_respects_path_boundaries_and_quotes_keep_field_phrases() {
        assert!(keyword_matches("People|Family|Alice", "people|family"));
        assert!(!keyword_matches("People|FamilyTrip", "People|Family"));
        assert_eq!(
            tokens("camera:\"Sony A7\" -flag:reject OR keyword:Travel"),
            ["camera:Sony A7", "-flag:reject", "OR", "keyword:Travel"]
        );
        assert!(number_matches(Some(400), ">=200"));
        assert!(!number_matches(Some(400), "<200"));
        assert!(!number_matches(None, "0"));
    }
}
