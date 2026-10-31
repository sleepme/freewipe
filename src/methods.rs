//! Catalogue of secure-erase methods.
//!
//! Each method is an ordered sequence of [`Pass`]es. This table is the single
//! place to add a new standard: implement [`Method::passes`] and list the
//! variant in [`Method::ALL`]. Everything here is pure logic and therefore
//! identical on every target platform.
//!
//! Honest note carried into the UI: on modern flash media (SSD/NVMe/USB)
//! wear-levelling means overwriting logical blocks does *not* guarantee the
//! old data is gone. NIST SP 800-88 treats a single pass as sufficient for
//! magnetic media; the long legacy schemes (Gutmann's 35, Pfitzner's 33) are
//! included for completeness, not because they are needed today.

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::pattern::Pass;

/// A method defined by the user (Settings → Methods), stored in settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CustomMethod {
    pub name: String,
    /// One entry per pass: `"random"` or hex bytes (`"00"`, `"FF"`, `"92 49 24"`).
    pub passes: Vec<String>,
    /// Read the last pass back and compare.
    #[serde(default)]
    pub verify: bool,
}

/// Most passes a custom method may have.
pub const MAX_CUSTOM_PASSES: usize = 99;

impl CustomMethod {
    /// Parse one pass spec.
    pub fn parse_pass(spec: &str) -> Option<Pass> {
        let t = spec.trim();
        if t.eq_ignore_ascii_case("random") || t.eq_ignore_ascii_case("rnd") {
            return Some(Pass::Random);
        }
        let hex: String = t.chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
        let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
        if hex.is_empty() || hex.len() % 2 != 0 || hex.len() > 64 {
            return None;
        }
        let bytes: Option<Vec<u8>> =
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
        let bytes = bytes?;
        Some(if bytes.len() == 1 { Pass::Fixed(bytes[0]) } else { Pass::Pattern(bytes.into()) })
    }

    /// The passes, or why the definition is invalid (localised).
    pub fn validate(&self) -> Result<Vec<Pass>, String> {
        if self.name.trim().is_empty() {
            return Err(crate::tr!("custom.err_name"));
        }
        if self.passes.is_empty() || self.passes.len() > MAX_CUSTOM_PASSES {
            return Err(crate::tr!("custom.err_count", max = MAX_CUSTOM_PASSES));
        }
        self.passes
            .iter()
            .enumerate()
            .map(|(i, p)| Self::parse_pass(p).ok_or_else(|| crate::tr!("custom.err_pass", n = i + 1, spec = p)))
            .collect()
    }

    /// Key used with `--method`: `custom-<name>` in lowercase with dashes.
    pub fn key(&self) -> String {
        let slug: String = self
            .name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        format!("custom-{slug}")
    }
}

/// A registered custom method (strings leaked once so `Method` stays `Copy`
/// and keeps returning `&'static str` like the built-in ones).
struct Registered {
    key: &'static str,
    name: &'static str,
    description: &'static str,
    passes: Vec<Pass>,
    verify: bool,
}

static CUSTOM: RwLock<Vec<Registered>> = RwLock::new(Vec::new());

/// Make the user's custom methods available (called when settings load or
/// change). Invalid definitions are skipped.
pub fn set_custom(list: &[CustomMethod]) {
    let mut v = Vec::new();
    for c in list {
        let Ok(passes) = c.validate() else { continue };
        if v.iter().any(|r: &Registered| r.key == c.key()) || v.len() >= 200 {
            continue;
        }
        let desc = format!("Custom: {} pass(es){}", passes.len(), if c.verify { ", verify" } else { "" });
        v.push(Registered {
            key: Box::leak(c.key().into_boxed_str()),
            name: Box::leak(c.name.trim().to_string().into_boxed_str()),
            description: Box::leak(desc.into_boxed_str()),
            passes,
            verify: c.verify,
        });
    }
    if let Ok(mut g) = CUSTOM.write() {
        *g = v;
    }
}

