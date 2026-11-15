//! The interface's typeface: Manrope, which CrossFX ships, or the Gilroy the user licensed.
//!
//! ## Manrope, fitted to Gilroy's measurements
//!
//! The original FxSound for Windows sets its window in Gilroy, Radomir Tinkov's commercial
//! typeface, and every box in the window was measured in it (`docs/spec/01-window-layout.md` §2.3).
//! Gilroy cannot be redistributed, so CrossFX ships Manrope instead (Mikhail Sharanda, The Manrope
//! Project Authors, SIL OFL 1.1): a geometric face close to it, but drawn larger on the same em.
//! Its x-height is 1080 units of 2000 against Gilroy's 500 of 1000, its capitals and letters a few
//! per cent wider, and its line — ascent and descent, 2732 units, 1.37 em — far taller than
//! Gilroy's 1.20. Set at the sizes the window asks for, Manrope's text runs out of the boxes
//! Gilroy's fitted, and a one-line label in a 14-point box lays out 19 points tall.
//!
//! Each face therefore comes with a [`TypeScale`], applied where the face is registered rather
//! than at each of the window's hundreds of `FontId`s:
//!
//! - **a size factor**, egui's [`FontTweak::scale`], which multiplies every size the window asks
//!   for in that face, advances and line included. Manrope's, [`MANROPE`]'s 0.93, gives it
//!   Gilroy's x-height: its text reads at the size Gilroy's did and comes out narrower on the
//!   whole — every string of the Latin and Cyrillic tables measured, 0.95 of Gilroy's width on
//!   average. Not word by word: a short word full of `t`s and `f`s, French "Effets", Czech
//!   "Zrušit", is up to 8 % wider. So no box is taken on trust: `tests/text_fits.rs` draws every
//!   window in every language and fails on a label its box cuts or clips;
//! - **a line**, the ascent and the height of a line as shares of the size asked for, written
//!   into the face's vertical metrics (`hhea`, and `OS/2`'s typographic and Windows ones) before
//!   egui reads them. Manrope's is Gilroy's own, 0.78 and 1.20: a line is as tall as it was, the
//!   baseline sits where it sat, so text stays where every box put it, and the conversion of a
//!   JUCE height to an em ([`crate::widgets::icon_button::JUCE_HEIGHT_PER_EM`]) holds for both.
//!
//! Gilroy's own scale is 1 and its own metrics, which the window was measured in. A Gilroy the
//! user supplies is never rewritten: what its licence allows is the user's business, and the
//! file is drawn as it is.
//!
//! The Noto faces behind either — Chinese, Korean, Arabic, Thai — keep their own size and line,
//! as they did beside Gilroy.
//!
//! ## Gilroy, when the user has it
//!
//! CrossFX neither ships nor downloads Gilroy. The application finds a Gilroy the user added in
//! Settings (CrossFX's own data folder) or installed on the system, reads each file with
//! [`read_face`] and hands what it found to [`set_gilroy`] as a [`GilroyFaces`]: Regular,
//! SemiBold — or Medium in its place — and Bold, all three or none. Behind Gilroy, Manrope stands
//! first in every family's fallbacks, so a letter Gilroy lacks comes from the face fitted to it;
//! and a language whose letters Gilroy does not have is set in Manrope whole
//! ([`GilroyFaces::sets`]) — the original did the same for Vietnamese, in another face.
//!
//! ## Which face is installed
//!
//! Like the language in force (`fxsound_core::i18n`), the choice is the process's: the
//! application says what the user chose ([`set_choice`]) and what it found ([`set_gilroy`]), and
//! a window shows the face [`shown`] gives: Gilroy when it is chosen, or when the original look
//! is (`Palette::is_windows`, «Like the original FxSound LLC» = Interface and above), and a whole
//! Gilroy is there for the language; Manrope otherwise, never a face that is not there.
//! [`install`] puts that face into a context ([`crate::theme::apply`] calls it), and [`sync`],
//! which the Pro and Lite windows call at the top of every frame they draw
//! ([`crate::views::pro::show`], [`crate::views::lite::show`]) and the Settings window at the
//! top of its own, installs it again the moment any of it changes. A context whose fonts were set
//! some other way, as a test sets its own, is left as it is.

use egui::{FontData, FontDefinitions, FontFamily, FontTweak};
use fxsound_core::InterfaceFont;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

/// Font family names registered with egui.
pub mod families {
    /// The semibold weight's family: labels and values, and the dialogs' normal font.
    pub const SEMIBOLD: &str = "CrossFX-SemiBold";
    /// The bold weight's family: headings and the preset name.
    pub const BOLD: &str = "CrossFX-Bold";
}

/// One of the three weights the window is set in. The original picks a face per control rather
/// than deriving a weight, so each is a family of its own ([`Weight::family`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    /// Body text: the small font, the menus, the notices.
    Regular,
    /// The normal font: labels, values, the tooltips.
    SemiBold,
    /// Headings and the pane titles.
    Bold,
}

impl Weight {
    /// The three, in the order of their files.
    pub const ALL: [Self; 3] = [Self::Regular, Self::SemiBold, Self::Bold];

    /// The family a `FontId` asks for to be set in this weight.
    #[must_use]
    pub fn family(self) -> FontFamily {
        match self {
            Self::Regular => FontFamily::Proportional,
            Self::SemiBold => FontFamily::Name(families::SEMIBOLD.into()),
            Self::Bold => FontFamily::Name(families::BOLD.into()),
        }
    }

    /// The face's own name for the weight, as a font menu writes it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Regular => "Regular",
            Self::SemiBold => "SemiBold",
            Self::Bold => "Bold",
        }
    }

    /// The name Manrope's face for the weight is registered under.
    const fn manrope_key(self) -> &'static str {
        match self {
            Self::Regular => "Manrope-Regular",
            Self::SemiBold => "Manrope-SemiBold",
            Self::Bold => "Manrope-Bold",
        }
    }

    /// The name the user's Gilroy face for the weight is registered under.
    const fn gilroy_key(self) -> &'static str {
        match self {
            Self::Regular => "Gilroy-Regular",
            Self::SemiBold => "Gilroy-SemiBold",
            Self::Bold => "Gilroy-Bold",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Regular => 0,
            Self::SemiBold => 1,
            Self::Bold => 2,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The type scale
// ---------------------------------------------------------------------------------------------

/// How one face is fitted to the window's measurements, which were taken in Gilroy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    /// What every size the window asks for is multiplied by in this face: egui's
    /// [`FontTweak::scale`], which scales the advances and the line as well as the glyphs.
    pub size: f32,
    /// The face's line, as shares of the size asked for, written into its vertical metrics; `None`
    /// keeps the face's own.
    pub line: Option<Line>,
}

