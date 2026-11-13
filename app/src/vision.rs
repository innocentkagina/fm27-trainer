//! Leest het Tactical Effectiveness "Breakdown"-scherm van FM27 uit beeld.
//!
//! FM27 is een Unity-app zonder toegankelijke UI-elementen, dus: venster vastleggen (xcap),
//! tekst + posities vinden met Apple's Vision-OCR, en per factor de taartmeter rechts van het
//! label uitmeten (aandeel gekleurd vs. grijs en de tint).

use anyhow::{Context, Result, anyhow, bail};
use image::RgbaImage;
use serde::Serialize;

use crate::labels::{LANGS, Lang};
use crate::model::{Component, Effectiveness, Factor};

#[derive(Clone, Debug, Serialize)]
pub struct OcrLine {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub confidence: f32,
}

impl OcrLine {
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    fn yc(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct GaugeReading {
    pub factor: Factor,
    pub score: f32,
    pub fill: f32,
    pub hue: f32,
    pub bbox: [u32; 4],
    /// de gemeten vulling past niet bij de kleur van de meter: nakijken
    pub doubt: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub gauges: Vec<GaugeReading>,
    pub components: Vec<(Component, f32)>,
    pub missing: Vec<Factor>,
    pub image_size: (u32, u32),
    /// taal van FM volgens de gevonden labels
    pub lang: &'static str,
    /// gevonden titels en labels (van de 21)
    pub labels: usize,
    /// taal van de OCR
    pub ocr: String,
    /// hoogte van de labels (mediaan), voor de tweede meting op de referentiegrootte
    pub text_height: Option<f32>,
}

impl Analysis {
    pub fn to_effectiveness(&self, source: &str) -> Effectiveness {
        let mut e = Effectiveness::default();
        e.components.clear();
        for g in &self.gauges {
            e.set(g.factor, g.score);
        }
        e.components = self.components.clone();
        e.source = source.to_string();
        e
    }
}

// ---------------------------------------------------------------- vastleggen

/// Legt het FM27-venster vast (vereist "Schermopname"-toestemming in Systeeminstellingen).
pub fn capture_fm_window() -> Result<RgbaImage> {
    let windows = xcap::Window::all().context("vensters opvragen mislukt")?;
    // Op Windows het venster van fm.exe zelf. Alleen op de titel afgaan pakte ook een browser met
    // een pagina over Football Manager open (een forum, de downloadpagina met een schermafbeelding
    // van de Breakdown): die is vaak even groot of groter dan FM.
    #[cfg(windows)]
    let fm_pid = fm27_agent::fm_pid();
    #[cfg(not(windows))]
    let fm_pid: Option<u32> = None;
    const BROWSERS: [&str; 8] = ["chrome", "firefox", "edge", "opera", "brave", "safari", "vivaldi", "browser"];
    let mut best: Option<((bool, u64), xcap::Window)> = None;
    for w in windows {
        let app = w.app_name().unwrap_or_default();
        let title = w.title().unwrap_or_default();
        let own = fm_pid.is_some() && w.pid().ok() == fm_pid;
        let named = app.contains("Football Manager") || title.contains("Football Manager");
        let browser = BROWSERS.iter().any(|b| app.to_lowercase().contains(b) || title.to_lowercase().contains(b));
        if !(own || (named && !browser)) {
            continue;
        }
        if w.is_minimized().unwrap_or(false) {
            continue;
        }
        let area = w.width().unwrap_or(0) as u64 * w.height().unwrap_or(0) as u64;
        // het venster van fm.exe gaat voor; daarna het grootste
        let key = (own, area);
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, w));
        }
    }
    let (_, w) = best.ok_or_else(|| anyhow!("{}", crate::t!("Geen Football Manager-venster gevonden. Draait FM27?", "No Football Manager window found. Is FM27 running?")))?;
    let img = w.capture_image().map_err(|e| {
        anyhow!("{} ({e})", crate::t!(
            "Vastleggen mislukt. Geef deze app toestemming bij Systeeminstellingen → Privacy en beveiliging → Schermopname.",
            "Capture failed. Allow this app under System Settings → Privacy & Security → Screen Recording."
        ))
    })?;
    if img.width() < 200 {
        bail!("{}", crate::t!("Vastgelegd beeld is leeg; staat Schermopname-toestemming aan?", "Captured image is empty; is Screen Recording permission enabled?"));
    }
    Ok(img)
}

// ---------------------------------------------------------------- OCR

/// Tekstherkenning: Apple Vision op macOS, Windows.Media.Ocr op Windows. `lang` kiest de taal van
/// de OCR (Windows; `None` = die van het gebruikersprofiel). Geeft de regels (coördinaten in pixels,
/// oorsprong linksboven) en de gebruikte taal.
pub fn ocr(img: &RgbaImage, lang: Option<&str>) -> Result<(Vec<OcrLine>, String)> {
    platform::ocr(img, lang)
}

/// De talen waarin de OCR op deze computer kan lezen (Windows: de geïnstalleerde OCR-talen).
pub fn ocr_languages() -> Vec<String> {
    platform::languages()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::OcrLine;
    use anyhow::{Context, Result, anyhow};
    use image::{ImageEncoder, RgbaImage};
    use objc2::AnyThread;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

    /// De talen die Vision kan lezen (macOS 13: Engels, Frans, Italiaans, Duits, Spaans, Portugees,
    /// Chinees, Koreaans, Japans, Russisch, Oekraïens; later ook meer).
    pub fn languages() -> Vec<String> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            match unsafe { request.supportedRecognitionLanguagesAndReturnError() } {
                Ok(list) => list.iter().map(|s| s.to_string()).collect(),
                Err(_) => Vec::new(),
            }
        })
    }

    /// Zonder taal leest Vision Engels (zijn standaard); latijnse talen gaan daar meestal ook mee.
    pub fn ocr(img: &RgbaImage, lang: Option<&str>) -> Result<(Vec<OcrLine>, String)> {
        ocr_lines(img, lang).map(|l| (l, lang.unwrap_or("en-US").to_string()))
    }

    fn ocr_lines(img: &RgbaImage, lang: Option<&str>) -> Result<Vec<OcrLine>> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
            .context("PNG")?;
        autoreleasepool(|_| {
            let data = NSData::with_bytes(&png);
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::new());
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            request.setUsesLanguageCorrection(false);
            if let Some(tag) = lang {
                request.setRecognitionLanguages(&NSArray::from_retained_slice(&[NSString::from_str(tag)]));
            }
            let as_request: Retained<VNRequest> = Retained::into_super(Retained::into_super(request.clone()));
            let requests = NSArray::from_retained_slice(&[as_request]);
            handler
                .performRequests_error(&requests)
                .map_err(|e| anyhow!("Vision OCR: {}", e.localizedDescription()))?;
            let mut out = Vec::new();
            if let Some(results) = request.results() {
                for obs in results.iter() {
                    let cands = obs.topCandidates(1);
                    let Some(best) = cands.firstObject() else { continue };
                    let bb = unsafe { obs.boundingBox() };
                    out.push(OcrLine {
                        text: best.string().to_string(),
                        x0: bb.origin.x as f32 * w,
                        x1: (bb.origin.x + bb.size.width) as f32 * w,
                        y0: (1.0 - (bb.origin.y + bb.size.height) as f32) * h,
                        y1: (1.0 - bb.origin.y as f32) * h,
                        confidence: best.confidence(),
                    });
                }
            }
            Ok(out)
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::OcrLine;
    use anyhow::{Result, anyhow};
    use image::RgbaImage;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::core::HSTRING;

    fn init() {
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
    }

    pub fn languages() -> Vec<String> {
        init();
        let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else { return Vec::new() };
        list.into_iter().filter_map(|l| l.LanguageTag().ok()).map(|t| t.to_string()).collect()
    }

    /// De OCR in de gevraagde taal, anders die van het gebruikersprofiel, anders de eerste
    /// geïnstalleerde. Windows-OCR bestaat niet voor elke taal (bv. Thai): staat de profieltaal
    /// daarom niet in de lijst, dan kan Engels (of een andere geïnstalleerde taal) het toch lezen.
    fn engine(lang: Option<&str>) -> Result<OcrEngine> {
        if let Some(tag) = lang {
            let l = Language::CreateLanguage(&HSTRING::from(tag))?;
            return OcrEngine::TryCreateFromLanguage(&l).map_err(|e| anyhow!("Windows OCR ({tag}): {e}"));
        }
        if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
            return Ok(e);
        }
        for tag in languages() {
            if let Ok(e) = Language::CreateLanguage(&HSTRING::from(tag.as_str())).and_then(|l| OcrEngine::TryCreateFromLanguage(&l)) {
                return Ok(e);
            }
        }
        Err(anyhow!(
            "{}",
            crate::t!(
                "Windows heeft geen tekstherkenning (OCR) geïnstalleerd. Voeg Engels toe: Instellingen → Tijd en taal → Taal en regio → Een taal toevoegen → English, met 'Optische tekenherkenning'.",
                "Windows has no text recognition (OCR) installed. Add English: Settings → Time & language → Language & region → Add a language → English, with 'Optical character recognition'."
            )
        ))
    }

    pub fn ocr(img: &RgbaImage, lang: Option<&str>) -> Result<(Vec<OcrLine>, String)> {
        init();
        let engine = engine(lang)?;
        let tag = engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default();
        let mut bgra = img.as_raw().clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let buffer = writer.DetachBuffer()?;
        let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, img.width() as i32, img.height() as i32)?;
        let result = engine.RecognizeAsync(&bmp)?.join()?;
        let mut out = Vec::new();
        for line in result.Lines()? {
            let text = line.Text()?.to_string();
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0f32, 0f32);
            for w in line.Words()? {
                let r = w.BoundingRect()?;
                x0 = x0.min(r.X);
                y0 = y0.min(r.Y);
                x1 = x1.max(r.X + r.Width);
                y1 = y1.max(r.Y + r.Height);
            }
            if x1 > x0 {
                out.push(OcrLine { text, x0, y0, x1, y1, confidence: 1.0 });
            }
        }
        Ok((out, tag))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::OcrLine;
    pub fn languages() -> Vec<String> {
        Vec::new()
    }
    pub fn ocr(_img: &image::RgbaImage, _lang: Option<&str>) -> anyhow::Result<(Vec<OcrLine>, String)> {
        anyhow::bail!("OCR is only available on macOS and Windows")
    }
}