fn with_custom<T>(i: u8, f: impl FnOnce(&Registered) -> T) -> Option<T> {
    CUSTOM.read().ok().and_then(|g| g.get(i as usize).map(f))
}

/// The 27 deterministic Gutmann patterns (passes 5–31 of the full 35), aimed
/// at the RLL/MFM encodings of 1990s hard drives.
mod gutmann {
    pub static PATTERNS: &[&[u8]] = &[
        &[0x55],
        &[0xAA],
        &[0x92, 0x49, 0x24],
        &[0x49, 0x24, 0x92],
        &[0x24, 0x92, 0x49],
        &[0x00],
        &[0x11],
        &[0x22],
        &[0x33],
        &[0x44],
        &[0x55],
        &[0x66],
        &[0x77],
        &[0x88],
        &[0x99],
        &[0xAA],
        &[0xBB],
        &[0xCC],
        &[0xDD],
        &[0xEE],
        &[0xFF],
        &[0x92, 0x49, 0x24],
        &[0x49, 0x24, 0x92],
        &[0x24, 0x92, 0x49],
        &[0x6D, 0xB6, 0xDB],
        &[0xB6, 0xDB, 0x6D],
        &[0xDB, 0x6D, 0xB6],
    ];
}

/// Default pass count for the user-tunable `RandomN` method.
pub const DEFAULT_RANDOM_PASSES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Zero,
    One,
    Random,
    Random3,
    RandomN,
    NistClear,
    Gost,
    HmgIs5Base,
    HmgIs5Enh,
    DodE,
    DodEce,
    ArmyAr38019,
    Af5020,
    Navso523926,
    RcmpOpsII,
    Vsitr,
    Schneier,
    Pfitzner,
    Gutmann,
    /// The n-th custom method (see [`set_custom`]).
    Custom(u8),
}

