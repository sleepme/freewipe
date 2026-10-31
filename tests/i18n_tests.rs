//! Every language has every key, and every key used in the code exists.

use std::collections::BTreeSet;

use freewipe::i18n;

type Dict = std::collections::BTreeMap<String, String>;

fn locales_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/locales"))
}

/// Every language pack shipped in `locales/` (all but English).
fn packs() -> Vec<(String, Dict)> {
    let mut v: Vec<(String, Dict)> = std::fs::read_dir(locales_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .filter(|p| p.file_stem().unwrap() != "en")
        .map(|p| {
            let d: Dict = serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (p.file_stem().unwrap().to_string_lossy().to_string(), d)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn english() -> Dict {
    serde_json::from_str(&std::fs::read_to_string(locales_dir().join("en.json")).unwrap()).unwrap()
}

#[test]
fn shipped_packs_are_complete() {
    let en: BTreeSet<String> = english().into_keys().filter(|k| !k.starts_with('_')).collect();
    let packs = packs();
    assert!(packs.len() >= 6, "expected the shipped language packs");
    for (code, d) in packs {
        let keys: BTreeSet<String> = d.keys().filter(|k| !k.starts_with('_')).cloned().collect();
        let missing: Vec<_> = en.difference(&keys).collect();
        let extra: Vec<_> = keys.difference(&en).collect();
        assert!(missing.is_empty(), "{code} is missing {missing:?}");
        assert!(extra.is_empty(), "{code} has unknown keys {extra:?}");
        assert!(d.contains_key("_name"), "{code} has no _name");
    }
}

#[test]
fn placeholders_match_english() {
    let re = |s: &str| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            if let Some(j) = rest[i..].find('}') {
                out.insert(rest[i..i + j + 1].to_string());
                rest = &rest[i + j + 1..];
            } else {
                break;
            }
        }
        out
    };
    let en = english();
    for (code, d) in packs() {
        for (k, v) in &en {
            if k.starts_with('_') {
                continue;
            }
            assert_eq!(re(v), re(&d[k]), "placeholders differ for {code}:{k}");
        }
    }
}

#[test]
fn every_key_used_in_code_exists() {
    let en: BTreeSet<String> = i18n::english_keys().into_iter().collect();
    let mut missing = Vec::new();
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            let mut rest = text.as_str();
            if p.ends_with("i18n.rs") {
                continue;
            }
            while let Some(i) = rest.find("tr!(") {
                let standalone =
                    i == 0 || !rest.as_bytes()[i - 1].is_ascii_alphanumeric() && rest.as_bytes()[i - 1] != b'_';
                rest = &rest[i + 4..];
                if !standalone {
                    continue;
                }
                let r = rest.trim_start();
                if let Some(r) = r.strip_prefix('"') {
                    if let Some(end) = r.find('"') {
                        let key = &r[..end];
                        if !en.contains(key) {
                            missing.push(format!("{}: {key}", p.display()));
                        }
                    }
                }
            }
        }
    }
    assert!(missing.is_empty(), "keys missing from en.json: {missing:#?}");
}

#[test]
fn switching_language_works_and_defaults_to_english() {
    std::env::set_var("FREEWIPE_LOCALES_DIR", locales_dir());
    assert_eq!(i18n::current_language(), "en");
    let langs = i18n::languages();
    assert_eq!(langs[0].code, "en");
    assert!(langs.iter().any(|l| l.code == "uk" && l.complete > 0.99));
    assert!(i18n::set_language("uk"));
    assert_eq!(i18n::current_language(), "uk");
    assert!(!i18n::set_language("xx"));
    assert_eq!(i18n::current_language(), "uk");
    assert!(i18n::set_language("en"));
}