// ---------------------------------------------------------------- matching
//
// FM27 kan in 20 talen draaien; de labels van elke taal staan in `labels.rs` (uit FM's eigen
// taalbestanden). Een opname wordt met elke taal vergeleken, en de taal met de meeste treffers telt.

/// Accenten weg (é → e, ß → ss, ł → l): een OCR in een andere taal dan die van FM leest ze vaak
/// zonder, en zo kost dat geen fout.
fn fold(c: char, out: &mut Vec<char>) {
    let base = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ł' => 'l',
        'ś' | 'š' | 'ş' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        'ğ' => 'g',
        'ß' => {
            out.extend(['s', 's']);
            return;
        }
        'æ' => {
            out.extend(['a', 'e']);
            return;
        }
        'œ' => {
            out.extend(['o', 'e']);
            return;
        }
        c => c,
    };
    out.push(base);
}

/// Tekst als reeks kleine letters en cijfers (spaties, leestekens en iconen eruit), met bij elk teken
/// de plek in de oorspronkelijke tekst. Zo maakt het niet uit of de OCR een spatie mist of toevoegt
/// (Chinees, Japans en Koreaans hebben er geen), en de plek waar een label eindigt klopt toch.
struct Letters {
    chars: Vec<char>,
    at: Vec<usize>,
    /// aantal tekens van de oorspronkelijke tekst
    n: usize,
}

fn letters(s: &str) -> Letters {
    let mut chars = Vec::new();
    let mut at = Vec::new();
    let mut n = 0;
    for (i, c) in s.chars().enumerate() {
        n = i + 1;
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                let before = chars.len();
                fold(l, &mut chars);
                at.extend(std::iter::repeat_n(i, chars.len() - before));
            }
        }
    }
    Letters { chars, at, n }
}

/// Hoeveel letters een label mag verschillen: op een klein scherm leest de OCR er nogal eens een
/// verkeerd ("Pressing IntensiW", "Tacticd Complexity", "Player Buy-ln").
fn allowed(len: usize) -> usize {
    if len >= 7 { len / 6 } else { 0 }
}