/// A line of text: where its baseline sits under its top, and how tall it is, both as shares of
/// the size the window asked for. Its descent is what is left under the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub ascent: f32,
    pub height: f32,
}

/// Manrope's: Gilroy's x-height, and Gilroy's line.
///
/// 0.93 is Gilroy's x-height over Manrope's (500/1000 against 1080/2000), and the text that comes
/// out is narrower than Gilroy's was by about 6 % in Regular, 3½ % in SemiBold and 2 % in Bold —
/// on average; a short word can be wider (the module's notes). The line is Gilroy Regular's
/// (ascent 0.78, 1.20 in all), which is also the ascent plus descent
/// [`crate::widgets::icon_button::JUCE_HEIGHT_PER_EM`] converts JUCE's heights by.
pub const MANROPE: TypeScale = TypeScale {
    size: 0.93,
    line: Some(Line {
        ascent: 0.78,
        height: 1.2,
    }),
};

/// Gilroy's: as it is. The window was measured in it.
pub const GILROY: TypeScale = TypeScale {
    size: 1.0,
    line: None,
};

/// The type scale of a face.
#[must_use]
pub const fn type_scale(font: InterfaceFont) -> TypeScale {
    match font {
        InterfaceFont::Manrope => MANROPE,
        InterfaceFont::Gilroy => GILROY,
    }
}

// ---------------------------------------------------------------------------------------------
// The faces
// ---------------------------------------------------------------------------------------------

/// Manrope's three faces, embedded so the binary never depends on a font being installed.
const MANROPE_FILES: [&[u8]; 3] = [
    include_bytes!("../../../assets/fonts/Manrope-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-Bold.ttf"),
];

/// Script fallbacks, so a Chinese, Korean, Arabic or Thai interface still renders: each behind
/// both faces, at its own size and with its own line.
const NOTO: [(&str, &[u8]); 4] = [
    (
        "NotoSansSC",
        include_bytes!("../../../assets/fonts/NotoSansSC-Regular.otf"),
    ),
    (
        "NotoSansKR",
        include_bytes!("../../../assets/fonts/NotoSansKR-Regular.otf"),
    ),
    (
        "NotoSansArabic",
        include_bytes!("../../../assets/fonts/NotoSansArabic-Regular.ttf"),
    ),
    (
        "NotoSansThai",
        include_bytes!("../../../assets/fonts/NotoSansThai-Regular.ttf"),
    ),
];

/// Manrope's three faces as egui is given them ([`manrope_face`]). Built once, and shared, so
/// that installing the same face again compares as the same fonts and costs nothing.
fn manrope() -> &'static [Arc<FontData>; 3] {
    static FACES: OnceLock<[Arc<FontData>; 3]> = OnceLock::new();
    FACES.get_or_init(|| MANROPE_FILES.map(|file| Arc::new(manrope_face(file))))
}

/// The Noto faces, built once.
fn noto() -> &'static [Arc<FontData>; 4] {
    static FACES: OnceLock<[Arc<FontData>; 4]> = OnceLock::new();
    FACES.get_or_init(|| NOTO.map(|(_, file)| Arc::new(FontData::from_static(file))))
}

/// The ligatures Manrope's standard set (`liga`) makes of `--`, `---`, `->`, `<-` and `<->`:
/// a dash and arrows, where a command line (`crossfx --next-preset`) and a device's or a
/// preset's name have to show the characters typed. egui shapes with every default feature on,
/// so the feature is renamed out of the shaper's way ([`without_features`]). Its `fi` and `ff`
/// go with it, as they never were in the original, whose JUCE did not shape.
const MANROPE_OFF: [[u8; 4]; 1] = [*b"liga"];

/// One of Manrope's faces as egui is given it: [`MANROPE`]'s line written into its metrics, its
/// ligatures off ([`MANROPE_OFF`]) and its size as the tweak. A file whose tables cannot be found
/// to rewrite is used as it is — drawn, if not quite in the window's measurements — and the log
/// says so.
fn manrope_face(file: &'static [u8]) -> FontData {
    let rewritten = MANROPE
        .line
        .and_then(|line| with_line(file, line, MANROPE.size))
        .and_then(|font| without_features(&font, &MANROPE_OFF));
    let data = match rewritten {
        Some(font) => FontData::from_owned(font),
        None => {
            log::warn!("Manrope's metrics could not be rewritten; using the face as it is");
            FontData::from_static(file)
        }
    };
    data.tweak(FontTweak {
        scale: MANROPE.size,
        ..FontTweak::default()
    })
}

/// `font` with its vertical metrics replaced by `line`, for a face drawn at `size` of the sizes
/// asked for: `hhea`'s ascender, descender and line gap, and `OS/2`'s typographic and Windows
/// ones, whichever of them a reader takes (skrifa, as FreeType, takes the typographic ones when
/// `USE_TYPO_METRICS` is set and `hhea`'s otherwise). `None` when the font's tables are not where
/// its directory says.
///
/// The bytes are the font's own otherwise: no outline, advance or name changes, and the table
/// checksums are left as they were, which no reader of a font in memory checks.
#[must_use]
pub fn with_line(font: &[u8], line: Line, size: f32) -> Option<Vec<u8>> {
    let units = f32::from(read_u16(font, table(font, *b"head")?.start + 18)?);
    if units <= 0.0 || size <= 0.0 {
        return None;
    }
    let ascent = (line.ascent / size * units).round();
    let descent = ((line.height - line.ascent) / size * units).round();
    if !(0.0..=f32::from(i16::MAX)).contains(&ascent)
        || !(0.0..=f32::from(i16::MAX)).contains(&descent)
    {
        return None;
    }
    let (ascent, descent) = (ascent as i16, descent as i16);
    let mut out = font.to_vec();
    let hhea = table(font, *b"hhea")?;
    if hhea.len() < 10 {
        return None;
    }
    write_i16(&mut out, hhea.start + 4, ascent)?;
    write_i16(&mut out, hhea.start + 6, -descent)?;
    write_i16(&mut out, hhea.start + 8, 0)?;
    if let Some(os2) = table(font, *b"OS/2").filter(|os2| os2.len() >= 78) {
        write_i16(&mut out, os2.start + 68, ascent)?;
        write_i16(&mut out, os2.start + 70, -descent)?;
        write_i16(&mut out, os2.start + 72, 0)?;
        write_i16(&mut out, os2.start + 74, ascent)?;
        write_i16(&mut out, os2.start + 76, descent)?;
    }
    Some(out)
}

