//! Localization.
//!
//! Every user-visible string goes through [`t`] / [`tr!`]. English is built
//! into the executables and is the fallback for any key a translation lacks.
//!
//! **Other languages are language packs**: `<code>.json` files (ISO 639-1
//! code, e.g. `de.json`, `pt-BR.json` → `pt`) in a `locales` folder. A pack
//! is a flat JSON object — `"key": "text with {placeholder}"` — plus a few
//! metadata keys starting with `_` (`_name`: the language's own name,
//! `_name_en`, `_translators`, `_freewipe`: the version it was made for). The
//! complete format is described in `locales/README.md`; `freewipe languages`
//! lists what is installed and how complete each pack is.
//!
//! Folders searched, later ones overriding earlier ones key by key:
//! `locales` next to the executable, `%APPDATA%\freewipe\locales`, and the
//! folder named by `FREEWIPE_LOCALES_DIR`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

/// Default UI language.
pub const DEFAULT_LANG: &str = "en";

/// English, the only language compiled in.
const ENGLISH: &str = include_str!("../locales/en.json");

type Dict = BTreeMap<String, String>;

struct State {
    code: String,
    current: Dict,
    fallback: Dict,
}

/// Extra English dictionaries registered by programs built on the library.
static EXTRA: RwLock<Vec<&'static str>> = RwLock::new(Vec::new());

/// Built-in English: freewipe's strings plus every registered dictionary.
fn english() -> Dict {
    let mut d = parse(ENGLISH).unwrap_or_default();
    for extra in EXTRA.read().map(|g| g.clone()).unwrap_or_default() {
        if let Some(x) = parse(extra) {
            d.extend(x);
        }
    }
    d
}

/// Add a program's own English strings (a flat JSON object like
/// `locales/en.json`). They become part of the built-in fallback; language
/// packs translate them like any other key. Call before [`set_language`].
pub fn register_english(json: &'static str) {
    if let Ok(mut g) = EXTRA.write() {
        g.push(json);
    }
    let en = english();
    let mut s = state().write().unwrap_or_else(|e| e.into_inner());
    if s.code == DEFAULT_LANG {
        s.current = en.clone();
    } else {
        for (k, v) in &en {
            s.current.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    s.fallback = en;
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| {
        let en = english();
        RwLock::new(State { code: DEFAULT_LANG.into(), current: en.clone(), fallback: en })
    })
}

fn parse(json: &str) -> Option<Dict> {
    serde_json::from_str::<Dict>(json).ok()
}

/// Folders searched for language packs, in order of precedence (lowest first).
pub fn locale_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("locales"))) {
        v.push(dir);
    }
    v.push(crate::settings::config_dir().join("locales"));
    if let Ok(d) = std::env::var("FREEWIPE_LOCALES_DIR") {
        v.push(PathBuf::from(d));
    }
    v
}

/// Pack files for `code` (`de.json`, also `de-DE.json` / `de_DE.json`).
fn pack_files(code: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in locale_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .filter(|p| p.file_stem().and_then(|s| s.to_str()).map(normalize).as_deref() == Some(code))
            .collect();
        found.sort();
        out.extend(found);
    }
    out
}

/// Dictionary for `code`: English built in; other languages from packs.
fn load(code: &str) -> Option<Dict> {
    let mut dict = (code == DEFAULT_LANG).then(english);
    for p in pack_files(code) {
        if let Some(ext) = std::fs::read_to_string(&p).ok().and_then(|s| parse(&s)) {
            dict.get_or_insert_with(Dict::new).extend(ext);
        }
    }
    dict
}