/// Beste plek van `want` als aaneengesloten stuk van `text` (Levenshtein, het begin en eind in
/// `text` vrij): (afstand, begin, eind), met `text[begin..eind]` het gevonden stuk.
fn find_in(text: &[char], want: &[char]) -> (usize, usize, usize) {
    let n = text.len();
    // per plek: (kosten, begin van het stuk)
    let mut prev: Vec<(usize, usize)> = (0..=n).map(|j| (0, j)).collect();
    let mut cur = vec![(0usize, 0usize); n + 1];
    for (i, w) in want.iter().enumerate() {
        cur[0] = (i + 1, 0);
        for j in 1..=n {
            let sub = (prev[j - 1].0 + usize::from(text[j - 1] != *w), prev[j - 1].1);
            let skip_label = (prev[j].0 + 1, prev[j].1);
            let skip_text = (cur[j - 1].0 + 1, cur[j - 1].1);
            cur[j] = if sub.0 <= skip_label.0 && sub.0 <= skip_text.0 {
                sub
            } else if skip_label.0 <= skip_text.0 {
                skip_label
            } else {
                skip_text
            };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    // laagste kosten; bij gelijke kosten het langste stuk
    (0..=n).map(|j| (prev[j].0, prev[j].1, j)).min_by_key(|(d, b, e)| (*d, usize::MAX - (e - b))).unwrap_or((want.len(), 0, 0))
}

/// Een label in een OCR-regel: de afstand, het gevonden stuk, en de x waar het label eindigt.
#[derive(Clone, Copy, Debug)]
struct Found {
    dist: usize,
    begin: usize,
    end: usize,
    /// tekens van de regel die niet bij het label horen (iconen die de OCR als letters ziet, ander tekst)
    extra: usize,
    x_end: f32,
    /// tekens (van de oorspronkelijke tekst) tot en met het label, en of er daarna nog iets komt
    end_char: usize,
    trailing: bool,
}

impl Found {
    fn score(&self) -> i32 {
        1000 - 100 * self.dist as i32 - self.extra as i32
    }
}

fn find_label_in(line: &OcrLine, text: &Letters, want: &[char], slack: usize) -> Option<Found> {
    let max = allowed(want.len()) + slack;
    if want.is_empty() || text.chars.len() + max < want.len() {
        return None;
    }
    let (dist, begin, end) = find_in(&text.chars, want);
    // Het label moet minstens de helft van de regel zijn: zonder spaties staat "Player Capability"
    // anders ook in de beschrijving erboven ("a player's capability and their ...").
    if dist > max || end == 0 || text.chars.len() > 2 * want.len() + 4 {
        return None;
    }
    let end_char = text.at[end - 1] + 1;
    let x_end = line.x0 + (line.x1 - line.x0) * end_char as f32 / text.n.max(1) as f32;
    let trailing = text.n > end_char;
    Some(Found { dist, begin, end, extra: text.chars.len() - (end - begin), x_end, end_char, trailing })
}

/// Wat er van één taal in de opname staat.
struct Matched<'a> {
    lang: &'static Lang,
    titles: Vec<(Component, &'a OcrLine)>,
    labels: Vec<(Factor, &'a OcrLine, f32)>,
    /// de koppen "Key Factors"
    heads: Vec<&'a OcrLine>,
}

impl Matched<'_> {
    fn count(&self) -> usize {
        self.titles.len() + self.labels.len()
    }
}

fn match_lang<'a>(lines: &'a [OcrLine], texts: &[Letters], lang: &'static Lang) -> Matched<'a> {
    let want = |s: &str| letters(s).chars;
    // Titels eerst: de hele regel is de titel (op een kort icoon na). In sommige talen heet een
    // onderdeel net als een key factor (Duits: "Spielerfähigkeit" is Player Ability én Player
    // Capability); de titel staat bovenaan het paneel, dus bij gelijke stand de bovenste regel.
    let mut titles: Vec<(Component, &OcrLine)> = Vec::new();
    let mut used = vec![false; lines.len()];
    for (c, label) in Component::ALL.iter().zip(lang.components) {
        let w = want(label);
        let best = (0..lines.len())
            .filter(|i| !used[*i])
            .filter_map(|i| find_label_in(&lines[i], &texts[i], &w, 0).map(|f| (i, f)))
            .filter(|(i, f)| f.extra <= 3 && f.end == texts[*i].chars.len())
            .min_by(|(i, a), (j, b)| a.dist.cmp(&b.dist).then(lines[*i].y0.total_cmp(&lines[*j].y0)));
        if let Some((i, _)) = best {
            used[i] = true;
            titles.push((*c, &lines[i]));
        }
    }
    // Key factors: elke combinatie van label en regel, de beste eerst. Een regel kan twee labels
    // bevatten (de OCR plakt soms twee kolommen aan elkaar), maar een stuk tekst hoort maar bij één
    // label: zo pakt "Ικανότητα Παίκτη" niet het stuk uit "Τακτική Ικανότητα Παίκτη".
    let mut cands: Vec<(i32, usize, usize, Found)> = Vec::new();
    for (k, label) in lang.factors.iter().enumerate() {
        let w = want(label);
        for i in 0..lines.len() {
            if used[i] {
                continue;
            }
            if let Some(f) = find_label_in(&lines[i], &texts[i], &w, 0) {
                cands.push((f.score(), k, i, f));
            }
        }
    }
    cands.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut spans: Vec<Vec<(usize, usize)>> = vec![Vec::new(); lines.len()];
    let mut done = [false; 17];
    let mut picked: Vec<(usize, usize, Found)> = Vec::new();
    for (_, k, i, f) in cands {
        if done[k] || spans[i].iter().any(|(b, e)| f.begin < *e && *b < f.end) {
            continue;
        }
        done[k] = true;
        spans[i].push((f.begin, f.end));
        picked.push((k, i, f));
    }
    // Waar het label eindigt, naar rato van de tekens in de regel. Staat er achter het label nog
    // iets in dezelfde regel, dan is dat vaak de taartmeter zelf, die de OCR als "O" of "'" leest
    // ("...rivalesO"): die is veel breder dan een letter, en het einde zou dan ín de meter vallen.
    // Dan telt de gemiddelde letterbreedte van de regels zonder zo'n staart.
    let mut widths: Vec<f32> = picked.iter().filter(|(_, _, f)| !f.trailing).map(|(_, i, f)| (lines[*i].x1 - lines[*i].x0) / f.end_char.max(1) as f32).collect();
    widths.sort_by(|a, b| a.total_cmp(b));
    let cw = widths.get(widths.len() / 2).copied();
    let labels = picked
        .into_iter()
        .map(|(k, i, f)| {
            let x_end = match cw {
                Some(cw) if f.trailing => f.x_end.min(lines[i].x0 + cw * f.end_char as f32),
                _ => f.x_end,
            };
            (Factor::ALL[k], &lines[i], x_end)
        })
        .collect();
    let kf = want(lang.key_factors);
    let heads = (0..lines.len())
        .filter(|i| find_label_in(&lines[*i], &texts[*i], &kf, 1).is_some_and(|f| f.extra <= 2))
        .map(|i| &lines[i])
        .collect();
    Matched { lang, titles, labels, heads }
}

/// De taal van FM in deze opname: die met de meeste titels en labels (Engels bij gelijke stand).
fn best_match(lines: &[OcrLine]) -> Matched<'_> {
    let texts: Vec<Letters> = lines.iter().map(|l| letters(&l.text)).collect();
    let mut best: Option<Matched> = None;
    for lang in LANGS {
        let m = match_lang(lines, &texts, lang);
        if best.as_ref().is_none_or(|b| m.count() > b.count()) {
            best = Some(m);
        }
    }
    best.expect("LANGS is niet leeg")
}

/// De kolommen van de Breakdown, elk met zijn key factors van boven naar beneden. Een label dat de
/// OCR mist, krijgt zijn plek uit de rij-afstand en de meter-kolom van de gevonden labels.
const COLUMNS: &[&[Factor]] = &[
    &[
        Factor::InPossessionShape,
        Factor::PassingStyle,
        Factor::BuildUp,
        Factor::ChanceCreation,
        Factor::OutOfPossessionShape,
        Factor::LineOfEngagement,
        Factor::PressingIntensity,
        Factor::PressingTrap,
    ],
    &[Factor::PlayerTacticalAbility, Factor::TacticalComplexity],
    &[Factor::PlayerCapability, Factor::PlayerRoleSuitability],
    &[Factor::ManagerialSupport, Factor::TeammatesAndStaffRelationships, Factor::ManagerTendencies],
    &[Factor::QualityOfNextOppositionReport, Factor::OppositionPlayerKnowledge],
];

// ---------------------------------------------------------------- pixels

fn hsv(p: &image::Rgba<u8>) -> (f32, f32, f32) {
    let r = p[0] as f32 / 255.0;
    let g = p[1] as f32 / 255.0;
    let b = p[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, max)
}

/// Gekleurd deel van een meter: verzadigd, helder, tint rood…groen.
fn is_colored(p: &image::Rgba<u8>) -> bool {
    let (h, s, v) = hsv(p);
    // na het (bilineair) herschalen van een kleine meter zijn de kleuren iets flauwer
    s >= 0.36 && v >= 0.42 && (h <= 160.0 || h >= 345.0)
}

/// Gekleurd, en duidelijk lichter dan de achtergrond `bg`: de vulling van een meter is fel. Een
/// paneel in een sterke clubkleur (groen, geel) kan zelf "gekleurd" zijn, maar is veel donkerder.
fn is_colored_on(p: &image::Rgba<u8>, bg: f32) -> bool {
    is_colored(p) && hsv(p).2 >= bg + 0.2
}

/// Middelgrijs (leeg) deel van een meter, zoals eerder op macOS gemeten. Let op: de lichtere
/// achtergrond van een gemarkeerde rij in de Breakdown valt hier ook onder.
fn is_gauge_grey(p: &image::Rgba<u8>) -> bool {
    let (_, s, v) = hsv(p);
    s < 0.16 && (0.30..=0.66).contains(&v)
}

/// Bijna wit: het lege deel van een taartmeter zoals FM27 hem (op Windows gemeten) tekent. Nooit
/// achtergrond. "Licht" is ten opzichte van de achtergrond `bg` (helderheid): op een donkerdere
/// opname (bv. met HDR aan) is het lege deel maar ~0,6, op een opname met opgetilde zwarttinten
/// is de gemarkeerde rij zelf al 0,6. Op een gewone opname (achtergrond ~0,3, leeg deel ~0,85)
/// ligt de grens rond 0,55.
fn is_light_on(p: &image::Rgba<u8>, bg: f32) -> bool {
    let (_, s, v) = hsv(p);
    s < 0.25 && v > (bg + 0.25).clamp(0.35, 0.9)
}

/// De helderheid van de achtergrond in een band (mediaan, elke tweede pixel).
fn band_bg(img: &RgbaImage, xs: u32, xe: u32, y0: u32, y1: u32) -> f32 {
    let mut vs: Vec<f32> = (xs..xe.min(img.width())).step_by(2).flat_map(|x| (y0..y1.min(img.height())).step_by(2).map(move |y| (x, y))).map(|(x, y)| hsv(img.get_pixel(x, y)).2).collect();
    vs.sort_by(|a, b| a.total_cmp(b));
    vs.get(vs.len() / 2).copied().unwrap_or(0.3)
}

type Pred = dyn Fn(&image::Rgba<u8>) -> bool;

fn column_hits(img: &RgbaImage, x: u32, y0: u32, y1: u32, part: &Pred) -> u32 {
    (y0..y1).filter(|&y| part(img.get_pixel(x, y))).count() as u32
}

/// Zoekt de eerste taartmeter rechts van `x_start` binnen de rij rond `yc`.
///
/// Eerst streng: een meter bestaat uit gekleurde en bijna witte pixels (de lege segmenten), die
/// nooit in de achtergrond voorkomen. Lukt dat niet (oudere weergave met middelgrijze lege
/// segmenten), dan zoals voorheen met grijs; de achtergrond van een gemarkeerde rij geeft dan een
/// veel te lange reeks en valt af.
fn find_pie(img: &RgbaImage, x_start: f32, x_limit: f32, yc: f32, th: f32) -> Option<GaugeShape> {
    find_pies(img, x_start, x_limit, yc, th).into_iter().next()
}

/// De laatste taartmeter op de rij (voor een rij waarvan het label niet gelezen is: het label staat
/// links, de meter is het laatste ronde blok vóór de volgende kolom).
fn find_pie_last(img: &RgbaImage, x_start: f32, x_limit: f32, yc: f32, th: f32) -> Option<GaugeShape> {
    find_pies(img, x_start, x_limit, yc, th).into_iter().last()
}

/// Alle taartmeters tussen `x_start` en `x_limit` op de rij rond `yc`, van links naar rechts.
fn find_pies(img: &RgbaImage, x_start: f32, x_limit: f32, yc: f32, th: f32) -> Vec<GaugeShape> {
    let band = th.max(6.0);
    let bg = band_bg(img, x_start.max(0.0) as u32, x_limit.max(0.0) as u32, (yc - band).max(0.0) as u32, (yc + band).max(0.0) as u32);
    let light = move |p: &image::Rgba<u8>| is_light_on(p, bg);
    let colored = move |p: &image::Rgba<u8>| is_colored_on(p, bg);
    let strict: Box<Pred> = Box::new(move |p| colored(p) || light(p));
    let loose: Box<Pred> = Box::new(move |p| colored(p) || is_gauge_grey(p));
    let v = find_pies_with(img, x_start, x_limit, yc, th, &*strict, &colored, &light);
    if !v.is_empty() {
        return v;
    }
    find_pies_with(img, x_start, x_limit, yc, th, &*loose, &colored, &is_gauge_grey)
}

/// Kleine gaten (de donkere spaken tussen de taartpunten) breken een meter niet op.
fn find_pies_with(img: &RgbaImage, x_start: f32, x_limit: f32, yc: f32, th: f32, part: &Pred, colored: &Pred, empty: &Pred) -> Vec<GaugeShape> {
    let (w, h) = (img.width(), img.height());
    let band = (th * 1.0).max(6.0);
    let y0 = (yc - band).max(0.0) as u32;
    let y1 = ((yc + band) as u32).min(h);
    let xs = x_start.max(0.0) as u32;
    let xe = (x_limit as u32).min(w);
    let need = ((th * 0.30) as u32).max(2);
    let min_run = ((th * 0.55) as u32).max(3);
    let max_run = (th * 3.2) as u32 + 4;
    let max_gap = ((th * 0.15) as u32).max(2);

    let mut out = Vec::new();
    let mut run: Option<(u32, u32)> = None; // (begin, laatste treffer)
    let mut finish = |run: Option<(u32, u32)>, out: &mut Vec<GaugeShape>| {
        if let Some((rs, last)) = run {
            let len = last + 1 - rs;
            if len >= min_run && len <= max_run {
                if let Some(g) = measure_pie(img, rs, last + 1, y0, y1, yc, colored, empty) {
                    out.push(g);
                }
            }
        }
    };
    for x in xs..xe {
        if column_hits(img, x, y0, y1, part) >= need {
            run = Some(match run {
                Some((rs, last)) if x - last <= max_gap + 1 => (rs, x),
                other => {
                    finish(other, &mut out);
                    (x, x)
                }
            });
        }
    }
    finish(run, &mut out);
    out
}

#[derive(Clone, Debug)]
struct GaugeShape {
    fill: f32,
    hue: f32,
    bbox: [u32; 4],
}

/// Meet een taartmeter: FM vult hem met de klok mee vanaf 12 uur, in 10 segmenten. De vulling
/// wordt per hoeksector bepaald (36 sectoren van 10°), zodat de spaken tussen de punten niet
/// meetellen; `empty` herkent de lege segmenten.
fn measure_pie(img: &RgbaImage, xa: u32, xb: u32, y0: u32, y1: u32, yc: f32, colored: &Pred, empty: &Pred) -> Option<GaugeShape> {
    // Verticale grenzen van de meter binnen de kolommen: de aaneengesloten reeks rijen rond het
    // midden van het label. Valt er in de zoekband ook een stuk van de meter erboven of eronder
    // (grotere tekst in FM, kleinere rijafstand), dan hoort dat er niet bij.
    let has: Vec<bool> = (y0..y1)
        .map(|y| {
            (xa..xb).any(|x| {
                let p = img.get_pixel(x, y);
                colored(p) || empty(p)
            })
        })
        .collect();
    let rows: Vec<usize> = (0..has.len()).filter(|i| has[*i]).collect();
    let mid = (yc - y0 as f32).max(0.0);
    let start = *rows.iter().min_by(|a, b| (**a as f32 - mid).abs().total_cmp(&(**b as f32 - mid).abs()))?;
    let (mut lo, mut hi) = (start, start);
    while lo > 0 && (has[lo - 1] || (lo > 1 && has[lo - 2])) {
        lo -= 1;
    }
    while hi + 1 < has.len() && (has[hi + 1] || (hi + 2 < has.len() && has[hi + 2])) {
        hi += 1;
    }
    let (ty0, ty1) = (y0 + lo as u32, y0 + hi as u32);
    // een taart is rond: een blok dat veel breder dan hoog is (of andersom) is iets anders
    let (bw, bh) = ((xb - xa) as f32, (ty1 - ty0 + 1) as f32);
    if bw > bh * 1.6 || bh > bw * 1.6 {
        return None;
    }
    // Iets breder dan hoog: het label links raakt de meter (een lang label, grote tekst) en de
    // laatste letters horen bij de reeks. De meter staat rechts van zijn label, dus de rechterkant
    // klopt: zo breed als hij hoog is. Alleen als de linkerrand niet op de rechter lijkt: de rand
    // van een ronde meter is aan beide kanten laag, letters zijn hoog.
    let extent = |x: u32| {
        let ys: Vec<u32> = (ty0..=ty1).filter(|y| {
            let p = img.get_pixel(x, *y);
            colored(p) || empty(p)
        }).collect();
        ys.last().map(|l| l + 1 - ys[0]).unwrap_or(0) as f32
    };
    // de buitenste kolommen links en rechts paarsgewijs: bij een (eventueel uitgerekte) ronde
    // meter even hoog, met een letter ertegenaan links veel hoger
    let lopsided = bw > bh * 1.12 && (0..2.min((xb - xa) / 2)).any(|j| extent(xa + j) > extent(xb - 1 - j) + bh * 0.15);
    if std::env::var_os("FM27_VISION_DEBUG").is_some() && bw > bh * 1.12 {
        eprintln!("taart {xa}..{xb} × {ty0}..{ty1}: breder dan hoog, randen links {:?} rechts {:?}, scheef {lopsided}", [extent(xa), extent(xa + 1)], [extent(xb - 1), extent(xb - 2)]);
    }
    let xa = if lopsided { xb - bh.round() as u32 } else { xa };
    // Altijd op een vaste maat meten: op een klein scherm (1366×768, of 1080p met een kleine
    // UI-schaal) heeft een meter maar 12–18 px doorsnede, en dan hebben veel hoeksectoren geen
    // enkele pixel. Vergroten (bilineair) voegt geen informatie toe, maar maakt de telling per
    // sector wel betrouwbaar; de drempels hieronder gelden dan op elke resolutie.
    const SIZE: f32 = 72.0;
    let (sw, sh) = (xb - xa, ty1 - ty0 + 1);
    // De helderheid van de achtergrond links en rechts van de meter (op dezelfde rijen): een pixel
    // "hoort bij iets" als hij gekleurd is of duidelijk lichter dan die achtergrond. Vast ingesteld
    // (v ≥ 0,45) zou dat misgaan op een opname met lichtere achtergrond (HDR, een lichte clubkleur).
    let bg_v = {
        let mut vs: Vec<f32> = Vec::new();
        for y in ty0..=ty1.min(img.height() - 1) {
            for x in xa.saturating_sub(sw)..xa.saturating_sub(1) {
                vs.push(hsv(img.get_pixel(x, y)).2);
            }
            for x in (xb + 1)..(xb + sw).min(img.width()) {
                vs.push(hsv(img.get_pixel(x, y)).2);
            }
        }
        vs.sort_by(|a, b| a.total_cmp(b));
        vs.get(vs.len() / 2).copied().unwrap_or(0.3)
    };
    let stands_out = |p: &image::Rgba<u8>| colored(p) || hsv(p).2 >= bg_v + 0.18;
    let k = SIZE / sw.max(sh) as f32;
    let sub = image::imageops::crop_imm(img, xa, ty0, sw, sh).to_image();
    let pie = image::imageops::resize(&sub, ((sw as f32 * k).round() as u32).max(1), ((sh as f32 * k).round() as u32).max(1), image::imageops::FilterType::Triangle);
    let (pw, ph) = (pie.width(), pie.height());
    let cx = (pw as f32 - 1.0) / 2.0;
    let cy = (ph as f32 - 1.0) / 2.0;
    let r = pw.max(ph) as f32 / 2.0;

    const BINS: usize = 36;
    // (sector, fel gekleurd, tint, binnenring) per pixel van de ring tussen het midden en de rand.
    // Ook zwakker gekleurde pixels: op een flets vastgelegde opname (HDR) is een deel van de vulling
    // niet verzadigd genoeg voor `colored`; ze tellen mee als ze de tint van de vulling hebben.
    let mut px: Vec<(usize, bool, f32, bool)> = Vec::new();
    let tinted = |p: &image::Rgba<u8>| {
        let (h, s, v) = hsv(p);
        s >= 0.2 && v >= bg_v + 0.2 && (h <= 160.0 || h >= 345.0)
    };
    let mut gry = [0u32; BINS];
    // alle pixels van de ring per sector, en hoeveel daarvan bij een taart passen (gekleurd of licht)
    let mut ring = [0u32; BINS];
    let (mut inner, mut solid) = (0u32, 0u32);
    let (mut corner, mut corner_solid) = (0u32, 0u32);
    for y in 0..ph {
        for x in 0..pw {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > r * 1.2 {
                // de hoeken van het kader, buiten de cirkel
                corner += 1;
                if stands_out(pie.get_pixel(x, y)) {
                    corner_solid += 1;
                }
            }
            if d < r * 0.25 || d > r * 1.05 {
                continue; // midden (alle spaken komen samen) en rand overslaan
            }
            // hoek met de klok mee vanaf 12 uur
            let mut a = dx.atan2(-dy).to_degrees();
            if a < 0.0 {
                a += 360.0;
            }
            let bin = ((a / 360.0 * BINS as f32) as usize).min(BINS - 1);
            let p = pie.get_pixel(x, y);
            let is_fill = colored(p);
            if d <= r * 0.85 {
                ring[bin] += 1;
                inner += 1;
                if stands_out(p) {
                    solid += 1;
                }
            }
            if is_fill {
                px.push((bin, true, hsv(p).0, d < r * 0.85));
            } else if tinted(p) {
                px.push((bin, false, hsv(p).0, d < r * 0.85));
            } else if empty(p) {
                gry[bin] += 1;
            }
        }
    }
    // Een taart is een dichte schijf: bijna elke pixel binnen de rand is gekleurd of licht. Een
    // letter van het label ernaast ("Q", "O") is maar een dunne lijn en valt hier af; anders wordt
    // hij gemeten als een lege meter (0). En een taart is rond: een icoontje (een wit vierkantje
    // vóór het label) is ook in de hoeken van zijn kader licht.
    if solid * 10 < inner * 6 || (corner >= 20 && corner_solid * 2 > corner) {
        return None;
    }
    // De vulling heeft één kleur (oranje, geel of groen, naar het niveau). Die tint komt uit het
    // binnenste van de ring, waar geen achtergrond doorheen mengt; alleen pixels in die tint tellen
    // als gevuld. Een rand die met een gekleurde achtergrond (clubkleuren) mengt, telt zo niet mee.
    let hue_dist = |a: f32, b: f32| {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d)
    };
    // de tint van de vulling alleen uit de felle pixels
    let mut hist = [0u32; 36];
    for (_, strong, h, inner) in &px {
        if *inner && *strong {
            hist[((h / 10.0) as usize).min(35)] += 1;
        }
    }
    let peak = (0..36).max_by_key(|i| hist[*i]).unwrap_or(0);
    let fill_hue = if hist[peak] == 0 {
        None
    } else {
        let (mut sx, mut sy) = (0f32, 0f32);
        for (_, strong, h, inner) in &px {
            if *inner && *strong && hue_dist(*h, peak as f32 * 10.0 + 5.0) <= 15.0 {
                sx += h.to_radians().cos();
                sy += h.to_radians().sin();
            }
        }
        Some(sy.atan2(sx).to_degrees().rem_euclid(360.0))
    };
    let mut col = [0u32; BINS];
    let (mut hx, mut hy, mut ncol) = (0f32, 0f32, 0u32);
    for (bin, _, h, _) in &px {
        if fill_hue.is_some_and(|f| hue_dist(*h, f) <= 25.0) {
            col[*bin] += 1;
            hx += h.to_radians().cos();
            hy += h.to_radians().sin();
            ncol += 1;
        } else {
            gry[*bin] += 1;
        }
    }
    let seen = (0..BINS).filter(|i| col[*i] + gry[*i] > 0).count();
    if seen < BINS / 2 {
        return None;
    }
    // De vulling loopt aaneengesloten met de klok mee vanaf 12 uur. Loop de sectoren langs: één
    // sector zonder echte kleur is een lichte spaak tussen twee punten (of het lichte kruis in het
    // midden) en breekt de reeks niet, twee op rij wel. De laatste gekleurde sector vóór die
    // onderbreking bepaalt het gevulde deel; kleur die door het vergroten over de 12-uur-grens
    // heen lekt (de sector vlak vóór 12 uur bij een niet-volle meter) telt zo niet mee.
    // Telt tegen alle pixels van de sector, niet alleen de herkende: op een donkerdere opname (bv. met
    // HDR aan) is het lege deel niet meer "licht", en dan zou de kleur die aan de rand van de
    // vulling in het eerste lege segment lekt al genoeg zijn om dat segment gevuld te noemen.
    let colored = |i: usize| {
        let total = (col[i] + gry[i]).max(ring[i]).max(1);
        col[i] >= 3 && col[i] * 4 >= total
    };
    let (mut last, mut gap) = (None, 0);
    for i in 0..BINS {
        if colored(i) {
            last = Some(i);
            gap = 0;
        } else {
            gap += 1;
            if gap >= 2 {
                break;
            }
        }
    }
    let mut fill = match last {
        Some(i) => {
            let total = (col[i] + gry[i]).max(ring[i]).max(1);
            let frac = (col[i] as f32 / total as f32).min(1.0);
            (i as f32 + frac) / BINS as f32
        }
        None => 0.0,
    };
    // 10 segmenten: de lichte spaak op de grens snoept een paar procent af, dus afronden op een
    // heel segment als de meting daar dichtbij zit
    let tenths = (fill * 10.0).round();
    if (fill * 10.0 - tenths).abs() <= 0.6 {
        fill = tenths / 10.0;
    }
    let mut hue = hy.atan2(hx).to_degrees();
    if hue < 0.0 {
        hue += 360.0;
    }
    if hue > 300.0 {
        hue = 0.0; // roodtinten rond 360° → 0
    }
    Some(GaugeShape {
        fill: fill.clamp(0.0, 1.0),
        hue: if ncol == 0 { 0.0 } else { hue },
        bbox: [xa, ty0, xb, ty1 + 1],
    })
}