/// `font` with the `GSUB` features tagged as in `tags` renamed to their upper-case spelling,
/// which names no feature a shaper applies by itself — egui's applies every default feature, and
/// offers no way to turn one off. `None` when the font's `GSUB` is not where its directory says;
/// a font without one has nothing to rename.
#[must_use]
pub fn without_features(font: &[u8], tags: &[[u8; 4]]) -> Option<Vec<u8>> {
    let mut out = font.to_vec();
    let Some(gsub) = table(font, *b"GSUB") else {
        return Some(out);
    };
    let list = gsub.start + usize::from(read_u16(font, gsub.start + 6)?);
    let count = usize::from(read_u16(font, list)?);
    for index in 0..count {
        let record = list + 2 + 6 * index;
        let tag: [u8; 4] = font.get(record..record + 4)?.try_into().ok()?;
        if tags.contains(&tag) {
            out.get_mut(record..record + 4)?
                .copy_from_slice(&tag.map(|c| c.to_ascii_uppercase()));
        }
    }
    Some(out)
}

/// Where the table tagged `tag` lies in `font`, from the font's table directory.
fn table(font: &[u8], tag: [u8; 4]) -> Option<std::ops::Range<usize>> {
    let count = usize::from(read_u16(font, 4)?);
    for index in 0..count {
        let record = 12 + 16 * index;
        if font.get(record..record + 4)? != tag {
            continue;
        }
        let start = usize::try_from(read_u32(font, record + 8)?).ok()?;
        let length = usize::try_from(read_u32(font, record + 12)?).ok()?;
        let end = start.checked_add(length)?;
        return (end <= font.len()).then_some(start..end);
    }
    None
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn write_i16(bytes: &mut [u8], at: usize, value: i16) -> Option<()> {
    bytes
        .get_mut(at..at + 2)?
        .copy_from_slice(&value.to_be_bytes());
    Some(())
}

// ---------------------------------------------------------------------------------------------
// Reading a Gilroy the user supplied
// ---------------------------------------------------------------------------------------------

/// Which of Gilroy's faces a file is, of the four the window can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GilroyFace {
    Regular,
    /// Stands in for SemiBold when there is no SemiBold.
    Medium,
    SemiBold,
    Bold,
}

/// What a font file the user offered turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaceReading {
    /// One of the Gilroy faces the window is set in.
    Gilroy(GilroyFace),
    /// A Gilroy face the window has no use for: an italic, or a weight other than Regular,
    /// Medium, SemiBold and Bold.
    OtherGilroy,
    /// A face of another family, by the family's name.
    OtherFamily(String),
    /// Not a font that can be read: another kind of file, or a collection.
    NotAFont,
}

/// What `bytes` are, by the font's name table and its `OS/2` weight and style.
///
/// A Gilroy is a face whose family — the typographic one when the font has it, its family name
/// otherwise — is Gilroy, or a name that starts with it ("Gilroy Semibold", the older four-styles
/// grouping). Its face is the typographic subfamily, or what the family name adds to "Gilroy",
/// or its subfamily, read as a weight's name; failing that, its weight class.
#[must_use]
pub fn read_face(bytes: &[u8]) -> FaceReading {
    use skrifa::MetadataProvider as _;
    use skrifa::attribute::Style;
    use skrifa::string::StringId;

    let Ok(font) = skrifa::FontRef::new(bytes) else {
        return FaceReading::NotAFont;
    };
    let name = |id: StringId| {
        font.localized_strings(id)
            .english_or_first()
            .map(|name| name.chars().collect::<String>().trim().to_owned())
            .filter(|name| !name.is_empty())
    };
    let Some(family) =
        name(StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(StringId::FAMILY_NAME))
    else {
        return FaceReading::NotAFont;
    };
    let Some(rest) = gilroy_suffix(&family) else {
        return FaceReading::OtherFamily(family);
    };
    let attributes = font.attributes();
    if !matches!(attributes.style, Style::Normal) {
        return FaceReading::OtherGilroy;
    }
    let style = name(StringId::TYPOGRAPHIC_SUBFAMILY_NAME)
        .or_else(|| (!rest.is_empty()).then(|| rest.to_owned()))
        .or_else(|| name(StringId::SUBFAMILY_NAME))
        .unwrap_or_default();
    let named = face_by_name(&style);
    let weighed = || match attributes.weight.value().round() as i32 {
        400 => Some(GilroyFace::Regular),
        500 => Some(GilroyFace::Medium),
        600 => Some(GilroyFace::SemiBold),
        700 => Some(GilroyFace::Bold),
        _ => None,
    };
    match named {
        Ok(Some(face)) => FaceReading::Gilroy(face),
        // A name that is a weight the window does not use, or an italic one.
        Err(()) => FaceReading::OtherGilroy,
        Ok(None) => weighed().map_or(FaceReading::OtherGilroy, FaceReading::Gilroy),
    }
}

/// What `family` adds to "Gilroy" when it is Gilroy's — empty for Gilroy itself — or `None` for
/// another family.
fn gilroy_suffix(family: &str) -> Option<&str> {
    let head = family.get(..6)?;
    if !head.eq_ignore_ascii_case("gilroy") {
        return None;
    }
    let rest = &family[6..];
    match rest.chars().next() {
        None => Some(""),
        Some(' ' | '-' | '_') => Some(rest[1..].trim()),
        Some(_) => None,
    }
}