impl Method {
    /// Every method, in display order. Adding a variant here lists it in
    /// `freewipe methods` and the tests automatically cover it.
    pub const ALL: &'static [Method] = &[
        Method::Zero,
        Method::One,
        Method::Random,
        Method::Random3,
        Method::RandomN,
        Method::NistClear,
        Method::Gost,
        Method::HmgIs5Base,
        Method::HmgIs5Enh,
        Method::DodE,
        Method::DodEce,
        Method::ArmyAr38019,
        Method::Af5020,
        Method::Navso523926,
        Method::RcmpOpsII,
        Method::Vsitr,
        Method::Schneier,
        Method::Pfitzner,
        Method::Gutmann,
    ];

    /// CLI key accepted by `--method`.
    pub fn key(&self) -> &'static str {
        match self {
            Method::Zero => "zero",
            Method::One => "one",
            Method::Random => "random",
            Method::Random3 => "random3",
            Method::RandomN => "randomN",
            Method::NistClear => "nist-clear",
            Method::Gost => "gost",
            Method::HmgIs5Base => "hmg-is5-base",
            Method::HmgIs5Enh => "hmg-is5-enh",
            Method::DodE => "dod-e",
            Method::DodEce => "dod-ece",
            Method::ArmyAr38019 => "army-ar380-19",
            Method::Af5020 => "af-5020",
            Method::Navso523926 => "navso-5239-26",
            Method::RcmpOpsII => "rcmp-ops-ii",
            Method::Vsitr => "vsitr",
            Method::Schneier => "schneier",
            Method::Pfitzner => "pfitzner",
            Method::Gutmann => "gutmann",
            Method::Custom(i) => with_custom(*i, |r| r.key).unwrap_or("custom"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Method::Zero => "Zeroes",
            Method::One => "Ones",
            Method::Random => "Pseudorandom",
            Method::Random3 => "Pseudorandom x3",
            Method::RandomN => "Pseudorandom xN",
            Method::NistClear => "NIST SP 800-88 Clear",
            Method::Gost => "GOST R 50739-95",
            Method::HmgIs5Base => "British HMG IS5 Baseline",
            Method::HmgIs5Enh => "British HMG IS5 Enhanced",
            Method::DodE => "US DoD 5220.22-M (E)",
            Method::DodEce => "US DoD 5220.22-M (ECE)",
            Method::ArmyAr38019 => "US Army AR 380-19",
            Method::Af5020 => "US Air Force SSI 5020",
            Method::Navso523926 => "US Navy NAVSO P-5239-26",
            Method::RcmpOpsII => "Canadian RCMP TSSIT OPS-II",
            Method::Vsitr => "German VSITR (BSI)",
            Method::Schneier => "Bruce Schneier",
            Method::Pfitzner => "Pfitzner",
            Method::Gutmann => "Peter Gutmann",
            Method::Custom(i) => with_custom(*i, |r| r.name).unwrap_or("Custom"),
        }
    }

    /// Whether reading back and verifying the final pass is part of the
    /// standard (or simply advisable for it).
    pub fn verify_recommended(&self) -> bool {
        if let Method::Custom(i) = self {
            return with_custom(*i, |r| r.verify).unwrap_or(false);
        }
        matches!(
            self,
            Method::NistClear
                | Method::HmgIs5Enh
                | Method::DodE
                | Method::DodEce
                | Method::ArmyAr38019
                | Method::Navso523926
        )
    }

    /// Resolve a `--method` key, case-insensitively.
    pub fn from_key(key: &str) -> Option<Method> {
        Method::all().into_iter().find(|m| m.key().eq_ignore_ascii_case(key.trim()))
    }

    /// Built-in methods followed by the user's custom ones.
    pub fn all() -> Vec<Method> {
        let mut v = Method::ALL.to_vec();
        let n = CUSTOM.read().map(|g| g.len()).unwrap_or(0);
        v.extend((0..n.min(200)).map(|i| Method::Custom(i as u8)));
        v
    }

    pub fn is_custom(&self) -> bool {
        matches!(self, Method::Custom(_))
    }

    /// The ordered passes for this method.
    ///
    /// `n_override` sets the pass count for [`Method::RandomN`] (and is ignored
    /// by the fixed-length standards).
    pub fn passes(&self, n_override: Option<usize>) -> Vec<Pass> {
        use Pass::*;
        match self {
            Method::Zero => vec![Fixed(0x00)],
            Method::One => vec![Fixed(0xFF)],
            Method::Random => vec![Random],
            Method::Random3 => vec![Random, Random, Random],
            Method::RandomN => {
                let n = n_override.unwrap_or(DEFAULT_RANDOM_PASSES).max(1);
                vec![Random; n]
            }
            // NIST SP 800-88r1 "Clear": a single overwrite, verify.
            Method::NistClear => vec![Fixed(0x00)],
            // GOST R 50739-95: zeros, then random.
            Method::Gost => vec![Fixed(0x00), Random],
            // HMG Infosec Standard 5.
            Method::HmgIs5Base => vec![Fixed(0x00)],
            Method::HmgIs5Enh => vec![Fixed(0x00), Fixed(0xFF), Random],
            // DoD 5220.22-M (E): a byte, its complement, random (+verify).
            Method::DodE => vec![Fixed(0x00), Fixed(0xFF), Random],
            // DoD 5220.22-M (ECE): (E), a random pass, (E) again = 7 passes.
            Method::DodEce => vec![Fixed(0x00), Fixed(0xFF), Random, Random, Fixed(0x00), Fixed(0xFF), Random],
            // US Army AR 380-19: random, a byte, its complement (+verify).
            Method::ArmyAr38019 => vec![Random, Fixed(0x00), Fixed(0xFF)],
            // US Air Force SSI 5020: zeros, ones, any character.
            Method::Af5020 => vec![Fixed(0x00), Fixed(0xFF), Random],
            // US Navy NAVSO P-5239-26: a byte, its complement, random (+verify).
            Method::Navso523926 => vec![Fixed(0x01), Fixed(0xFE), Random],
            // RCMP TSSIT OPS-II: alternating 0x00/0xFF, final random = 7 passes.
            Method::RcmpOpsII => {
                vec![Fixed(0x00), Fixed(0xFF), Fixed(0x00), Fixed(0xFF), Fixed(0x00), Fixed(0xFF), Random]
            }
            // German VSITR / BSI: three 0x00/0xFF alternations, final 0xAA.
            Method::Vsitr => {
                vec![Fixed(0x00), Fixed(0xFF), Fixed(0x00), Fixed(0xFF), Fixed(0x00), Fixed(0xFF), Fixed(0xAA)]
            }
            // Schneier: 0xFF, 0x00, then five random passes = 7 passes.
            Method::Schneier => vec![Fixed(0xFF), Fixed(0x00), Random, Random, Random, Random, Random],
            // Pfitzner: 33 random passes.
            Method::Pfitzner => vec![Random; 33],
            // Gutmann: 4 random, 27 encoded patterns, 4 random = 35 passes.
            Method::Custom(i) => with_custom(*i, |r| r.passes.clone()).unwrap_or_else(|| vec![Random]),
            Method::Gutmann => {
                let mut v = Vec::with_capacity(35);
                v.extend(std::iter::repeat(Random).take(4));
                for p in gutmann::PATTERNS {
                    v.push(Pattern(std::borrow::Cow::Borrowed(p)));
                }
                v.extend(std::iter::repeat(Random).take(4));
                v
            }
        }
    }

    /// Number of passes this method performs (with the given override).
    pub fn pass_count(&self, n_override: Option<usize>) -> usize {
        self.passes(n_override).len()
    }

    /// Description in the current UI language (falls back to English).
    pub fn localized_description(&self) -> String {
        if let Method::Custom(_) = self {
            let n = self.pass_count(None);
            return if self.verify_recommended() {
                crate::tr!("custom.desc_verify", n = n)
            } else {
                crate::tr!("custom.desc", n = n)
            };
        }
        let key = format!("method.{}", self.key());
        let s = crate::i18n::t(&key);
        if s == key {
            self.description().to_string()
        } else {
            s
        }
    }

    /// One-line description for `freewipe methods` (English).
    pub fn description(&self) -> &'static str {
        match self {
            Method::Zero => "Single pass of 0x00.",
            Method::One => "Single pass of 0xFF.",
            Method::Random => "Single pass of random data.",
            Method::Random3 => "Three passes of random data.",
            Method::RandomN => "N passes of random data (set with --passes).",
            Method::NistClear => "Modern baseline: one overwrite with verification.",
            Method::Gost => "Russian standard: zeros then random.",
            Method::HmgIs5Base => "UK baseline: a single zero pass.",
            Method::HmgIs5Enh => "UK enhanced: zeros, ones, random (verify).",
            Method::DodE => "US DoD: byte, complement, random (verify).",
            Method::DodEce => "US DoD extended: 7 passes with verification.",
            Method::ArmyAr38019 => "US Army: random, byte, complement (verify).",
            Method::Af5020 => "US Air Force: zeros, ones, random.",
            Method::Navso523926 => "US Navy: byte, complement, random (verify).",
            Method::RcmpOpsII => "Canadian: alternating 0x00/0xFF then random, 7 passes.",
            Method::Vsitr => "German BSI: 0x00/0xFF alternations then 0xAA, 7 passes.",
            Method::Schneier => "Schneier: 0xFF, 0x00, then 5 random, 7 passes.",
            Method::Pfitzner => "Pfitzner: 33 random passes (legacy).",
            Method::Gutmann => "Gutmann: 35 passes for old RLL/MFM drives (legacy).",
            Method::Custom(i) => with_custom(*i, |r| r.description).unwrap_or("Custom method."),
        }
    }
}
