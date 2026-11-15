//! Shared shaping scenarios for the corpus pin test and the shaping
//! bench: the fixture fonts, the sample texts, and the feature sets.

#![allow(dead_code)]

pub const DEJAVU: &[u8] = include_bytes!("../fixtures/DejaVuSans.ttf");
pub const DEJAVU_MONO: &[u8] = include_bytes!("../fixtures/DejaVuSansMono.ttf");
pub const INTER: &[u8] = include_bytes!("../fixtures/InterVariable.ttf");
pub const NOTO_ARABIC: &[u8] = include_bytes!("../fixtures/NotoSansArabic-Regular.ttf");

/// Plain Latin prose (about 620 characters).
pub const LATIN_PARAGRAPH: &str = "The afternoon ferry left the quay a little after four, \
carrying two dozen travellers, a van full of printing paper and one very patient dog. \
Nobody on deck paid attention to the weather until the first gust flattened the awning; \
by then the harbour wall was already a thin grey line. \"We will be across in fifty minutes,\" \
the mate said, without looking up from his coffee. He was wrong by an hour and a half. \
What the passengers remembered afterwards was not the wind or the spray, but the quiet \
efficiency of the crew: ropes coiled, hatches fastened, every loose crate wedged tight \
before anybody had thought to ask. Typography, like seamanship, is mostly preparation.";

/// Ligature- and kerning-dense Latin: f-ligature words, capital pairs
/// with large negative kerns, digits, fractions and punctuation runs
/// that trigger contextual alternates.
pub const LIGA_KERN_HEAVY: &str = "AVATAR WAVE Type To Ty Yo. LT AY VA \"A\" 'T' office \
waffle affluent fjord shuffle offline baffling efficient fifty-five flfi ffl ffi fi fl ff \
-> --> => <- <-> 1/2 3/4 15/16 10:30 3x4 2x2 (A) [B] {C} A-B i-j @hello T. V, W; Y: P. \
AT&T WWW. L'A f\u{00ef} f\u{00ec} Te Ta Tr Vo We Ya r. r, 7. 11:45 Av Aw Ay FA PA TA \
office waffle fjord AVATAR Tofu Yawl Waffle Kafka r\u{00e9}sum\u{00e9} na\u{00ef}ve";

/// Latin text carrying combining marks (base + one or two marks), so
/// mark-to-base and mark-to-mark attachment run.
pub const LATIN_COMBINING: &str = "a\u{0301}e\u{0300}o\u{0308}\u{0304} u\u{0302}\u{0301} \
n\u{0303} A\u{030a} E\u{0301}\u{0323} i\u{0308} x\u{0307}\u{0323} c\u{0327}\u{0301} \
O\u{0303}\u{0304} q\u{0323} fi\u{0301} T\u{0308}o";

/// Greek and Cyrillic prose for the non-Latin simple scripts.
pub const GREEK_CYRILLIC: &str = "\u{0397} \u{03b3}\u{03c1}\u{03b1}\u{03c6}\u{03ae} \
\u{03b5}\u{03af}\u{03bd}\u{03b1}\u{03b9} \u{03c4}\u{03ad}\u{03c7}\u{03bd}\u{03b7}. \
\u{0393}\u{03a5} \u{03a4}\u{03b1} \u{0391}\u{03a5}\u{0391}. \
\u{0428}\u{0440}\u{0438}\u{0444}\u{0442} \u{0413}\u{0410}\u{0422}\u{0410} \
\u{0443}\u{0434}\u{043e}\u{0431}\u{0435}\u{043d} \u{0434}\u{043b}\u{044f} \
\u{0447}\u{0442}\u{0435}\u{043d}\u{0438}\u{044f}, \u{0422}\u{0443}\u{0420}\u{0443}.";