/// A face by its style's name: `Ok(Some)` for one the window uses, `Err` for a name that is a
/// face it does not (Light, ExtraBold, an italic…), `Ok(None)` for a name that says nothing.
fn face_by_name(style: &str) -> Result<Option<GilroyFace>, ()> {
    let key: String = style
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect();
    if key.contains("italic") || key.contains("oblique") {
        return Err(());
    }
    match key.as_str() {
        "" => Ok(None),
        "regular" | "normal" | "roman" => Ok(Some(GilroyFace::Regular)),
        "medium" => Ok(Some(GilroyFace::Medium)),
        "semibold" | "demibold" => Ok(Some(GilroyFace::SemiBold)),
        "bold" => Ok(Some(GilroyFace::Bold)),
        "thin" | "hairline" | "ultralight" | "extralight" | "light" | "extrabold" | "ultrabold"
        | "heavy" | "black" => Err(()),
        _ => Ok(None),
    }
}

/// A whole Gilroy: a face for each of the three weights, from the files the application found.
///
/// Cheap to clone; the faces themselves are shared.
#[derive(Clone)]
pub struct GilroyFaces {
    faces: [Arc<FontData>; 3],
    /// Whether Medium stands in for SemiBold.
    pub medium_for_semibold: bool,
    /// Which languages' letters it has, as [`GilroyFaces::sets`] found them, by code.
    coverage: Arc<Mutex<Vec<(&'static str, bool)>>>,
}

impl std::fmt::Debug for GilroyFaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GilroyFaces")
            .field("medium_for_semibold", &self.medium_for_semibold)
            .finish_non_exhaustive()
    }
}

impl GilroyFaces {
    /// The faces for the three weights from `files`, each the file's bytes and what
    /// [`read_face`] read it as, in the order they are preferred: for each weight the first file
    /// that is it — SemiBold before Medium wherever both are. The weights nothing was found for
    /// otherwise.
    pub fn assemble(
        files: impl IntoIterator<Item = (GilroyFace, Vec<u8>)>,
    ) -> Result<Self, Vec<Weight>> {
        let mut regular = None;
        let mut semibold = None;
        let mut medium = None;
        let mut bold = None;
        for (face, bytes) in files {
            let slot = match face {
                GilroyFace::Regular => &mut regular,
                GilroyFace::Medium => &mut medium,
                GilroyFace::SemiBold => &mut semibold,
                GilroyFace::Bold => &mut bold,
            };
            if slot.is_none() {
                *slot = Some(bytes);
            }
        }
        let medium_for_semibold = semibold.is_none() && medium.is_some();
        let semibold = semibold.or(medium);
        match (regular, semibold, bold) {
            (Some(regular), Some(semibold), Some(bold)) => Ok(Self {
                faces: [regular, semibold, bold].map(|bytes| {
                    Arc::new(FontData::from_owned(bytes).tweak(FontTweak {
                        scale: GILROY.size,
                        ..FontTweak::default()
                    }))
                }),
                medium_for_semibold,
                coverage: Arc::default(),
            }),
            (regular, semibold, bold) => Err(Self::missing(
                [
                    regular.map(|_| GilroyFace::Regular),
                    semibold.map(|_| GilroyFace::SemiBold),
                    bold.map(|_| GilroyFace::Bold),
                ]
                .into_iter()
                .flatten(),
            )),
        }
    }

    /// The weights `faces` leave without a face, by the rule [`GilroyFaces::assemble`] goes by —
    /// Medium stands in for a SemiBold that is not there — and none for a whole Gilroy.
    ///
    /// What a caller that has some faces asks before it reads more: the application, whose own
    /// folder holding a whole Gilroy spares it the system's font folders.
    #[must_use]
    pub fn missing(faces: impl IntoIterator<Item = GilroyFace>) -> Vec<Weight> {
        let mut has = [false; 3];
        for face in faces {
            let weight = match face {
                GilroyFace::Regular => Weight::Regular,
                GilroyFace::Medium | GilroyFace::SemiBold => Weight::SemiBold,
                GilroyFace::Bold => Weight::Bold,
            };
            has[weight.index()] = true;
        }
        Weight::ALL
            .into_iter()
            .filter(|weight| !has[weight.index()])
            .collect()
    }

    /// The face for `weight`.
    #[must_use]
    pub fn face(&self, weight: Weight) -> &Arc<FontData> {
        &self.faces[weight.index()]
    }

    /// Whether Gilroy has every letter `language`'s translation sets that Manrope has — its own,
    /// that is; a letter neither has comes from the Noto faces whichever stands first. English
    /// and an unknown code, whose text is the English keys, always.
    ///
    /// Letters only: a dash or a sign Gilroy lacks comes from Manrope behind it and is not worth
    /// a whole language in another face. Worked out once per language and remembered.
    #[must_use]
    pub fn sets(&self, language: &str) -> bool {
        let Some(language) = fxsound_core::i18n::language(language) else {
            return true;
        };
        let mut known = self.coverage.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, sets)) = known.iter().find(|(code, _)| *code == language.code) {
            return *sets;
        }
        let sets = covers(
            &letters_of(language),
            |letter| has_char(MANROPE_FILES[0], letter),
            |letter| has_char(self.faces[0].bytes(), letter),
        );
        known.push((language.code, sets));
        sets
    }
}

/// Whether a face that `has` letters sets every one of `letters` that Manrope (`manrope`) does.
fn covers(letters: &[char], manrope: impl Fn(char) -> bool, has: impl Fn(char) -> bool) -> bool {
    letters
        .iter()
        .all(|&letter| !manrope(letter) || has(letter))
}

/// Every letter `language`'s table translates into, once each.
fn letters_of(language: &fxsound_core::i18n::Language) -> Vec<char> {
    let table = fxsound_core::i18n::Catalogue::for_language(language);
    let mut letters: Vec<char> = table
        .keys()
        .filter_map(|key| table.get(key))
        .flat_map(str::chars)
        .filter(|c| c.is_alphabetic())
        .collect();
    letters.sort_unstable();
    letters.dedup();
    letters
}

/// Whether `font` maps `letter` to a glyph.
fn has_char(font: &[u8], letter: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::new(font).is_ok_and(|font| font.charmap().map(letter).is_some())
}

// ---------------------------------------------------------------------------------------------
// What is chosen, what is found, and what is shown
// ---------------------------------------------------------------------------------------------

/// The process's choice and what was found for it.
struct Registry {
    choice: InterfaceFont,
    gilroy: Option<GilroyFaces>,
    /// Moves on at every change, so that [`sync`] can tell a context's fonts are stale.
    generation: u64,
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry {
    choice: InterfaceFont::Manrope,
    gilroy: None,
    generation: 0,
});

