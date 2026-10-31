//! File-backed localization seam with English fallback.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    OnceLock,
};

thread_local! { static ACTIVE: Cell<Option<Language>> = const { Cell::new(None) }; }
// File-picker workers inherit the application's language; UI threads retain
// their own choice so independent contexts can be tested concurrently.
static APP_LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// The UI thread chooses the language once per frame. Stable source labels
/// serve as catalog keys; user-entered names and underlying data stay intact.
pub fn set_language(language: Language) {
    ACTIVE.set(Some(language));
}

/// Only the real application's UI publishes a default for file-picker and
/// background workers. Independent UI contexts keep thread-local choices.
pub fn set_application_language(language: Language) {
    set_language(language);
    APP_LANGUAGE.store(u8::from(language == Language::Dutch), Ordering::Relaxed);
}

pub fn text(source: &str) -> &str {
    translate(
        ACTIVE.get().unwrap_or_else(|| {
            if APP_LANGUAGE.load(Ordering::Relaxed) == 1 {
                Language::Dutch
            } else {
                Language::English
            }
        }),
        source,
    )
}

pub fn translate(language: Language, source: &str) -> &str {
    match language {
        Language::English => source,
        Language::Dutch => dutch_ui().get(source).map(String::as_str).unwrap_or(source),
    }
}

/// Translate a template, substituting independently formatted values without
/// interpreting their contents. Names, filenames, braces and diagnostics stay
/// verbatim. Indexed placeholders allow a translation to reorder its arguments.
pub fn format(source: &str, values: &[String]) -> String {
    interpolate(text(source), values)
        .unwrap_or_else(|| interpolate(source, values).unwrap_or_else(|| source.to_owned()))
}

fn interpolate(template: &str, values: &[String]) -> Option<String> {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(['{', '}']) {
        result.push_str(&rest[..start]);
        let brace = rest.as_bytes()[start];
        rest = &rest[start + 1..];
        if brace == b'}' {
            rest = rest.strip_prefix('}')?;
            result.push('}');
            continue;
        }
        if let Some(escaped) = rest.strip_prefix('{') {
            result.push('{');
            rest = escaped;
            continue;
        }
        let end = rest.find('}')?;
        let index: usize = rest[..end].parse().ok()?;
        result.push_str(values.get(index)?);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Some(result)
}

fn dutch_ui() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse(include_str!("../../../assets/i18n/nl-ui.json")))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    English,
    Dutch,
}

#[derive(Debug, Clone, Copy)]
pub enum Text {
    Open,
    Export,
    View,
    SearchPhotos,
}

impl Text {
    const fn key(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Export => "export",
            Self::View => "view",
            Self::SearchPhotos => "search_photos",
        }
    }
}

fn parse(source: &str) -> HashMap<String, String> {
    serde_json::from_str(source).expect("embedded locale must be valid JSON")
}

fn english() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse(include_str!("../../../assets/i18n/en.json")))
}

fn dutch() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| parse(include_str!("../../../assets/i18n/nl.json")))
}