/// Vocalised Arabic prose: letters with harakat (fatha, damma, kasra,
/// shadda, sukun, tanwin), lam-alef sequences and Arabic punctuation.
pub const ARABIC_PARAGRAPH: &str = "\u{0627}\u{0644}\u{0644}\u{0651}\u{064f}\u{063a}\u{064e}\u{0629}\u{064f} \
\u{0627}\u{0644}\u{0639}\u{064e}\u{0631}\u{064e}\u{0628}\u{0650}\u{064a}\u{0651}\u{064e}\u{0629}\u{064f} \
\u{062c}\u{064e}\u{0645}\u{0650}\u{064a}\u{0644}\u{064e}\u{0629}\u{064c} \
\u{0648}\u{064e}\u{063a}\u{064e}\u{0646}\u{0650}\u{064a}\u{0651}\u{064e}\u{0629}\u{064c} \
\u{0628}\u{0650}\u{0627}\u{0644}\u{0645}\u{064f}\u{0641}\u{0652}\u{0631}\u{064e}\u{062f}\u{064e}\u{0627}\u{062a}\u{0650}\u{060c} \
\u{0648}\u{064e}\u{0627}\u{0644}\u{062e}\u{064e}\u{0637}\u{0651}\u{064f} \
\u{0641}\u{064e}\u{0646}\u{0651}\u{064c} \u{0644}\u{0627} \u{064a}\u{064e}\u{0646}\u{0652}\u{062a}\u{064e}\u{0647}\u{0650}\u{064a}. \
\u{0643}\u{062a}\u{0628} \u{0627}\u{0644}\u{0637}\u{0627}\u{0644}\u{0628} \u{0631}\u{0633}\u{0627}\u{0644}\u{0629} \
\u{0637}\u{0648}\u{064a}\u{0644}\u{0629} \u{0625}\u{0644}\u{0649} \u{0635}\u{062f}\u{064a}\u{0642}\u{0647} \
\u{0641}\u{064a} \u{0627}\u{0644}\u{0645}\u{062f}\u{064a}\u{0646}\u{0629} \u{0627}\u{0644}\u{0628}\u{0639}\u{064a}\u{062f}\u{0629}\u{061b} \
\u{0644}\u{0623}\u{0646}\u{0651}\u{064e} \u{0627}\u{0644}\u{0628}\u{064e}\u{062d}\u{0652}\u{0631}\u{064e} \
\u{0643}\u{064e}\u{0627}\u{0646}\u{064e} \u{0647}\u{064e}\u{0627}\u{062f}\u{0650}\u{0626}\u{064b}\u{0627} \
\u{0648}\u{0627}\u{0644}\u{0633}\u{0651}\u{064e}\u{0645}\u{064e}\u{0627}\u{0621}\u{064f} \u{0635}\u{064e}\u{0627}\u{0641}\u{0650}\u{064a}\u{064e}\u{0629}\u{064c}.";

/// Pointed Hebrew (letters with niqqud) for the DejaVu `hebr` lookups.
pub const HEBREW_POINTED: &str = "\u{05e9}\u{05c1}\u{05b8}\u{05dc}\u{05d5}\u{05b9}\u{05dd} \
\u{05e2}\u{05d5}\u{05b9}\u{05dc}\u{05b8}\u{05dd} \u{05d1}\u{05bc}\u{05b0}\u{05e8}\u{05b5}\u{05d0}\u{05e9}\u{05c1}\u{05b4}\u{05d9}\u{05ea} \
\u{05d0}\u{05b8}\u{05dc}\u{05b6}\u{05e3} \u{05dc}\u{05b9}\u{05d0}";

pub type Tag = [u8; 4];

/// Everyday Latin feature set.
pub const LATN_DEFAULT: &[Tag] = &[*b"ccmp", *b"liga", *b"calt", *b"kern", *b"mark", *b"mkmk"];
/// Latin with the discretionary / stylistic features switched on too.
pub const LATN_RICH: &[Tag] = &[
    *b"ccmp", *b"liga", *b"calt", *b"dlig", *b"hlig", *b"frac", *b"case", *b"ss01", *b"ss03",
    *b"cv05", *b"tnum", *b"zero", *b"salt", *b"aalt", *b"smcp", *b"c2sc", *b"ordn", *b"sups",
    *b"cpsp", *b"kern", *b"mark", *b"mkmk",
];
/// Everyday Arabic feature set.
pub const ARAB_DEFAULT: &[Tag] = &[
    *b"ccmp", *b"init", *b"medi", *b"fina", *b"rlig", *b"liga", *b"calt", *b"curs", *b"kern",
    *b"mark", *b"mkmk",
];
/// Arabic without the positional-form features (isolated letters), with
/// the discretionary ligatures on.
pub const ARAB_NO_FORMS: &[Tag] = &[
    *b"ccmp", *b"rlig", *b"liga", *b"dlig", *b"kern", *b"mark", *b"mkmk",
];
/// Hebrew feature set.
pub const HEBR_DEFAULT: &[Tag] = &[*b"ccmp", *b"hlig", *b"kern", *b"mark", *b"salt"];
/// Positioning only.
pub const KERN_ONLY: &[Tag] = &[*b"kern"];
