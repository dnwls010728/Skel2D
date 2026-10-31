//! UI language. Every user-facing string is written in English in the code
//! and looked up here, so a missing translation shows English instead of
//! nothing. `tr` takes the exact English text; `tf!` fills `{}`
//! placeholders after the lookup (pre-format numbers, it has no specs).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    #[default]
    Korean,
    English,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Korean, Lang::English];

    /// The language's own name, for the picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Korean => "한국어",
            Lang::English => "English",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::English as u8 { Lang::English } else { Lang::Korean }
}

/// The current language's text for `en`.
pub fn tr(en: &str) -> &str {
    #[cfg(test)]
    SEEN.with(|s| s.borrow_mut().push(en.to_string()));
    if current() == Lang::English {
        return en;
    }
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| KO_TABLES.iter().flat_map(|t| t.iter().copied()).collect()).get(en).copied().unwrap_or(en)
}

/// Replaces each `{}` in `template` with the next argument.
pub fn fill(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        out.push_str(args.next().map_or("", String::as_str));
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// `format!` for translated templates: `tf!("Opened {}", path.display())`.
#[macro_export]
macro_rules! tf {
    ($en:literal $(, $arg:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($en), &[$(format!("{}", $arg)),*])
    };
}

mod ko_app;
mod ko_dialogs;
mod ko_panels;
mod ko_steps;
mod ko_timeline;
mod ko_viewport;

/// Korean tables, one per source file so they can be edited independently.
/// The same English text may appear in several only with the same Korean.
pub(crate) const KO_TABLES: [&[(&str, &str)]; 6] = [ko_dialogs::KO, ko_app::KO, ko_panels::KO, ko_timeline::KO, ko_viewport::KO, ko_steps::KO];
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_in_order() {
        assert_eq!(fill("{} of {}", &["1".into(), "3".into()]), "1 of 3");
        assert_eq!(fill("no args", &[]), "no args");
        assert_eq!(fill("{} {}", &["a".into()]), "a ");
    }

    #[test]
    fn tables_agree_on_shared_keys() {
        let mut seen = HashMap::new();
        for (en, ko) in KO_TABLES.iter().flat_map(|t| t.iter()) {
            if let Some(prev) = seen.insert(en, ko) {
                assert_eq!(prev, ko, "{en:?} translated two ways");
            }
        }
    }
}

// Test support: every key `tr` was asked for on this thread, and whether Korean has it.
#[cfg(test)]
thread_local! {
    pub static SEEN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub fn has_korean(en: &str) -> bool {
    KO_TABLES.iter().any(|t| t.iter().any(|(k, _)| *k == en))
}