/// Welk deel van een taart (0–1) bij de kleur van de vulling past. FM kleurt een meter naar zijn
/// niveau (gemeten op 2560×1440, Ajax- en Sunderland-save): 20–40 % oranje (tint ~43°), 50–70 %
/// geel (~60°), 90 % groen (~120°); 40 % kan ook geel zijn. De grenzen hier zijn ruim genomen.
fn color_band(hue: f32) -> (f32, f32) {
    if hue < 50.0 {
        (0.0, 0.5)
    } else if hue < 90.0 {
        (0.3, 0.9)
    } else {
        (0.7, 1.0)
    }
}

/// Meter-score: het gevulde deel van de taart (4 van de 10 segmenten = 40), en of dat bij de kleur
/// past. Past het niet (een oranje meter die als 70 gemeten wordt), dan is de meting fout gegaan:
/// de score wordt naar de rand van wat de kleur toelaat gezet en de meter gemarkeerd, zodat de
/// gebruiker hem nakijkt.
fn gauge_score(fill: f32, hue: f32, has_color: bool) -> (f32, bool) {
    if !has_color {
        // een lege meter komt in FM bijna niet voor; zonder kleur is het eerder iets anders dat
        // rond en licht is (een icoon, een letter): laten nakijken
        return (0.0, true);
    }
    let (lo, hi) = color_band(hue);
    let doubt = fill < lo - 0.01 || fill > hi + 0.01;
    ((fill.clamp(lo, hi) * 100.0).clamp(0.0, 100.0), doubt)
}

