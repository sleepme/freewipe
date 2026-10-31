//! Correctness of the method catalogue: pass counts, uniqueness, key
//! round-trips and override behaviour. These lock the standards in place so a
//! later refactor can't silently change, say, Gutmann from 35 passes.

use freewipe::methods::{Method, DEFAULT_RANDOM_PASSES};
use freewipe::pattern::Pass;

#[test]
fn keys_are_unique() {
    let mut keys: Vec<&str> = Method::ALL.iter().map(|m| m.key()).collect();
    let before = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(before, keys.len(), "duplicate method key(s) in ALL");
}

#[test]
fn from_key_roundtrips_case_insensitively() {
    for m in Method::ALL {
        assert_eq!(Method::from_key(m.key()), Some(*m));
        assert_eq!(Method::from_key(&m.key().to_uppercase()), Some(*m));
    }
    assert_eq!(Method::from_key("definitely-not-a-method"), None);
}

#[test]
fn fixed_pass_counts_match_standards() {
    let cases = [
        (Method::Zero, 1),
        (Method::One, 1),
        (Method::Random, 1),
        (Method::Random3, 3),
        (Method::NistClear, 1),
        (Method::Gost, 2),
        (Method::HmgIs5Base, 1),
        (Method::HmgIs5Enh, 3),
        (Method::DodE, 3),
        (Method::DodEce, 7),
        (Method::ArmyAr38019, 3),
        (Method::Af5020, 3),
        (Method::Navso523926, 3),
        (Method::RcmpOpsII, 7),
        (Method::Vsitr, 7),
        (Method::Schneier, 7),
        (Method::Pfitzner, 33),
        (Method::Gutmann, 35),
    ];
    for (m, expected) in cases {
        assert_eq!(m.pass_count(None), expected, "{} should have {} passes", m.key(), expected);
    }
}

#[test]
fn gutmann_structure_is_exact() {
    let p = Method::Gutmann.passes(None);
    assert_eq!(p.len(), 35);
    // First four and last four are random; the 27 in the middle are patterns.
    assert!(p[0..4].iter().all(|x| matches!(x, Pass::Random)));
    assert!(p[31..35].iter().all(|x| matches!(x, Pass::Random)));
    assert!(p[4..31].iter().all(|x| matches!(x, Pass::Pattern(_))));
}

#[test]
fn random_n_respects_override_and_floor() {
    assert_eq!(Method::RandomN.pass_count(None), DEFAULT_RANDOM_PASSES);
    assert_eq!(Method::RandomN.pass_count(Some(7)), 7);
    // Never degenerate to zero passes.
    assert_eq!(Method::RandomN.pass_count(Some(0)), 1);
    assert!(Method::RandomN.passes(Some(5)).iter().all(|x| matches!(x, Pass::Random)));
}

#[test]
fn dod_e_is_byte_complement_random() {
    let p = Method::DodE.passes(None);
    assert_eq!(p[0], Pass::Fixed(0x00));
    assert_eq!(p[1], Pass::Fixed(0xFF));
    assert!(matches!(p[2], Pass::Random));
}

#[test]
fn every_method_has_at_least_one_pass() {
    for m in Method::ALL {
        assert!(m.pass_count(None) >= 1, "{} has no passes", m.key());
    }
}

#[test]
fn custom_methods_register_and_resolve() {
    use freewipe::methods::{set_custom, CustomMethod};
    use freewipe::pattern::Pass;
    let c = CustomMethod {
        name: "My Wipe 1".into(),
        passes: vec!["00".into(), "random".into(), "92 49 24".into()],
        verify: true,
    };
    assert_eq!(c.key(), "custom-my-wipe-1");
    let passes = c.validate().unwrap();
    assert_eq!(passes[0], Pass::Fixed(0));
    assert_eq!(passes[1], Pass::Random);
    assert!(matches!(&passes[2], Pass::Pattern(p) if p.as_ref() == [0x92, 0x49, 0x24]));
    assert!(CustomMethod { name: "x".into(), passes: vec!["0".into()], verify: false }.validate().is_err());
    assert!(CustomMethod { name: " ".into(), passes: vec!["00".into()], verify: false }.validate().is_err());
    set_custom(&[c]);
    let m = freewipe::Method::from_key("custom-my-wipe-1").expect("registered");
    assert_eq!(m.name(), "My Wipe 1");
    assert_eq!(m.pass_count(None), 3);
    assert!(m.verify_recommended());
    assert!(freewipe::Method::all().contains(&m));
    set_custom(&[]);
}