pub fn tr(language: Language, text: Text) -> &'static str {
    let key = text.key();
    let localized = match language {
        Language::English => english(),
        Language::Dutch => dutch(),
    };
    localized
        .get(key)
        .or_else(|| english().get(key))
        .map(String::as_str)
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_files_have_matching_keys() {
        let mut en = english().keys().collect::<Vec<_>>();
        let mut nl = dutch().keys().collect::<Vec<_>>();
        en.sort();
        nl.sort();
        assert_eq!(en, nl);
    }

    #[test]
    fn static_photo_controls_have_catalog_entries() {
        fn files(directory: &std::path::Path, output: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files(&path, output);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_none_or(|n| n != "i18n.rs")
                {
                    output.push(path);
                }
            }
        }
        let mut paths = Vec::new();
        files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut paths,
        );
        let mut keys = std::collections::BTreeSet::new();
        for path in paths {
            let source = std::fs::read_to_string(path).unwrap();
            for marker in [
                "i18n::text(",
                "i18n::format(",
                "param_slider(",
                "param_slider_spec(",
                "sub_heading(",
            ] {
                for suffix in source.split(marker).skip(1) {
                    let suffix = if marker.starts_with("i18n::") {
                        suffix
                    } else {
                        suffix.split_once(',').map_or("", |(_, rest)| rest)
                    };
                    let suffix = suffix.trim_start();
                    if suffix.starts_with('"') {
                        if let Some(Ok(key)) = serde_json::Deserializer::from_str(suffix)
                            .into_iter::<String>()
                            .next()
                        {
                            keys.insert(key);
                        }
                    }
                }
            }
        }
        assert!(keys.len() > 650, "coverage scan failed");
        let missing: Vec<_> = keys
            .iter()
            .filter(|key| !dutch_ui().contains_key(*key))
            .collect();
        assert!(
            missing.is_empty(),
            "Missing Dutch control translations: {missing:?}"
        );
        for (key, value) in dutch_ui() {
            if !key.trim().is_empty() {
                assert!(!value.trim().is_empty(), "Empty translation for {key}");
            }
        }
    }

    #[test]
    fn core_photo_controls_translate_and_user_names_fall_back() {
        assert_eq!(translate(Language::Dutch, "Restore blur"), "Onscherpte");
        assert_eq!(translate(Language::Dutch, "Exposure"), "Belichting");
        assert_eq!(
            translate(Language::Dutch, "family-2026-09-29.dng"),
            "family-2026-09-29.dng"
        );
    }

    #[test]
    fn templates_preserve_data_and_allow_reordered_values() {
        let filename = "Sky {raw} }} / vakantié\n{}".to_owned();
        assert_eq!(
            interpolate("{1}: {0}", &[filename.clone(), "12".into()]),
            Some(format!("12: {filename}"))
        );
        assert_eq!(
            interpolate("{{{0}}}", std::slice::from_ref(&filename)),
            Some(format!("{{{filename}}}"))
        );
        set_language(Language::Dutch);
        assert_eq!(
            format("Created {0}", std::slice::from_ref(&filename)),
            format!("{filename} gemaakt")
        );
        assert_eq!(text(&filename), filename);
        set_language(Language::English);
        assert_eq!(
            format("Created {0}", &[filename]),
            "Created Sky {raw} }} / vakantié\n{}"
        );
        assert!(interpolate("Missing {2}", &["one".into()]).is_none());
    }

    #[test]
    fn translated_templates_keep_exact_placeholder_sets() {
        fn indices(s: &str) -> Vec<usize> {
            let mut values = Vec::new();
            for part in s.split('{').skip(1) {
                if let Some(end) = part.find('}') {
                    if let Ok(value) = part[..end].parse() {
                        values.push(value);
                    }
                }
            }
            values.sort();
            values
        }
        for (source, translated) in dutch_ui() {
            assert_eq!(
                indices(source),
                indices(translated),
                "Changed translation placeholders: {source}"
            );
        }
    }

    #[test]
    fn builtin_presets_and_inspector_titles_have_translations() {
        for preset in sp_core::preset::DevelopPreset::ALL {
            for source in [preset.name(), preset.category(), preset.description()] {
                assert!(
                    dutch_ui().contains_key(source),
                    "Missing built-in preset text: {source}"
                );
            }
        }
        for group in crate::edit_groups::Group::ALL {
            assert!(
                dutch_ui().contains_key(group.title()),
                "Missing inspector title: {}",
                group.title()
            );
        }
    }

    #[test]
    fn independent_ui_contexts_keep_their_own_language() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = [
            (Language::English, "Exposure"),
            (Language::Dutch, "Belichting"),
        ]
        .into_iter()
        .map(|(language, expected)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                set_language(language);
                barrier.wait();
                assert_eq!(text("Exposure"), expected);
            })
        })
        .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn dutch_is_loaded_from_the_catalog() {
        assert_eq!(tr(Language::Dutch, Text::Export), "Exporteren");
    }
}