/// Balkmeter rechts van een onderdeel-titel: een rij streepjes (11 plekken) waarvan er één
/// gekleurd is (de stand) en één wit (een referentie). De gekleurde streep vervangt een grijs
/// streepje, dus de witte en gekleurde streep tellen mee voor de breedte van de balk.
fn read_bar(img: &RgbaImage, x_start: f32, x_limit: f32, yc: f32, th: f32) -> Option<f32> {
    let (w, h) = (img.width(), img.height());
    let y0 = (yc - th * 0.9).max(0.0) as u32;
    let y1 = ((yc + th * 0.9) as u32).min(h);
    let xs = x_start.max(0.0) as u32;
    let xe = (x_limit as u32).min(w);
    if xe <= xs + 4 || y1 <= y0 + 2 {
        return None;
    }
    let need = ((th * 0.45) as u32).max(3);
    // Wat "licht" is voor de rand van het kader: 0,45, of duidelijk lichter dan de achtergrond van
    // de band (mediaan) als die zelf al licht is (HDR, opgetilde zwarttinten); anders is alles licht
    // en valt elk streepje als rand af.
    let bg = band_bg(img, xs, xe, y0, y1);
    let light = (bg + 0.15).max(0.45);
    // Per kolom: de gekleurde streep (de stand), de helderheid van het lichtste derde van de pixels
    // (een streepje is daar een piek in, ook als het op een klein scherm uitgesmeerd is en de
    // tussenruimte niet meer echt donker wordt) en hoeveel van de kolom licht is (de rand van het
    // kader loopt over de hele hoogte, een streepje maar over een deel).
    let mut marker: Vec<u32> = Vec::new();
    let mut prof: Vec<f32> = Vec::with_capacity((xe - xs) as usize);
    let mut tall: Vec<f32> = Vec::with_capacity((xe - xs) as usize);
    for x in xs..xe {
        let mut vs: Vec<f32> = Vec::with_capacity((y1 - y0) as usize);
        let mut c = 0;
        for y in y0..y1 {
            let p = img.get_pixel(x, y);
            if is_colored_on(p, bg) {
                c += 1;
            }
            vs.push(hsv(p).2);
        }
        if c >= need {
            marker.push(x);
        }
        tall.push(vs.iter().filter(|v| **v > light).count() as f32 / vs.len() as f32);
        vs.sort_by(|a, b| b.total_cmp(a));
        let top = (vs.len() / 3).max(1);
        prof.push(vs[..top].iter().sum::<f32>() / top as f32);
    }
    // De stand is een smalle gekleurde streep. Een breed gekleurd vlak in de band (de clubkleur
    // aan de rand van het scherm, op een lichtere opname) is het niet: de eerste smalle streep telt.
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for x in marker {
        match runs.last_mut() {
            Some((_, e)) if x <= *e + 2 => *e = x,
            _ => runs.push((x, x)),
        }
    }
    let narrow = (th * 0.6).max(3.0) as u32;
    let (ma, mb) = *runs.iter().find(|(a, b)| b - a < narrow)?;
    let m = (ma + mb) as f32 / 2.0;
    // licht gladstrijken, dan de plaatselijke maxima die duidelijk boven hun omgeving uitsteken
    let n = prof.len();
    let smooth: Vec<f32> = (0..n).map(|i| (prof[i.saturating_sub(1)] + prof[i] + prof[(i + 1).min(n - 1)]) / 3.0).collect();
    let pitch_guess = (th * 0.3).max(3.0);
    let win = (pitch_guess * 0.7).round().max(1.0) as usize;
    let mut centers: Vec<f32> = Vec::new();
    let mut i = 1;
    while i + 1 < n {
        let v = smooth[i];
        if v >= smooth[i - 1] && v >= smooth[i + 1] {
            // een plateau is één piek: tot waar de waarde gelijk blijft
            let mut j = i;
            while j + 1 < n && smooth[j + 1] == v {
                j += 1;
            }
            let lo_l = smooth[i.saturating_sub(win)..i].iter().copied().fold(v, f32::min);
            let lo_r = smooth[j + 1..(j + 1 + win).min(n)].iter().copied().fold(v, f32::min);
            // een streepje beslaat ruwweg de halve hoogte van de band, de rand van het kader veel meer
            let border = tall[i..=j].iter().copied().fold(0.0, f32::max) >= 0.6;
            if v - lo_l.max(lo_r) >= 0.05 && !border {
                centers.push(xs as f32 + (i + j) as f32 / 2.0);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    centers.push(m);
    centers.sort_by(|a, b| a.total_cmp(b));
    centers.dedup_by(|a, b| (*a - *b).abs() < pitch_guess * 0.5);
    // Vanaf de marker naar buiten lopen zolang de afstand tussen streepjes regelmatig blijft: één
    // stap, of twee (één gemist streepje). De rand van het kader staat ~2,3 stappen van het eerste
    // streepje en valt zo ook af als hij niet al als rand herkend is (op een donkere opname is hij
    // niet licht genoeg); losse vlakken verderop liggen verder uit elkaar.
    let mi = centers.iter().position(|c| (*c - m).abs() < pitch_guess * 0.5)?;
    let gaps: Vec<f32> = centers.windows(2).map(|w| w[1] - w[0]).collect();
    let median = |mut v: Vec<f32>| -> Option<f32> {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied()
    };
    let chain = |typical: f32| {
        let regular = |g: f32| {
            let r = g / typical;
            (0.7..=1.3).contains(&r) || (1.75..=2.2).contains(&r)
        };
        let (mut lo_i, mut hi_i) = (mi, mi);
        while lo_i > 0 && regular(centers[lo_i] - centers[lo_i - 1]) {
            lo_i -= 1;
        }
        while hi_i + 1 < centers.len() && regular(centers[hi_i + 1] - centers[hi_i]) {
            hi_i += 1;
        }
        (lo_i, hi_i)
    };
    // De stap eerst uit alle pieken, dan nog eens uit alleen de keten rond de marker: tekst in de
    // band (een titel die de OCR niet las, zodat de band bij de rand van het paneel begint) geeft
    // pieken met een andere afstand.
    let mut typical = median(gaps.clone())?;
    let (mut lo_i, mut hi_i) = chain(typical);
    if hi_i > lo_i {
        typical = median(gaps[lo_i..hi_i].to_vec())?;
        (lo_i, hi_i) = chain(typical);
    }
    let (lo, hi) = (centers[lo_i], centers[hi_i]);
    let span = hi - lo;
    if hi_i - lo_i < 3 || typical < 2.0 || span <= 0.0 {
        return None;
    }
    if std::env::var_os("FM27_VISION_DEBUG").is_some() {
        eprintln!("balk y={yc:.0} marker={m:.1} streepjes={centers:?} keten={lo:.1}..{hi:.1} stap={typical:.2}");
    }
    // FM27 tekent 11 plekken: de stand volgt uit de plek van de gekleurde streep tussen het eerste
    // en het laatste streepje, zodat een streepje dat in de marker opgaat niet meetelt. Alleen als
    // de reeks duidelijk korter is (een deel van de balk in ruis), telt de stap vanaf het eerste.
    let slots = (span / typical).round() as usize + 1;
    let idx = if (9..=12).contains(&slots) {
        (m - lo) / span * 10.0
    } else {
        (m - lo) / typical
    };
    Some((idx.round().max(0.0) / 10.0 * 100.0).clamp(0.0, 100.0))
}

// ---------------------------------------------------------------- analyse

/// De tekstgrootte waarop de meting is afgesteld: de labels van FM27 op 2560×1440 met 100 % schaal.
const REF_TEXT: f32 = 20.0;

/// Leest de Breakdown. Op een ander scherm (of een andere UI-schaal in FM) is alles groter of
/// kleiner, en op een klein scherm leest de OCR slechter en zijn de meters te klein om te meten:
/// dan nog eens op een kopie die naar de referentiegrootte is geschaald, en het beste resultaat
/// telt. Zonder één herkend label worden twee vergrotingen geprobeerd.
///
/// Leest de OCR in de taal van het profiel bijna niets van de Breakdown (FM in het Japans terwijl
/// Windows Engels leest, of andersom), dan nog eens in elke andere geïnstalleerde OCR-taal.
pub fn analyze_image(img: &RgbaImage) -> Result<(Analysis, Vec<OcrLine>)> {
    let mut best = analyze_in(img, None)?;
    if best.0.labels < 6 {
        for tag in ocr_languages() {
            if tag.eq_ignore_ascii_case(&best.0.ocr) {
                continue;
            }
            let Ok(other) = analyze_in(img, Some(&tag)) else { continue };
            if (other.0.gauges.len(), other.0.labels) > (best.0.gauges.len(), best.0.labels) {
                best = other;
            }
        }
    }
    Ok(best)
}

fn analyze_in(img: &RgbaImage, lang: Option<&str>) -> Result<(Analysis, Vec<OcrLine>)> {
    let (lines, tag) = ocr(img, lang)?;
    let mut first = analyze_with_lines(img, &lines);
    first.ocr = tag.clone();
    let mut best = (first, lines);
    let scales: Vec<f32> = match best.0.text_height {
        Some(th) if (th - REF_TEXT).abs() / REF_TEXT > 0.12 => vec![REF_TEXT / th],
        Some(_) => Vec::new(),
        None => vec![1.5, 2.0],
    };
    for k in scales {
        let (w, h) = ((img.width() as f32 * k).round() as u32, (img.height() as f32 * k).round() as u32);
        if w < 64 || h < 64 || w > 10_000 || h > 10_000 {
            continue;
        }
        let big = image::imageops::resize(img, w, h, image::imageops::FilterType::CatmullRom);
        let (lines2, _) = ocr(&big, Some(tag.as_str()).filter(|t| !t.is_empty()))?;
        let mut a2 = analyze_with_lines(&big, &lines2);
        a2.ocr = tag.clone();
        a2.text_height = a2.text_height.map(|t| t / k);
        // terug naar de maten van de opname
        for g in &mut a2.gauges {
            g.bbox = g.bbox.map(|v| (v as f32 / k).round() as u32);
        }
        a2.image_size = (img.width(), img.height());
        let lines2: Vec<OcrLine> = lines2.into_iter().map(|l| OcrLine { x0: l.x0 / k, y0: l.y0 / k, x1: l.x1 / k, y1: l.y1 / k, ..l }).collect();
        // op de referentiegrootte zijn de drempels afgesteld: die meting wint bij gelijke stand; een
        // meting die nagekeken moet worden (kleur past niet, of 0) telt niet als gelezen
        let score = |a: &Analysis| (a.gauges.iter().filter(|g| !g.doubt).count() + a.components.len(), a.gauges.len());
        let better = score(&a2) >= score(&best.0);
        if better {
            best = (a2, lines2);
        }
    }
    Ok(best)
}

pub fn analyze_with_lines(img: &RgbaImage, lines: &[OcrLine]) -> Analysis {
    let w = img.width() as f32;
    let mut gauges = Vec::new();
    let mut missing = Vec::new();

    // de taal van FM en waar de titels, labels en koppen staan
    let m = best_match(lines);
    let label_hits = &m.labels;
    let text_height = {
        let mut hs: Vec<f32> = label_hits.iter().map(|(_, l, _)| l.height()).collect();
        hs.sort_by(|a, b| a.total_cmp(b));
        hs.get(hs.len() / 2).copied()
    };

    for f in Factor::ALL {
        let Some((_, line, label_end)) = label_hits.iter().find(|(x, _, _)| *x == f).cloned()
        else {
            missing.push(f);
            continue;
        };
        let th = line.height().max(4.0);
        let next_label_x = label_hits
            .iter()
            .filter(|(_, l, _)| (l.yc() - line.yc()).abs() < th && l.x0 > label_end + th)
            .map(|(_, l, _)| l.x0 - th * 0.3)
            .fold(w, f32::min);
        let limit = next_label_x.min(label_end + th * 22.0);
        if std::env::var_os("FM27_VISION_DEBUG").is_some() {
            eprintln!("label {:<34} regel {:?} eind {label_end:.0} zoek {:.0}..{limit:.0} y {:.0} th {th:.0}", f.label(), line.text, label_end + th * 0.4, line.yc());
        }
        match find_pie(img, label_end + th * 0.4, limit, line.yc(), th) {
            Some(g) => {
                let (score, doubt) = gauge_score(g.fill, g.hue, g.fill > 0.0);
                gauges.push(GaugeReading { factor: f, score, fill: g.fill, hue: g.hue, bbox: g.bbox, doubt });
            }
            None => missing.push(f),
        }
    }

    // Labels die de OCR niet (goed) las: de rijen van een kolom liggen even ver uit elkaar en de
    // meters van een kolom staan recht onder elkaar, dus uit de gevonden rijen volgt waar de
    // andere moeten zitten. De meter wordt dan alleen in die smalle kolom gezocht.
    let row = |f: Factor| -> Option<(usize, usize)> { COLUMNS.iter().enumerate().find_map(|(c, col)| col.iter().position(|x| *x == f).map(|i| (c, i))) };
    let found: Vec<(Factor, f32, f32, [u32; 4])> = gauges
        .iter()
        .filter_map(|g| label_hits.iter().find(|(f, _, _)| *f == g.factor).map(|(_, l, _)| (g.factor, l.yc(), l.height().max(4.0), g.bbox)))
        .collect();
    let mut pitches: Vec<f32> = Vec::new();
    for a in &found {
        for b in &found {
            if let (Some((ca, ia)), Some((cb, ib))) = (row(a.0), row(b.0)) {
                if ca == cb && ib > ia {
                    pitches.push((b.1 - a.1) / (ib - ia) as f32);
                }
            }
        }
    }
    pitches.sort_by(|a, b| a.total_cmp(b));
    let pitch = pitches.get(pitches.len() / 2).copied();

    // De kop "Key Factors" boven elke kolom: voor een kolom waarvan geen enkel label gelezen is,
    // geeft die de plek van de eerste rij (2,15 × de kophoogte eronder) en de linkerkant van de
    // kolom; de volgende kop (of de rand) begrenst de kolom rechts.
    let mut heads: Vec<&OcrLine> = m.heads.clone();
    // in leesvolgorde: op een smal scherm kunnen de panelen in twee rijen staan
    let band = |l: &OcrLine| (l.yc() / (l.height().max(4.0) * 8.0)).floor();
    heads.sort_by(|a, b| band(a).total_cmp(&band(b)).then(a.x0.total_cmp(&b.x0)));
    let mut head_of: Vec<Option<&OcrLine>> = vec![None; COLUMNS.len()];
    for (c, col) in COLUMNS.iter().enumerate() {
        // de kop staat boven de labels van de kolom, op ongeveer dezelfde x
        if let Some((_, l0, _)) = label_hits.iter().find(|(f, _, _)| col.contains(f)) {
            head_of[c] = heads
                .iter()
                .copied()
                .filter(|k| k.yc() < l0.yc() && (k.x0 - l0.x0).abs() < l0.height() * 4.0)
                .min_by(|a, b| (l0.yc() - a.yc()).total_cmp(&(l0.yc() - b.yc())));
        }
    }
    let open: Vec<usize> = (0..COLUMNS.len()).filter(|c| head_of[*c].is_none()).collect();
    let rest: Vec<&OcrLine> = heads.iter().copied().filter(|k| !head_of.iter().flatten().any(|a| std::ptr::eq(*a, *k))).collect();
    if rest.len() == open.len() {
        // de overgebleven koppen horen op volgorde bij de overgebleven kolommen
        for (c, k) in open.into_iter().zip(rest) {
            head_of[c] = Some(k);
        }
    }

    let still_missing: Vec<Factor> = missing.clone();
    for f in still_missing {
        let Some((c, i)) = row(f) else { continue };
        let in_col: Vec<&(Factor, f32, f32, [u32; 4])> = found.iter().filter(|(g, _, _, _)| row(*g).is_some_and(|(cg, _)| cg == c)).collect();
        let hit = if let Some(near) = in_col.iter().min_by_key(|(g, _, _, _)| row(*g).map(|(_, ig)| ig.abs_diff(i)).unwrap_or(usize::MAX)) {
            // zelfde kolom, andere rij: de meters staan recht onder elkaar
            let (g, yc, th, bbox) = **near;
            let Some((_, ig)) = row(g) else { continue };
            let y = yc + (i as f32 - ig as f32) * pitch.unwrap_or(th * 2.45);
            (y >= 0.0 && y < img.height() as f32).then(|| find_pie(img, bbox[0] as f32 - th, bbox[2] as f32 + th, y, th)).flatten()
        } else if let Some(k) = head_of[c] {
            // niets gelezen in deze kolom: vanaf de kop, de meter is het laatste ronde blok op de rij
            let th = k.height().max(4.0);
            let y = k.yc() + th * 2.15 + i as f32 * pitch.unwrap_or(th * 2.9);
            let right = heads.iter().filter(|o| o.x0 > k.x0 + th && (o.yc() - k.yc()).abs() < th * 8.0).map(|o| o.x0 - th).fold(w, f32::min);
            (y >= 0.0 && y < img.height() as f32).then(|| find_pie_last(img, k.x0, right, y, th * 1.15)).flatten()
        } else {
            None
        };
        if std::env::var_os("FM27_VISION_DEBUG").is_some() {
            eprintln!("afgeleid {:<34} → {:?}", f.label(), hit.as_ref().map(|g| (g.fill, g.bbox)));
        }
        if let Some(g) = hit {
            let (score, doubt) = gauge_score(g.fill, g.hue, g.fill > 0.0);
            gauges.push(GaugeReading { factor: f, score, fill: g.fill, hue: g.hue, bbox: g.bbox, doubt });
            missing.retain(|m| *m != f);
        }
    }
    gauges.sort_by_key(|g| Factor::ALL.iter().position(|f| *f == g.factor));

    let mut components = Vec::new();
    for (c, line) in &m.titles {
        let end = line.x1;
        let th = line.height().max(4.0);
        // de balk loopt tot de titel van het volgende paneel
        let next_title = m
            .titles
            .iter()
            .map(|(_, l)| *l)
            .filter(|l| (l.yc() - line.yc()).abs() < th && l.x0 > end + th * 2.0)
            .map(|l| l.x0 - th)
            .fold(w, f32::min);
        if let Some(v) = read_bar(img, end + th * 0.5, next_title, line.yc(), th) {
            components.push((*c, v));
        }
    }
    // Een titel die de OCR niet (goed) las ("Taktik Anlayt*"): de balken staan op de rij van de
    // andere titels, elk rechts in zijn eigen paneel. Een paneel begint waar zijn labels of zijn
    // kop "Key Factors" beginnen (de titel staat daar vlak boven) en loopt tot het volgende paneel.
    if !m.titles.is_empty() && m.titles.len() < Component::ALL.len() {
        let median = |mut v: Vec<f32>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2]
        };
        let y = median(m.titles.iter().map(|(_, l)| l.yc()).collect());
        let th = median(m.titles.iter().map(|(_, l)| l.height().max(4.0)).collect());
        // kolommen (COLUMNS) per paneel: Tactical Understanding heeft er twee
        const PANELS: [&[usize]; 4] = [&[0, 1], &[2], &[3], &[4]];
        let left_of = |k: usize| -> Option<f32> {
            let labels = label_hits.iter().filter(|(f, _, _)| row(*f).is_some_and(|(c, _)| PANELS[k].contains(&c))).map(|(_, l, _)| l.x0);
            let heads = PANELS[k].iter().filter_map(|c| head_of[*c]).map(|h| h.x0);
            labels.chain(heads).reduce(f32::min)
        };
        let title_x = |k: usize| m.titles.iter().find(|(c, _)| *c == Component::ALL[k]).map(|(_, l)| l.x0);
        for (k, c) in Component::ALL.iter().enumerate() {
            if title_x(k).is_some() {
                continue;
            }
            let Some(x0) = left_of(k) else { continue };
            let x_end = (k + 1..Component::ALL.len()).find_map(|j| title_x(j).or_else(|| left_of(j))).map(|x| x - th).unwrap_or(w);
            if let Some(v) = read_bar(img, x0 - th, x_end, y, th) {
                components.push((*c, v));
            }
        }
    }
    components.sort_by_key(|(c, _)| Component::ALL.iter().position(|x| x == c));

    Analysis {
        gauges,
        components,
        missing,
        image_size: (img.width(), img.height()),
        lang: m.lang.name,
        labels: m.count(),
        ocr: String::new(),
        text_height,
    }
}

/// Alles wat een uitlezing zag, als tekst: om mee te sturen bij een melding, zodat een verkeerde
/// uitlezing na te spelen is zonder de opname zelf (en met: `fm27-trainer analyze <png> --ocr`).
pub fn report(a: &Analysis, lines: &[OcrLine]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "FM27 Coach {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    let _ = writeln!(s, "opname {}x{}, OCR {} (geïnstalleerd: {}), FM-taal {}, labels {}/21, tekst {:?} px", a.image_size.0, a.image_size.1, a.ocr, ocr_languages().join(" "), a.lang, a.labels, a.text_height.map(|t| t.round()));
    for g in &a.gauges {
        let _ = writeln!(s, "meter {:<36} {:>5.1}  vulling {:.2} tint {:>3.0} kader {:?}{}", g.factor.label(), g.score, g.fill, g.hue, g.bbox, if g.doubt { "  NAKIJKEN" } else { "" });
    }
    for (c, v) in &a.components {
        let _ = writeln!(s, "balk  {:<36} {:>5.1}", c.label(), v);
    }
    let _ = writeln!(s, "niet gevonden: {:?}", a.missing);
    for l in lines {
        let _ = writeln!(s, "OCR {:>6.0},{:>5.0} {:>6.0},{:>5.0}  {}", l.x0, l.y0, l.x1, l.y1, l.text);
    }
    s
}

pub fn analyze_file(path: &std::path::Path) -> Result<(Analysis, Vec<OcrLine>)> {
    let img = image::open(path).with_context(|| path.display().to_string())?.to_rgba8();
    analyze_image(&img)
}

pub fn analyze_screen() -> Result<(Analysis, RgbaImage)> {
    let img = capture_fm_window()?;
    // de opname bewaren, zodat een verkeerde uitlezing achteraf na te spelen is:
    // `fm27-trainer analyze "<map>\last-capture.png" --ocr`
    let dir = crate::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = img.save(dir.join("last-capture.png"));
    let (a, lines) = analyze_image(&img)?;
    // wat de uitlezing zag, om mee te sturen bij een melding
    let _ = std::fs::write(dir.join("last-analysis.txt"), report(&a, &lines));
    if a.gauges.len() >= 4 {
        // de laatste opname waarop echt een Breakdown stond, blijft bewaard
        let _ = img.save(dir.join("last-breakdown.png"));
    }
    if a.gauges.len() < 4 {
        let send = crate::t!(
            "Lukt het niet? Stuur last-capture.png en last-analysis.txt uit de app-map (Instellingen → Open app-map) mee met je melding.",
            "Still not working? Attach last-capture.png and last-analysis.txt from the app folder (Settings → Open app folder) to your report."
        );
        if lines.len() < 5 {
            // geen tekst: de opname is leeg of zwart (FM op volledig scherm, of geminimaliseerd)
            bail!(
                "{} {send}",
                crate::t!(
                    format!("Op de opname van het FM-venster staat bijna geen tekst ({} regels). Staat FM in beeld (niet geminimaliseerd)? Zet FM anders op venster of randloos venster en probeer opnieuw.", lines.len()),
                    format!("The capture of the FM window has almost no text ({} lines). Is FM visible (not minimised)? Otherwise switch FM to windowed or borderless and try again.", lines.len())
                )
            );
        }
        if a.labels == 0 {
            // tekst, maar geen enkel label in welke taal ook: niet de Breakdown, of een OCR die FM's
            // taal niet kan lezen
            bail!(
                "{} {send}",
                crate::t!(
                    format!("Geen labels van de Breakdown gevonden (OCR-taal {}). Open in FM27 Training → Tactical Effectiveness met de Breakdown in beeld. FM in het Chinees, Japans, Koreaans, Russisch of Grieks? Dan moet Windows die taal kunnen lezen: Instellingen → Tijd en taal → Taal en regio → taal toevoegen, met 'Optische tekenherkenning'. Of zet FM op Engels.", a.ocr),
                    format!("No Breakdown labels found (OCR language {}). Open Training → Tactical Effectiveness in FM27 with the Breakdown on screen. FM in Chinese, Japanese, Korean, Russian or Greek? Then Windows must be able to read that language: Settings → Time & language → Language & region → Add a language, with 'Optical character recognition'. Or switch FM to English.", a.ocr)
                )
            );
        }
        bail!(
            "{} {send}",
            crate::t!(
                format!("Breakdown-scherm niet herkend ({} van 17 meters gevonden). Open in FM27 het Tactical Effectiveness-overzicht met de Breakdown (Tactical Understanding / Player Ability / Player Buy-In / Next Match Info) en probeer opnieuw.", a.gauges.len()),
                format!("Breakdown screen not recognised ({} of 17 gauges found). Open the Tactical Effectiveness overview with the Breakdown (Tactical Understanding / Player Ability / Player Buy-In / Next Match Info) in FM27 and try again.", a.gauges.len())
            )
        );
    }
    Ok((a, img))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Een taartmeter zoals FM27 hem tekent: 10 segmenten met lichte spaken, `filled` gevuld met de
    /// klok mee vanaf 12 uur in `fill`, de rest bijna wit, met zachte randen (4×4 deelpixels) op de
    /// achtergrond `bg`. Midden op (cx, cy), straal `r`.
    fn draw_pie(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, filled: u32, fill: [u8; 3], bg: [u8; 3]) {
        const EMPTY: [f32; 3] = [218.0, 206.0, 208.0];
        const SPOKE: [f32; 3] = [190.0, 210.0, 222.0];
        for y in 0..img.height() {
            for x in 0..img.width() {
                let mut acc = [0f32; 3];
                for sy in 0..4 {
                    for sx in 0..4 {
                        let dx = x as f32 + (sx as f32 + 0.5) / 4.0 - cx;
                        let dy = y as f32 + (sy as f32 + 0.5) / 4.0 - cy;
                        let d = (dx * dx + dy * dy).sqrt();
                        let c: [f32; 3] = if d > r {
                            bg.map(|v| v as f32)
                        } else {
                            let a = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
                            let seg = (a / 36.0) as u32;
                            let edge = (a - seg as f32 * 36.0).min((seg + 1) as f32 * 36.0 - a);
                            // spaak: ~1 px breed, ongeacht de straal
                            if edge.to_radians() * d < 0.6 && d > 1.0 {
                                SPOKE
                            } else if seg < filled {
                                fill.map(|v| v as f32)
                            } else {
                                EMPTY
                            }
                        };
                        for i in 0..3 {
                            acc[i] += c[i] / 16.0;
                        }
                    }
                }
                let o = img.get_pixel(x, y);
                // alleen het gebied van de taart (met rand) overschrijven
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if (dx * dx + dy * dy).sqrt() <= r + 1.5 {
                    img.put_pixel(x, y, Rgba([acc[0].round() as u8, acc[1].round() as u8, acc[2].round() as u8, 255]));
                } else {
                    img.put_pixel(x, y, *o);
                }
            }
        }
    }

    fn fm_color(filled: u32) -> [u8; 3] {
        match filled {
            0..=4 => [255, 183, 0], // oranje (~43°)
            5..=8 => [255, 255, 0], // geel
            _ => [0, 235, 40],      // groen
        }
    }

    /// Elke vulling van 0 tot 10 segmenten, klein (1366×768: ~9 px straal) en groot (2560×1440:
    /// ~14 px straal), op de achtergronden die een paneel in FM27 kan hebben: donker en half
    /// doorzichtig over de clubkleur (gemeten: (70, 49, 53) bij Ajax en Sunderland), de lichtere
    /// gemarkeerde rij, en donkere tinten van andere clubkleuren.
    #[test]
    fn pies_on_any_background() {
        let backgrounds: [[u8; 3]; 7] = [[70, 49, 53], [92, 80, 84], [95, 25, 40], [30, 40, 85], [28, 75, 40], [85, 78, 25], [40, 40, 40]];
        let mut wrong = Vec::new();
        for bg in backgrounds {
            for r in [9.0f32, 14.0] {
                for filled in 0..=10u32 {
                    let mut img = RgbaImage::from_pixel(80, 48, Rgba([bg[0], bg[1], bg[2], 255]));
                    draw_pie(&mut img, 40.0, 24.0, r, filled, fm_color(filled), bg);
                    let th = r * 1.4; // labelhoogte op die schaal
                    let got = find_pie(&img, 0.0, 80.0, 24.0, th).map(|g| {
                        let (score, _) = gauge_score(g.fill, g.hue, g.fill > 0.0);
                        score.round() as u32
                    });
                    if got != Some(filled * 10) {
                        wrong.push(format!("bg {bg:?} r {r}: {filled} segmenten → {got:?}"));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "verkeerd gelezen:\n{}", wrong.join("\n"));
    }

    /// Grotere tekst in FM (of een kleinere rijafstand): de zoekband rond een label reikt dan tot in
    /// de meters van de rij erboven en eronder. Elke meter moet zijn eigen waarde houden.
    #[test]
    fn pies_in_tight_rows() {
        let bg = [70, 49, 53];
        let mut wrong = Vec::new();
        for r in [9.0f32, 14.0] {
            // rijafstand in FM: ~2,7 × de straal van een meter (38 px bij 14 px straal)
            let pitch = (r * 2.7).round();
            // labelhoogte / rijafstand: 0,53 op 2560×1440 met standaardtekst, groter met grotere tekst
            for ratio in [0.53f32, 0.62, 0.7, 0.8, 0.9, 1.0, 1.1] {
                let th = pitch * ratio;
                let rows = [7u32, 3, 9, 2, 5];
                let h = (pitch * (rows.len() as f32 + 1.0)) as u32;
                let mut img = RgbaImage::from_pixel(80, h, Rgba([bg[0], bg[1], bg[2], 255]));
                for (i, k) in rows.iter().enumerate() {
                    draw_pie(&mut img, 40.0, pitch * (i as f32 + 1.0), r, *k, fm_color(*k), bg);
                }
                for (i, k) in rows.iter().enumerate() {
                    let yc = pitch * (i as f32 + 1.0);
                    let got = find_pie(&img, 0.0, 80.0, yc, th).map(|g| gauge_score(g.fill, g.hue, g.fill > 0.0).0.round() as u32);
                    if got != Some(k * 10) {
                        wrong.push(format!("r {r} tekst/rij {ratio}: rij {i} met {k} segmenten → {got:?}"));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "verkeerd gelezen:
{}", wrong.join("
"));
    }

    #[test]
    fn colour_disagrees_with_fill() {
        // een oranje meter (laag) die als 70 % gemeten wordt: naar de rand van oranje, en nakijken
        assert_eq!(gauge_score(0.7, 43.0, true), (50.0, true));
        assert_eq!(gauge_score(0.4, 43.0, true), (40.0, false));
        assert_eq!(gauge_score(0.4, 59.0, true), (40.0, false), "40 % kan geel zijn (Ajax-save)");
        assert_eq!(gauge_score(0.9, 120.0, true), (90.0, false));
        assert_eq!(gauge_score(0.3, 120.0, true), (70.0, true));
        assert_eq!(gauge_score(0.0, 0.0, false), (0.0, true), "een lege meter: nakijken");
    }
}