/// Record what the user chose: Settings ▸ General ▸ "Interface font", or `settings.toml` at start.
pub fn set_choice(choice: InterfaceFont) {
    let mut registry = REGISTRY.write().unwrap_or_else(PoisonError::into_inner);
    if registry.choice != choice {
        registry.choice = choice;
        registry.generation += 1;
    }
}

/// Record the Gilroy the application found, or that it found none.
pub fn set_gilroy(gilroy: Option<GilroyFaces>) {
    let mut registry = REGISTRY.write().unwrap_or_else(PoisonError::into_inner);
    registry.gilroy = gilroy;
    registry.generation += 1;
}

/// What the user chose, as last recorded.
#[must_use]
pub fn choice() -> InterfaceFont {
    REGISTRY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .choice
}

/// The Gilroy found, if a whole one was.
#[must_use]
pub fn gilroy() -> Option<GilroyFaces> {
    REGISTRY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .gilroy
        .clone()
}

/// The face the window is set in: Gilroy when `choice` is Gilroy or the original look is asked
/// for — «Like the original FxSound LLC» = Interface and above — and `gilroy` is a whole Gilroy
/// that has `language`'s letters ([`GilroyFaces::sets`]); Manrope otherwise.
#[must_use]
pub fn shown(
    choice: InterfaceFont,
    original_look: bool,
    gilroy: Option<&GilroyFaces>,
    language: &str,
) -> InterfaceFont {
    let wanted = matches!(choice, InterfaceFont::Gilroy) || original_look;
    match gilroy {
        Some(gilroy) if wanted && gilroy.sets(language) => InterfaceFont::Gilroy,
        Some(_) | None => InterfaceFont::Manrope,
    }
}

/// The font definitions for a window set in Gilroy when it is given, in Manrope otherwise.
///
/// Each weight's family is its face, then — behind Gilroy — Manrope's, then the Noto faces; the
/// regular weight is egui's proportional family, which ends with egui's own default faces so that
/// symbols and emoji still resolve.
#[must_use]
pub fn definitions(gilroy: Option<&GilroyFaces>) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let manrope = manrope();
    for weight in Weight::ALL {
        defs.font_data.insert(
            weight.manrope_key().to_owned(),
            Arc::clone(&manrope[weight.index()]),
        );
        if let Some(gilroy) = gilroy {
            defs.font_data.insert(
                weight.gilroy_key().to_owned(),
                Arc::clone(gilroy.face(weight)),
            );
        }
    }
    for ((name, _), data) in NOTO.iter().zip(noto()) {
        defs.font_data.insert((*name).to_owned(), Arc::clone(data));
    }

    let stock = defs
        .families
        .remove(&FontFamily::Proportional)
        .unwrap_or_default();
    for weight in Weight::ALL {
        let mut chain: Vec<String> = Vec::new();
        if gilroy.is_some() {
            chain.push(weight.gilroy_key().to_owned());
        }
        chain.push(weight.manrope_key().to_owned());
        chain.extend(NOTO.iter().map(|(name, _)| (*name).to_owned()));
        if weight == Weight::Regular {
            chain.extend(stock.iter().cloned());
        }
        defs.families.insert(weight.family(), chain);
    }
    defs
}

/// The face the registry says is shown at `original_look`, in the language in force, and the
/// generation that says so.
fn resolve(original_look: bool) -> (InterfaceFont, u64, Option<GilroyFaces>) {
    let registry = REGISTRY.read().unwrap_or_else(PoisonError::into_inner);
    let wanted = matches!(registry.choice, InterfaceFont::Gilroy) || original_look;
    // The language is only asked when it can matter: a frame with Manrope chosen asks nothing.
    let face = match &registry.gilroy {
        Some(gilroy) if wanted => shown(
            registry.choice,
            original_look,
            Some(gilroy),
            &fxsound_core::i18n::current(),
        ),
        Some(_) | None => InterfaceFont::Manrope,
    };
    let gilroy = match face {
        InterfaceFont::Gilroy => registry.gilroy.clone(),
        InterfaceFont::Manrope => None,
    };
    (face, registry.generation, gilroy)
}

/// The font definitions of the face shown at `original_look`.
#[must_use]
pub fn shown_definitions(original_look: bool) -> FontDefinitions {
    let (_, _, gilroy) = resolve(original_look);
    definitions(gilroy.as_ref())
}

/// What a context's fonts were installed as, so that [`sync`] can tell when to install again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Installed {
    face: InterfaceFont,
    original_look: bool,
    generation: u64,
}

fn installed_id() -> egui::Id {
    egui::Id::unique("crossfx-typeface-installed")
}

/// Install the face shown at `original_look` on `ctx`, and remember it for [`sync`].
pub fn install(ctx: &egui::Context, original_look: bool) {
    let (face, generation, gilroy) = resolve(original_look);
    ctx.set_fonts(definitions(gilroy.as_ref()));
    ctx.data_mut(|data| {
        data.insert_temp(
            installed_id(),
            Installed {
                face,
                original_look,
                generation,
            },
        );
    });
}