/// Normalise `"ru-RU"`, `"uk_UA.UTF-8"`, `"EN"` → `"ru"`, `"uk"`, `"en"`.
pub fn normalize(code: &str) -> String {
    code.split(['-', '_', '.']).next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Switch the UI language. Returns `false` (and keeps the current language)
/// if no dictionary exists for `code`.
pub fn set_language(code: &str) -> bool {
    let code = normalize(code);
    match load(&code) {
        Some(d) => {
            let mut s = state().write().unwrap_or_else(|e| e.into_inner());
            s.code = code;
            s.current = d;
            true
        }
        None => false,
    }
}

/// Code of the active language (e.g. `"en"`).
pub fn current_language() -> String {
    state().read().unwrap_or_else(|e| e.into_inner()).code.clone()
}

/// An installed language.
#[derive(Clone, Debug)]
pub struct LangInfo {
    pub code: String,
    /// The language's own name (`_name`).
    pub name: String,
    /// Share of the English keys it translates (0.0–1.0).
    pub complete: f32,
    /// Pack file (`None` for built-in English).
    pub path: Option<PathBuf>,
}

/// Every available language: English first, then the packs by code.
/// Cached for a few seconds (the settings page asks every frame).
pub fn languages() -> Vec<LangInfo> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<LangInfo>)>> = Mutex::new(None);
    if let Ok(mut c) = CACHE.lock() {
        if let Some((at, v)) = c.as_ref() {
            if at.elapsed() < Duration::from_secs(5) {
                return v.clone();
            }
        }
        let v = scan_languages();
        *c = Some((Instant::now(), v.clone()));
        return v;
    }
    scan_languages()
}

fn scan_languages() -> Vec<LangInfo> {
    let en = english();
    let total = en.keys().filter(|k| !k.starts_with('_')).count().max(1);
    let mut out = vec![LangInfo {
        code: DEFAULT_LANG.into(),
        name: en.get("_name").cloned().unwrap_or_else(|| "English".into()),
        complete: 1.0,
        path: None,
    }];
    let mut codes: Vec<(String, PathBuf)> = Vec::new();
    for dir in locale_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(code) = p.file_stem().and_then(|s| s.to_str()).map(normalize) else { continue };
            if code.is_empty() || code == DEFAULT_LANG || code.starts_with('_') {
                continue;
            }
            codes.retain(|(c, _)| *c != code);
            codes.push((code, p));
        }
    }
    codes.sort();
    for (code, path) in codes {
        let Some(d) = load(&code) else { continue };
        let done = en.keys().filter(|k| !k.starts_with('_') && d.contains_key(*k)).count();
        out.push(LangInfo {
            name: d.get("_name").cloned().unwrap_or_else(|| code.clone()),
            complete: done as f32 / total as f32,
            code,
            path: Some(path),
        });
    }
    out
}

/// All available languages as `(code, display name)`.
pub fn available_languages() -> Vec<(String, String)> {
    languages().into_iter().map(|l| (l.code, l.name)).collect()
}

/// Translate `key`. Falls back to English, then to the key itself.
pub fn t(key: &str) -> String {
    let s = state().read().unwrap_or_else(|e| e.into_inner());
    s.current.get(key).or_else(|| s.fallback.get(key)).cloned().unwrap_or_else(|| key.to_string())
}

/// Translate `key` and substitute `{name}` placeholders.
pub fn tf(key: &str, args: &[(&str, String)]) -> String {
    let mut s = t(key);
    for (name, val) in args {
        s = s.replace(&format!("{{{name}}}"), val);
    }
    s
}

/// The English keys (used by tests).
pub fn english_keys() -> Vec<String> {
    parse(ENGLISH).map(|d| d.into_keys().collect()).unwrap_or_default()
}

/// `tr!("key")` or `tr!("key", name = value, ...)`.
#[macro_export]
macro_rules! tr {
    ($key:literal) => {
        $crate::i18n::t($key)
    };
    ($key:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::tf($key, &[$((stringify!($name), ($val).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codes() {
        assert_eq!(normalize("ru-RU"), "ru");
        assert_eq!(normalize("uk_UA.UTF-8"), "uk");
        assert_eq!(normalize("EN"), "en");
    }

    #[test]
    fn placeholders_substitute() {
        // Uses a key guaranteed to exist with a placeholder.
        let s = tf("cli.passes_n", &[("n", "7".into())]);
        assert!(s.contains('7'), "{s}");
    }
}