/// Install the face again if what is shown has moved since [`install`] last put it on `ctx`: the
/// user's choice, a Gilroy found or added, the original look, the language. Nothing on a context
/// whose fonts were set another way.
///
/// Cheap enough for every frame: a read of the registry and, while Gilroy is shown or wanted, of
/// the language's code.
pub fn sync(ctx: &egui::Context, original_look: bool) {
    let Some(installed) = ctx.data(|data| data.get_temp::<Installed>(installed_id())) else {
        return;
    };
    let (face, generation, _) = resolve(original_look);
    let wanted = Installed {
        face,
        original_look,
        generation,
    };
    if wanted != installed {
        install(ctx, original_look);
        ctx.request_repaint();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use egui::{Color32, FontId};

    /// A context set in `definitions`.
    fn context(definitions: FontDefinitions) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions);
        ctx
    }

    /// What `f` measures in one frame of `ctx`.
    fn measure<T>(ctx: &egui::Context, f: impl FnOnce(&egui::Ui) -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        ctx.run_ui(Default::default(), |ui| {
            if let Some(f) = f.take() {
                out = Some(f(ui));
            }
        })
        .drop_without_applying_deltas();
        out.expect("the frame ran")
    }

    /// Manrope's file with every "Manrope" in its name table spelt "Gilroy " — a stand-in for a
    /// Gilroy, which this tree does not have — and its weight class and italic bit as given.
    pub(crate) fn stand_in(weight: Weight, weight_class: u16, italic: bool) -> Vec<u8> {
        let mut font = MANROPE_FILES[weight.index()].to_vec();
        let name = table(&font, *b"name").expect("a name table");
        let from: Vec<u8> = "Manrope"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        let to: Vec<u8> = "Gilroy "
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        let mut at = name.start;
        while at + from.len() <= name.end {
            if font[at..at + from.len()] == from[..] {
                font[at..at + to.len()].copy_from_slice(&to);
                at += from.len();
            } else {
                at += 1;
            }
        }
        // The Macintosh records, in one byte a letter.
        let mut at = name.start;
        while at + 7 <= name.end {
            if &font[at..at + 7] == b"Manrope" {
                font[at..at + 7].copy_from_slice(b"Gilroy ");
                at += 7;
            } else {
                at += 1;
            }
        }
        let os2 = table(&font, *b"OS/2").expect("an OS/2 table");
        font[os2.start + 4..os2.start + 6].copy_from_slice(&weight_class.to_be_bytes());
        let selection = os2.start + 62;
        let mut flags = u16::from_be_bytes([font[selection], font[selection + 1]]);
        flags = if italic {
            (flags | 1) & !0x40
        } else {
            flags & !1
        };
        font[selection..selection + 2].copy_from_slice(&flags.to_be_bytes());
        font
    }

    /// Whether `ctx`'s fonts were last installed for the original look, or `None` for a context
    /// whose fonts were set another way: what [`sync`] keeps up to date.
    pub(crate) fn installed_for_original_look(ctx: &egui::Context) -> Option<bool> {
        ctx.data(|data| data.get_temp::<Installed>(installed_id()))
            .map(|installed| installed.original_look)
    }

    /// A whole stand-in Gilroy.
    pub(crate) fn stand_in_gilroy() -> GilroyFaces {
        GilroyFaces::assemble([
            (GilroyFace::Regular, stand_in(Weight::Regular, 400, false)),
            (GilroyFace::SemiBold, stand_in(Weight::SemiBold, 600, false)),
            (GilroyFace::Bold, stand_in(Weight::Bold, 700, false)),
        ])
        .expect("three weights")
    }

    #[test]
    fn manropes_line_is_written_into_its_metrics_and_its_size_is_the_tweak() {
        use skrifa::MetadataProvider as _;
        for (weight, file) in Weight::ALL.into_iter().zip(MANROPE_FILES) {
            let line = MANROPE.line.expect("Manrope has a line");
            let rewritten = with_line(file, line, MANROPE.size).expect("rewritten");
            assert_eq!(rewritten.len(), file.len(), "{weight:?}");
            let font = skrifa::FontRef::new(&rewritten).expect("still a font");
            let metrics = font.metrics(
                skrifa::instance::Size::unscaled(),
                skrifa::instance::LocationRef::default(),
            );
            let em = f32::from(metrics.units_per_em);
            let ascent = metrics.ascent / em * MANROPE.size;
            let height = (metrics.ascent - metrics.descent + metrics.leading) / em * MANROPE.size;
            assert!((ascent - line.ascent).abs() < 1e-3, "{weight:?}: {ascent}");
            assert!((height - line.height).abs() < 1e-3, "{weight:?}: {height}");
            // Nothing else moved: the glyphs, their advances and the names are the file's own.
            let original = skrifa::FontRef::new(file).expect("a font");
            assert_eq!(
                font.charmap().map('ж'),
                original.charmap().map('ж'),
                "{weight:?}"
            );
            assert_eq!(font.attributes(), original.attributes(), "{weight:?}");
        }
        for face in manrope() {
            assert!((face.tweak.scale - MANROPE.size).abs() < 1e-6);
        }
    }

    #[test]
    fn a_line_of_manrope_is_as_tall_as_gilroys_and_its_baseline_where_gilroys_was() {
        // At the size the window asks for, a line is 1.2 of it whatever the weight, and the
        // baseline 0.78 of it under the line's top: what JUCE_HEIGHT_PER_EM converts by.
        let ctx = context(definitions(None));
        for weight in Weight::ALL {
            for size in [10.0_f32, 14.0, 17.0, 40.0] {
                let (height, baseline) = measure(&ctx, |ui| {
                    let galley = ui.painter().layout_no_wrap(
                        "Hxg".to_owned(),
                        FontId::new(size, weight.family()),
                        Color32::WHITE,
                    );
                    let row = &galley.rows[0];
                    (galley.size().y, row.glyphs[0].pos.y)
                });
                // egui rounds a face's ascent and descent to the pixel grid, a point at most here.
                assert!(
                    (height - 1.2 * size).abs() <= 1.0,
                    "{weight:?} {size}: {height}"
                );
                assert!(
                    (baseline - 0.78 * size).abs() <= 1.0,
                    "{weight:?} {size}: {baseline}"
                );
            }
        }
    }

    #[test]
    fn a_command_line_is_drawn_as_typed_without_manropes_dash_and_arrow_ligatures() {
        // As Manrope ships, `liga` draws `--` as one dash and `->` as an arrow; the shortcut
        // table's `crossfx --toggle-power` has to read as it is typed.
        let glyphs = |defs: FontDefinitions, text: &str| {
            measure(&context(defs), |ui| {
                let galley = ui.painter().layout_no_wrap(
                    text.to_owned(),
                    FontId::new(20.0, Weight::SemiBold.family()),
                    Color32::WHITE,
                );
                galley.rows[0]
                    .glyphs
                    .iter()
                    .filter(|glyph| glyph.uv_rect.size.x > 0.0)
                    .count()
            })
        };
        let mut shipped = FontDefinitions::empty();
        shipped.font_data.insert(
            "shipped".to_owned(),
            Arc::new(FontData::from_static(MANROPE_FILES[1])),
        );
        shipped
            .families
            .insert(Weight::SemiBold.family(), vec!["shipped".to_owned()]);
        for text in ["--toggle-power", "->", "<->", "a---b"] {
            let ink = text.chars().count();
            assert!(
                glyphs(shipped.clone(), text) < ink,
                "{text}: the shipped face ligates"
            );
            assert_eq!(glyphs(definitions(None), text), ink, "{text}");
        }
        // Renaming touches the feature's tag and nothing else.
        let renamed = without_features(MANROPE_FILES[1], &MANROPE_OFF).expect("a GSUB");
        assert_eq!(renamed.len(), MANROPE_FILES[1].len());
        let differing = renamed
            .iter()
            .zip(MANROPE_FILES[1])
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differing, 4, "the four letters of one tag");
    }

    #[test]
    fn the_size_factor_scales_manropes_advances_as_well_as_its_glyphs() {
        // egui's FontTweak::scale reaches the layout, which is what the type scale relies on.
        let mut plain = FontDefinitions::empty();
        plain.font_data.insert(
            "plain".to_owned(),
            Arc::new(FontData::from_static(MANROPE_FILES[1])),
        );
        plain
            .families
            .insert(FontFamily::Proportional, vec!["plain".to_owned()]);
        let width = |defs: FontDefinitions, family: FontFamily| {
            measure(&context(defs), |ui| {
                ui.painter()
                    .layout_no_wrap(
                        "Dynamic Boost".to_owned(),
                        FontId::new(17.0, family),
                        Color32::WHITE,
                    )
                    .size()
                    .x
            })
        };
        let own = width(plain, FontFamily::Proportional);
        let scaled = width(definitions(None), Weight::SemiBold.family());
        assert!(
            (scaled / own - MANROPE.size).abs() < 0.01,
            "{scaled} against {own}"
        );
    }

    #[test]
    fn a_gilroy_is_read_by_its_family_and_its_face_and_anything_else_is_refused() {
        assert_eq!(
            read_face(&stand_in(Weight::Regular, 400, false)),
            FaceReading::Gilroy(GilroyFace::Regular)
        );
        assert_eq!(
            read_face(&stand_in(Weight::SemiBold, 600, false)),
            FaceReading::Gilroy(GilroyFace::SemiBold)
        );
        assert_eq!(
            read_face(&stand_in(Weight::Bold, 700, false)),
            FaceReading::Gilroy(GilroyFace::Bold)
        );
        // An italic is a Gilroy the window does not use.
        assert_eq!(
            read_face(&stand_in(Weight::Regular, 400, true)),
            FaceReading::OtherGilroy
        );
        // Manrope itself is another family.
        assert_eq!(
            read_face(MANROPE_FILES[0]),
            FaceReading::OtherFamily("Manrope".to_owned())
        );
        assert_eq!(read_face(b"not a font at all"), FaceReading::NotAFont);
        assert_eq!(read_face(&[]), FaceReading::NotAFont);
    }

    #[test]
    fn a_style_name_says_the_face_and_a_weight_class_says_it_when_the_name_does_not() {
        assert_eq!(face_by_name("Semi Bold"), Ok(Some(GilroyFace::SemiBold)));
        assert_eq!(face_by_name("SemiBold"), Ok(Some(GilroyFace::SemiBold)));
        assert_eq!(face_by_name("semibold"), Ok(Some(GilroyFace::SemiBold)));
        assert_eq!(face_by_name("Medium"), Ok(Some(GilroyFace::Medium)));
        assert_eq!(face_by_name("Regular"), Ok(Some(GilroyFace::Regular)));
        assert_eq!(face_by_name("Bold"), Ok(Some(GilroyFace::Bold)));
        for unused in [
            "ExtraBold",
            "Light",
            "Heavy",
            "Bold Italic",
            "Black",
            "Thin",
        ] {
            assert_eq!(face_by_name(unused), Err(()), "{unused}");
        }
        assert_eq!(face_by_name(""), Ok(None));
        assert_eq!(face_by_name("Book"), Ok(None));
        assert_eq!(gilroy_suffix("Gilroy"), Some(""));
        assert_eq!(gilroy_suffix("gilroy semibold"), Some("semibold"));
        assert_eq!(gilroy_suffix("Gilroy-Medium"), Some("Medium"));
        assert_eq!(gilroy_suffix("Gilroyal"), None);
        assert_eq!(gilroy_suffix("Manrope"), None);
        assert_eq!(gilroy_suffix("Gil"), None);
    }

    #[test]
    fn a_whole_gilroy_needs_regular_bold_and_semibold_or_medium_in_its_place() {
        let face = |face, weight| (face, stand_in(weight, 400, false));
        // SemiBold is preferred to Medium, whatever the order they come in.
        let whole = GilroyFaces::assemble([
            face(GilroyFace::Medium, Weight::SemiBold),
            face(GilroyFace::Regular, Weight::Regular),
            face(GilroyFace::SemiBold, Weight::SemiBold),
            face(GilroyFace::Bold, Weight::Bold),
        ])
        .expect("whole");
        assert!(!whole.medium_for_semibold);
        let medium = GilroyFaces::assemble([
            face(GilroyFace::Regular, Weight::Regular),
            face(GilroyFace::Medium, Weight::SemiBold),
            face(GilroyFace::Bold, Weight::Bold),
        ])
        .expect("Medium stands in");
        assert!(medium.medium_for_semibold);
        assert_eq!(
            GilroyFaces::assemble([face(GilroyFace::Regular, Weight::Regular)]).err(),
            Some(vec![Weight::SemiBold, Weight::Bold])
        );
        assert_eq!(
            GilroyFaces::assemble(std::iter::empty()).err(),
            Some(Weight::ALL.to_vec())
        );
    }

    #[test]
    fn what_some_faces_leave_missing_is_what_assembling_them_would_say() {
        // The application asks `missing` of the faces in its own folder before it lists the
        // system's: the two must never disagree on what a whole Gilroy is.
        let bytes = stand_in(Weight::Regular, 400, false);
        let kinds = [
            GilroyFace::Regular,
            GilroyFace::Medium,
            GilroyFace::SemiBold,
            GilroyFace::Bold,
        ];
        for subset in 0..1_u8 << kinds.len() {
            let faces: Vec<GilroyFace> = kinds
                .iter()
                .enumerate()
                .filter(|(i, _)| subset & (1 << i) != 0)
                .map(|(_, face)| *face)
                .collect();
            let assembled = GilroyFaces::assemble(faces.iter().map(|face| (*face, bytes.clone())));
            assert_eq!(
                GilroyFaces::missing(faces.iter().copied()),
                assembled.err().unwrap_or_default(),
                "{faces:?}"
            );
        }
        assert!(
            GilroyFaces::missing([GilroyFace::Bold, GilroyFace::Medium, GilroyFace::Regular])
                .is_empty()
        );
    }

    #[test]
    fn gilroy_is_shown_only_when_it_is_wanted_whole_and_has_the_languages_letters() {
        let gilroy = stand_in_gilroy();
        use InterfaceFont::{Gilroy, Manrope};
        assert_eq!(shown(Gilroy, false, Some(&gilroy), "ru"), Gilroy);
        // The original look asks for it whatever was chosen…
        assert_eq!(shown(Manrope, true, Some(&gilroy), "de"), Gilroy);
        // …and neither shows a Gilroy that is not there.
        assert_eq!(shown(Gilroy, true, None, "en"), Manrope);
        assert_eq!(shown(Manrope, false, Some(&gilroy), "en"), Manrope);
        // The stand-in is Manrope underneath and has every letter Manrope has.
        for language in &fxsound_core::i18n::LANGUAGES {
            assert!(gilroy.sets(language.code), "{}", language.code);
        }
    }

    #[test]
    fn a_gilroy_without_a_languages_letters_leaves_that_language_in_manrope() {
        let manrope = |letter| has_char(MANROPE_FILES[0], letter);
        // Vietnamese's letters are Manrope's, so a Gilroy with Latin and Cyrillic only — the one
        // the original embedded — leaves Vietnamese in Manrope and keeps Russian.
        let vietnamese = letters_of(fxsound_core::i18n::language("vi").expect("vi"));
        assert!(vietnamese.contains(&'ế'), "{vietnamese:?}");
        assert!(manrope('ế') && manrope('ж') && manrope('ğ'));
        let latin_and_cyrillic =
            |letter: char| letter.is_ascii() || ('\u{400}'..='\u{4ff}').contains(&letter);
        assert!(!covers(&vietnamese, manrope, latin_and_cyrillic));
        let russian = letters_of(fxsound_core::i18n::language("ru").expect("ru"));
        assert!(covers(&russian, manrope, latin_and_cyrillic));
        // Thai's letters are in neither face — the Noto face sets them whichever stands first —
        // so they are no reason to leave Gilroy.
        let thai = letters_of(fxsound_core::i18n::language("th").expect("th"));
        assert!(thai.iter().any(|&letter| !manrope(letter)));
        let latin = |letter: char| letter.is_ascii();
        assert!(covers(
            &thai
                .iter()
                .copied()
                .filter(|&letter| !manrope(letter))
                .collect::<Vec<_>>(),
            manrope,
            latin
        ));
        // And a file that is no font has no letters.
        assert!(!has_char(b"", 'a'));
    }

    #[test]
    fn each_weights_family_is_its_face_then_manrope_then_the_scripts() {
        let manrope = definitions(None);
        let gilroy = definitions(Some(&stand_in_gilroy()));
        for weight in Weight::ALL {
            let chain = &manrope.families[&weight.family()];
            assert_eq!(chain[0], weight.manrope_key(), "{weight:?}");
            assert_eq!(chain[1], "NotoSansSC", "{weight:?}");
            let chain = &gilroy.families[&weight.family()];
            assert_eq!(chain[0], weight.gilroy_key(), "{weight:?}");
            assert_eq!(chain[1], weight.manrope_key(), "{weight:?}");
            assert!(
                chain.iter().any(|name| name == "NotoSansThai"),
                "{weight:?}"
            );
            for name in chain {
                assert!(
                    manrope.font_data.contains_key(name) || gilroy.font_data.contains_key(name),
                    "{name} is in a chain and nowhere else"
                );
            }
        }
        // egui's own faces end the proportional family only, for symbols and emoji.
        let stock = FontDefinitions::default();
        let last = stock.families[&FontFamily::Proportional]
            .last()
            .expect("egui has faces");
        assert_eq!(
            manrope.families[&FontFamily::Proportional].last(),
            Some(last)
        );
        assert!(
            !manrope
                .font_data
                .keys()
                .any(|key| key.starts_with("Gilroy"))
        );
        // The same face installed twice is the same fonts: no atlas is rebuilt for it.
        assert!(definitions(None) == manrope);
    }

    #[test]
    fn a_context_whose_fonts_were_set_another_way_is_left_alone_by_sync() {
        let mut own = FontDefinitions::default();
        own.families
            .insert(Weight::SemiBold.family(), vec!["Ubuntu-Light".to_owned()]);
        let ctx = context(own);
        sync(&ctx, true);
        assert!(
            ctx.data(|data| data.get_temp::<Installed>(installed_id()))
                .is_none()
        );
        // A context `install` set up is remembered, and synced only when something moved.
        let ctx = egui::Context::default();
        install(&ctx, false);
        let first = ctx.data(|data| data.get_temp::<Installed>(installed_id()));
        sync(&ctx, false);
        assert_eq!(
            ctx.data(|data| data.get_temp::<Installed>(installed_id())),
            first
        );
        sync(&ctx, true);
        assert_eq!(
            ctx.data(|data| data.get_temp::<Installed>(installed_id()))
                .map(|installed| installed.original_look),
            Some(true)
        );
    }

    #[test]
    fn a_users_font_is_read_by_the_skrifa_egui_loads_it_with() {
        // `read_face` vouches for the files egui is handed. egui panics on a font it cannot
        // parse, and the release build aborts on a panic: were the two parsers of different
        // versions, a file one took and the other refused would stop CrossFX at every start.
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .expect("the workspace's Cargo.lock");
        // The skrifa a package of the lock file depends on: `skrifa`, or `skrifa 0.44.0` while
        // the tree holds more than one.
        let skrifa_of = |package: &str| -> Vec<String> {
            let name = format!("name = \"{package}\"");
            lock.split("[[package]]")
                .find(|entry| entry.lines().any(|line| line == name))
                .map(|entry| {
                    entry
                        .lines()
                        .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix("\","))
                        .filter(|dependency| {
                            *dependency == "skrifa" || dependency.starts_with("skrifa ")
                        })
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let ours = skrifa_of("fxsound-ui");
        assert_eq!(ours.len(), 1, "fxsound-ui's skrifa: {ours:?}");
        assert_eq!(
            ours,
            skrifa_of("epaint"),
            "the workspace's skrifa is not the one egui parses fonts with"
        );
    }
}
