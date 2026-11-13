//! De app: zijbalk + pagina's. Alle teksten via `t!` (NL/EN), alle stijl via `theme`.

use anyhow::{Result, anyhow};
use chrono::{Duration, NaiveDate, Weekday};
use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, ScrollArea, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc;

use crate::agent;
use crate::export;
use crate::i18n::{self, Lang};
use crate::live::LiveState;
use crate::live_training;
use crate::model::{Component, Effectiveness, Factor, category_of, level_color, level_name, session_by_code};
use crate::planner::{self, CalendarOptions, CalendarWeek, PlannerSettings, WeekKind, WeekPlan, unique_schedules};
use crate::t;
use crate::theme::{self, icon};
use crate::vision;

pub fn run() -> Result<()> {
    // testhulp: FM27_WINDOW=1320x1400 voor hogere schermafbeeldingen
    let size = std::env::var("FM27_WINDOW")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])
        })
        .unwrap_or([1320.0, 880.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FM27 Coach")
            .with_inner_size(size)
            .with_min_inner_size([1040.0, 660.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/icon_256.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native(
        "FM27 Coach",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

// ------------------------------------------------------------------------ instellingen

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    lang: Lang,
    rate: f32,
    capture_delay: bool,
    #[serde(default = "yes")]
    auto_training: bool,
    /// Aanpassen in FM toestaan op een build die nog niet live getest is.
    #[serde(default)]
    allow_untested: bool,
    /// Tactisch begrip op 100 % houden zolang de app open is (FM laat het wekelijks zakken).
    #[serde(default)]
    keep_understanding: bool,
    planner: PlannerSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { lang: i18n::detect(), rate: 1.17, capture_delay: true, auto_training: true, allow_untested: false, keep_understanding: false, planner: PlannerSettings::default() }
    }
}

fn yes() -> bool {
    true
}

fn settings_file() -> PathBuf {
    crate::data_dir().join("settings.json")
}

fn calendar_file() -> PathBuf {
    crate::data_dir().join("calendar.json")
}

// ------------------------------------------------------------------------ app

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Overview,
    Training,
    Planning,
    Club,
    Squad,
    Sell,
    Settings,
}

#[derive(PartialEq, Clone, Copy)]
enum PlanTab {
    Week,
    Season,
}

type AnalysisResult = Result<(vision::Analysis, String), String>;

struct App {
    page: Page,
    settings: Settings,
    eff: Effectiveness,
    status: String,
    status_error: bool,
    analysis_rx: Option<mpsc::Receiver<AnalysisResult>>,
    image_path: String,
    show_file: bool,
    edit_mode: bool,
    plan_tab: PlanTab,
    week_start: String,
    week_matches: [bool; 7],
    prev_sunday: bool,
    next_monday: bool,
    week_kind: WeekKind,
    week_plan: Option<WeekPlan>,
    cal_opts: CalendarOptions,
    year_start: String,
    calendar: Vec<CalendarWeek>,
    year: Vec<WeekPlan>,
    live: LiveState,
    frames: u32,
    /// App icon for the sidebar logo, loaded on first use.
    logo: Option<egui::TextureHandle>,
}

impl App {
    fn new() -> Self {
        let mut settings: Settings = std::fs::read_to_string(settings_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        match std::env::var("FM27_LANG").as_deref() {
            Ok("nl") => settings.lang = Lang::Nl,
            Ok("en") => settings.lang = Lang::En,
            _ => {}
        }
        i18n::set(settings.lang);
        let cal_opts = CalendarOptions::default();
        let calendar = std::fs::read_to_string(calendar_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| planner::default_calendar(&cal_opts));
        let monday = planner::monday_of(chrono::Local::now().date_naive());
        let mut week_matches = [false; 7];
        week_matches[6] = true;
        // FM27_PAGE=training|planning|club|squad|sell|buy|settings opent direct die pagina (handig bij testen)
        let page = match std::env::var("FM27_PAGE").unwrap_or_default().as_str() {
            "training" => Page::Training,
            "planning" => Page::Planning,
            "club" => Page::Club,
            "squad" => Page::Squad,
            "sell" | "buy" => Page::Sell,
            "settings" => Page::Settings,
            _ => Page::Overview,
        };
        App {
            page,
            eff: crate::load_effectiveness().unwrap_or_default(),
            settings,
            status: String::new(),
            status_error: false,
            analysis_rx: None,
            image_path: String::new(),
            show_file: false,
            edit_mode: false,
            plan_tab: PlanTab::Week,
            week_start: monday.format("%Y-%m-%d").to_string(),
            week_matches,
            prev_sunday: true,
            next_monday: false,
            week_kind: WeekKind::Season,
            week_plan: None,
            year_start: cal_opts.start.format("%Y-%m-%d").to_string(),
            cal_opts,
            calendar,
            year: Vec::new(),
            live: {
                let mut l = LiveState::default();
                l.sell.buy = std::env::var("FM27_PAGE").as_deref() == Ok("buy");
                l
            },
            frames: 0,
            logo: None,
        }
    }

    /// Testhulp: FM27_SNAPSHOT=pad.png legt na het laden het eigen venster vast en sluit af.
    fn snapshot(&mut self, ctx: &egui::Context) {
        let Ok(path) = std::env::var("FM27_SNAPSHOT") else { return };
        self.frames += 1;
        if self.frames == 90 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(&path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    fn save_settings(&self) {
        let _ = std::fs::create_dir_all(crate::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(settings_file(), s);
        }
    }

    fn save_calendar(&self) {
        let _ = std::fs::create_dir_all(crate::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(&self.calendar) {
            let _ = std::fs::write(calendar_file(), s);
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, error: bool) {
        self.status = msg.into();
        self.status_error = error;
    }

    fn has_data(&self) -> bool {
        !self.eff.source.is_empty()
    }

    // -------------------------------------------------------------------- analyse

    fn start_analysis(&mut self, ctx: &egui::Context, file: Option<PathBuf>) {
        let (tx, rx) = mpsc::channel();
        let delay = self.settings.capture_delay && file.is_none();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            if delay && cfg!(target_os = "macos") {
                let _ = std::process::Command::new("open").args(["-b", "com.SportsInteractive.FootballManager27"]).status();
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
            let res: AnalysisResult = match file {
                Some(p) => vision::analyze_file(&p).map(|(a, _)| (a, p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())).map_err(|e| format!("{e:#}")),
                None => vision::analyze_screen().map(|(a, _)| (a, t!("FM27-scherm", "FM27 screen").to_string())).map_err(|e| format!("{e:#}")),
            };
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
        self.analysis_rx = Some(rx);
        self.set_status(t!("Bezig met uitlezen…", "Reading…"), false);
    }

    fn poll_analysis(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.analysis_rx else { return };
        if let Ok(res) = rx.try_recv() {
            self.analysis_rx = None;
            let _ = ctx;
            match res {
                Ok((a, src)) => {
                    let src_is_screen = src == t!("FM27-scherm", "FM27 screen");
                    let when = chrono::Local::now().format("%d-%m %H:%M");
                    self.eff = a.to_effectiveness(&format!("{src} · {when}"));
                    // spel-datum erbij, zodat de app weet hoe oud de meting in speltijd is
                    if let Ok(v) = agent::request(json!({"cmd": "game_date"})) {
                        let (y, d) = (v.get("year").and_then(|x| x.as_i64()), v.get("doy").and_then(|x| x.as_u64()));
                        if let (Some(y), Some(d)) = (y, d) {
                            self.eff.game_date = NaiveDate::from_yo_opt(y as i32, d as u32);
                        }
                    }
                    let _ = crate::save_effectiveness(&self.eff);
                    let mut msg = t!(format!("{} van 17 meters gelezen.", a.gauges.len()), format!("Read {} of 17 gauges.", a.gauges.len())).to_string();
                    if !a.lang.starts_with("english") {
                        msg.push_str(&format!(" (FM: {})", a.lang));
                    }
                    // een meting die niet bij de kleur van de meter past: laten nakijken
                    let doubts: Vec<&str> = a.gauges.iter().filter(|g| g.doubt).map(|g| g.factor.label()).collect();
                    if !doubts.is_empty() {
                        msg.push(' ');
                        msg.push_str(&t!(
                            format!("Kijk na (kleur en vulling kloppen niet): {}. Pas aan met 'Waarden handmatig aanpassen'.", doubts.join(", ")),
                            format!("Please check (colour and fill disagree): {}. Fix them with 'Adjust values manually'.", doubts.join(", "))
                        ));
                    }
                    let incomplete = a.gauges.len() < 17 || a.components.len() < 4 || !doubts.is_empty();
                    if incomplete && src_is_screen {
                        // zonder de opname is een verkeerde uitlezing niet na te spelen
                        msg.push(' ');
                        msg.push_str(t!(
                            "Klopt het niet? Stuur last-breakdown.png en last-analysis.txt uit de app-map (Instellingen → Open app-map) mee met je melding.",
                            "Not right? Attach last-breakdown.png and last-analysis.txt from the app folder (Settings → Open app folder) to your report."
                        ));
                    }
                    self.set_status(msg, incomplete);
                    self.week_plan = None;
                    self.year.clear();
                }
                Err(e) => self.set_status(e, true),
            }
        }
    }

    // -------------------------------------------------------------------- shell

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(236.0)
            .resizable(false)
            .frame(Frame::new().fill(theme::SIDEBAR).inner_margin(Margin { left: 14, right: 14, top: 18, bottom: 14 }).stroke(Stroke::new(1.0, theme::BORDER)))
            .show(ctx, |ui| {
                let logo = self
                    .logo
                    .get_or_insert_with(|| {
                        let img = image::load_from_memory(include_bytes!("../assets/icon/icon_256.png")).expect("bundled icon").to_rgba8();
                        let size = [img.width() as usize, img.height() as usize];
                        ctx.load_texture("app-logo", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
                    })
                    .clone();
                ui.horizontal(|ui| {
                    // the icon has transparent padding (100 of 1024 px per side): paint it 4 px larger than the
                    // 34 px slot so the visible tile fills the slot, without shifting the layout
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                    egui::Image::new(&logo).paint_at(ui, rect.expand(4.0));
                    ui.vertical(|ui| {
                        ui.add_space(1.0);
                        ui.label(RichText::new("FM27 Coach").font(egui::FontId::new(16.0, theme::bold())).color(theme::TEXT));
                        ui.label(theme::faint("Tactical Effectiveness"));
                    });
                });
                ui.add_space(22.0);
                ui.label(theme::caps("Menu"));
                ui.add_space(4.0);
                let items = [
                    (Page::Overview, icon::GAUGE, t!("Overzicht", "Overview")),
                    (Page::Training, icon::BARBELL, "Training"),
                    (Page::Planning, icon::CALENDAR_DOTS, "Planning"),
                    (Page::Club, icon::BANK, "Club"),
                    (Page::Squad, icon::USERS_THREE, t!("Selectie", "Squad")),
                    (Page::Sell, icon::HANDSHAKE, "Transfers"),
                    (Page::Settings, icon::GEAR_SIX, t!("Instellingen", "Settings")),
                ];
                for (p, ic, label) in items {
                    if theme::nav_item(ui, ic, label, self.page == p).clicked() {
                        self.page = p;
                    }
                    ui.add_space(2.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.label(theme::faint(format!("v{}", env!("CARGO_PKG_VERSION"))));
                    ui.add_space(6.0);
                    let mut lang = self.settings.lang;
                    if theme::segmented(ui, &mut lang, &[(Lang::Nl, "Nederlands"), (Lang::En, "English")]) {
                        self.settings.lang = lang;
                        i18n::set(lang);
                        self.save_settings();
                    }
                    ui.add_space(10.0);
                    Frame::new().fill(theme::SURFACE).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(12)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (c, txt) = if !agent::LIVE_SUPPORTED {
                                (theme::TEXT_FAINT, t!("Live: alleen macOS", "Live: macOS only"))
                            } else if self.live.connected {
                                (theme::SUCCESS, t!("Verbonden met FM27", "Connected to FM27"))
                            } else {
                                (theme::TEXT_FAINT, t!("Niet verbonden", "Not connected"))
                            };
                            theme::status_dot(ui, c);
                            ui.label(RichText::new(txt).size(13.0).color(theme::TEXT));
                        });
                        if let Some(n) = self.live.club_name() {
                            ui.label(theme::faint(n));
                        }
                    });
                });
            });
    }

    fn status_line(&self, ui: &mut Ui, msg: &str, error: bool) {
        if msg.is_empty() {
            return;
        }
        let (c, ic) = if error { (theme::DANGER, icon::WARNING) } else { (theme::INFO, icon::CHECK_CIRCLE) };
        Frame::new().fill(theme::tint(c, 0.12)).stroke(Stroke::new(1.0, theme::tint(c, 0.4))).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::ic(ic).color(c));
                ui.label(RichText::new(msg).color(theme::TEXT));
            });
        });
        ui.add_space(12.0);
    }

    /// Kaart met stappen om FM met de module te starten.
    fn connect_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        theme::card(ui, |ui| {
            if !agent::LIVE_SUPPORTED {
                ui.label(theme::h2(t!("Live in FM is niet beschikbaar op dit systeem", "Live in FM is not available on this system")));
                ui.label(theme::dim(t!("Uitlezen, planning en export werken wel.", "Reading, planning and export do work.")));
                return;
            }
            let windows = cfg!(windows);
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::PLUGS_CONNECTED).size(28.0).color(theme::ACCENT));
                ui.vertical(|ui| {
                    ui.label(theme::h2(t!("Verbind met Football Manager", "Connect to Football Manager")));
                    ui.label(theme::dim(if windows {
                        t!(
                            "De app leest FM27 rechtstreeks terwijl het draait. Je hoeft niets te installeren.",
                            "The app reads FM27 directly while it runs. Nothing to install."
                        )
                    } else {
                        t!(
                            "De app praat rechtstreeks met FM27 via een kleine module die met FM meestart.",
                            "The app talks to FM27 directly through a small module that starts with FM."
                        )
                    }));
                });
            });
            ui.add_space(10.0);
            let steps = if windows {
                [
                    (1, t!("Start FM27 op de pc: Steam (of de knop hieronder), Xbox-app / Game Pass of Epic.", "Start FM27 on your PC: Steam (or the button below), the Xbox app / Game Pass or Epic.")),
                    (2, t!("Laad je save.", "Load your save.")),
                    (3, t!("De app verbindt vanzelf.", "The app connects automatically.")),
                ]
            } else {
                [
                    (1, t!("Sla je spel op en sluit FM27.", "Save your game and quit FM27.")),
                    (2, t!("Klik op 'Start FM met module' (Steam moet open zijn).", "Click 'Start FM with module' (Steam must be open).")),
                    (3, t!("Laad je save. De app verbindt vanzelf.", "Load your save. The app connects automatically.")),
                ]
            };
            for (n, s) in steps {
                ui.horizontal(|ui| {
                    Frame::new().fill(theme::ACCENT_SOFT).corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
                        ui.label(RichText::new(n.to_string()).color(theme::ACCENT).strong());
                    });
                    ui.label(s);
                });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let start = if windows { t!("Start FM27", "Start FM27") } else { t!("Start FM met module", "Start FM with module") };
                if theme::primary_button(ui, icon::PLAY, start, !self.live.busy()).clicked() {
                    self.live.start_fm(ctx);
                }
                if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, t!("Opnieuw verbinden", "Reconnect"), !self.live.busy()).clicked() {
                    self.live.refresh(ctx, t!("verbinden", "connect"), None);
                }
                if self.live.busy() {
                    ui.spinner();
                }
            });
        });
    }

    // -------------------------------------------------------------------- overzicht

    fn page_overview(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let subtitle = if self.has_data() {
            t!(format!("Laatst uitgelezen: {}", self.eff.source), format!("Last read: {}", self.eff.source))
        } else {
            t!("Open in FM27 de Breakdown van Tactical Effectiveness en lees hem uit.", "Open the Tactical Effectiveness Breakdown in FM27 and read it.").to_string()
        };
        let busy = self.analysis_rx.is_some();
        let mut read = false;
        let mut file = false;
        theme::page_header(ui, "Tactical Effectiveness", &subtitle, |ui| {
            if theme::button(ui, icon::EXPORT, t!("Uit bestand", "From file"), !busy).clicked() {
                file = true;
            }
            if theme::primary_button(ui, icon::CAMERA, t!("Lees FM-scherm", "Read FM screen"), !busy).clicked() {
                read = true;
            }
            if busy {
                ui.spinner();
            }
        });
        if read {
            self.start_analysis(ctx, None);
        }
        if file {
            self.show_file = !self.show_file;
        }
        if self.show_file {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t!("Screenshot (PNG):", "Screenshot (PNG):"));
                    ui.add(egui::TextEdit::singleline(&mut self.image_path).hint_text("/…/breakdown.png").desired_width(420.0));
                    if theme::button(ui, "", t!("Lees", "Read"), !busy && !self.image_path.trim().is_empty()).clicked() {
                        let p = PathBuf::from(self.image_path.trim().trim_matches('\''));
                        self.start_analysis(ctx, Some(p));
                    }
                });
            });
            ui.add_space(10.0);
        }
        let (msg, err) = (self.status.clone(), self.status_error);
        self.status_line(ui, &msg, err);

        // onderdelen
        ui.columns(4, |cols| {
            for (i, c) in Component::ALL.iter().enumerate() {
                let v = self.eff.component(*c);
                let col = theme::rgb(level_color(v));
                let ic = [icon::TARGET, icon::SOCCER_BALL, icon::USERS_THREE, icon::EYE][i];
                theme::stat_tile(&mut cols[i], ic, c.label(), &format!("{v:.0}"), level_name(v), col, Some(v));
            }
        });
        ui.add_space(14.0);

        // factors per onderdeel
        let mut changed = false;
        ui.columns(4, |cols| {
            for (i, c) in Component::ALL.iter().enumerate() {
                let ui = &mut cols[i];
                theme::card(ui, |ui| {
                    ui.label(theme::caps(c.label()));
                    ui.add_space(6.0);
                    for f in Factor::ALL.iter().filter(|f| f.component() == *c) {
                        let v = self.eff.get(*f);
                        theme::metric_row(ui, f.label(), v, theme::rgb(level_color(v)));
                        if self.edit_mode {
                            let mut nv = v;
                            if ui.add(egui::Slider::new(&mut nv, 0.0..=100.0).show_value(false)).changed() && (nv - v).abs() > 0.01 {
                                self.eff.set(*f, nv);
                                changed = true;
                            }
                        }
                    }
                });
            }
        });
        if changed {
            self.eff.components.clear();
            if !self.eff.source.ends_with('*') {
                self.eff.source = format!("{} *", if self.eff.source.is_empty() { t!("handmatig", "manual") } else { &self.eff.source });
            }
            let _ = crate::save_effectiveness(&self.eff);
            self.week_plan = None;
            self.year.clear();
        }
        ui.add_space(4.0);
        ui.checkbox(&mut self.edit_mode, t!("Waarden handmatig aanpassen", "Adjust values manually"));
        ui.add_space(10.0);

        // prioriteiten
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::LIGHTNING).color(theme::WARNING).size(18.0));
                ui.label(theme::h2(t!("Prioriteiten", "Priorities")));
            });
            ui.add_space(6.0);
            for (f, v) in self.eff.weakest().into_iter().take(6) {
                ui.horizontal(|ui| {
                    ui.add_sized(vec2(250.0, 20.0), egui::Label::new(theme::label_strong(f.label())));
                    theme::pill(ui, &format!("{v:.0} · {}", level_name(v)), theme::rgb(level_color(v)));
                    ui.add(egui::Label::new(theme::dim(f.advice())).wrap());
                });
                ui.add_space(4.0);
            }
        });
    }

    // -------------------------------------------------------------------- training

    fn page_training(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let connected = agent::LIVE_SUPPORTED && self.live.connected;
        let can = connected && !self.live.busy();
        let (mut view, mut write) = (false, false);
        theme::page_header(
            ui,
            "Training",
            t!(
                "FM27 Coach plant je training op je zwakste gebieden en houdt hem in FM's kalender vast.",
                "FM27 Coach plans your training on your weakest areas and keeps it in FM's calendar."
            ),
            |ui| {
                if theme::primary_button(ui, icon::LIGHTNING, t!("Zet training nu in FM", "Apply training now"), can).clicked() {
                    write = true;
                }
                if theme::button(ui, icon::EYE, t!("Bekijk plan", "Preview"), can).clicked() {
                    view = true;
                }
                if self.live.busy() {
                    ui.spinner();
                }
            },
        );
        if write {
            self.live.training(ctx, &self.eff, &self.settings.planner, true, false);
        }
        if view {
            self.live.training(ctx, &self.eff, &self.settings.planner, false, false);
        }
        let (m, e) = (self.live.train_status.clone(), self.live.train_error);
        self.status_line(ui, &m, e);

        // ---- statusblok: automatisch bijhouden
        let auto = self.settings.auto_training;
        let accent = if auto && connected { theme::SUCCESS } else { theme::WARNING };
        Frame::new()
            .fill(theme::tint(accent, 0.10))
            .stroke(Stroke::new(1.0, theme::tint(accent, 0.45)))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(18))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(theme::ic(if auto { icon::CHECK_CIRCLE } else { icon::WARNING }).size(30.0).color(accent));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::h2(t!("Training automatisch bijhouden", "Keep training on plan automatically")));
                            ui.add_space(8.0);
                            if theme::toggle(ui, &mut self.settings.auto_training).changed() {
                                self.save_settings();
                            }
                            ui.label(RichText::new(if auto { t!("AAN", "ON") } else { t!("UIT", "OFF") }).strong().color(accent));
                        });
                        let text = if !connected {
                            t!(
                                "Werkt zodra FM met de module draait (zie hieronder).",
                                "Works once FM runs with the module (see below)."
                            )
                        } else if auto {
                            t!(
                                "FM's assistent plant elke week de komende 1–2 weken opnieuw in. FM27 Coach controleert elke 8 seconden en zet afwijkingen terug naar jouw plan. Laat de app open terwijl je speelt.",
                                "FM's assistant re-plans the next 1–2 weeks every week. FM27 Coach checks every 8 seconds and puts any change back on your plan. Keep the app open while you play."
                            )
                        } else {
                            t!(
                                "Uit: FM overschrijft je training elke week met zijn eigen planning. Zet dit aan, of klik na elke paar speelweken op 'Zet training nu in FM'.",
                                "Off: FM overwrites your training with its own plan every week. Turn this on, or click 'Apply training now' every few game weeks."
                            )
                        };
                        ui.add(egui::Label::new(theme::dim(text)).wrap());
                    });
                });
                if connected {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 36.0;
                        let last = self.live.last_check.map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_else(|| "—".into());
                        theme::mini_stat(ui, t!("Laatste controle", "Last check"), &last);
                        theme::mini_stat(ui, t!("Teruggezet (deze sessie)", "Restored (this session)"), &t!(format!("{} dagen", self.live.fixed_total), format!("{} days", self.live.fixed_total)));
                        let d = self.live.training.as_ref().and_then(|o| o.today).map(i18n::date).unwrap_or_else(|| "—".into());
                        theme::mini_stat(ui, t!("Spel-datum", "Game date"), &d);
                    });
                }
            });
        ui.add_space(14.0);

        // ---- zo werkt het: meten → plannen → bijhouden
        let today = self.live.training.as_ref().and_then(|o| o.today);
        let age_weeks = match (self.eff.game_date, today) {
            (Some(d), Some(t)) => Some((t - d).num_days() / 7),
            _ => None,
        };
        let stale = !self.has_data() || age_weeks.is_none_or(|w| w >= 3);
        let mut go_overview = false;
        ui.columns(3, |cols| {
            step_card(&mut cols[0], 1, icon::CAMERA, t!("Meten", "Measure"), |ui| {
                let txt = if !self.has_data() {
                    t!("Nog niet uitgelezen.", "Not read yet.").to_string()
                } else {
                    match age_weeks {
                        Some(0) => t!("Uitgelezen deze week (speltijd).", "Read this week (game time).").to_string(),
                        Some(w) => t!(format!("Uitgelezen {w} weken geleden (speltijd)."), format!("Read {w} weeks ago (game time).")),
                        None => t!(format!("Uitgelezen: {}", self.eff.source), format!("Read: {}", self.eff.source)),
                    }
                };
                ui.label(if stale { RichText::new(txt).color(theme::WARNING) } else { theme::dim(txt) });
                ui.add(egui::Label::new(theme::faint(t!(
                    "Open in FM de Breakdown en lees hem uit; doe dit elke 2–3 speelweken.",
                    "Open the Breakdown in FM and read it; do this every 2–3 game weeks."
                ))).wrap());
                if theme::small_button(ui, t!("Naar Overzicht", "Go to Overview"), true).clicked() {
                    go_overview = true;
                }
            });
            step_card(&mut cols[1], 2, icon::TARGET, t!("Plannen", "Plan"), |ui| {
                ui.label(theme::dim(t!("Extra aandacht voor:", "Extra focus on:")));
                for (f, v) in self.eff.weakest().into_iter().filter(|(f, _)| f.is_tactical_area()).take(3) {
                    theme::pill(ui, &format!("{} · {v:.0}", f.label()), theme::rgb(level_color(v)));
                }
                ui.add(egui::Label::new(theme::faint(t!(
                    "Het ritme rond wedstrijden volgt SI's presets; de focus schuift per week mee.",
                    "The rhythm around matches follows SI's presets; the focus shifts week by week."
                ))).wrap());
            });
            step_card(&mut cols[2], 3, icon::SPARKLE, t!("Bijhouden", "Keep"), |ui| {
                for l in [
                    t!("Alleen dagen ná vandaag worden aangepast.", "Only days after today are changed."),
                    t!("Wedstrijd- en reisdagen blijven van FM.", "Match and travel days stay with FM."),
                    t!("Rustweken (vakantie) blijven van FM.", "Rest weeks (holidays) stay with FM."),
                    t!("Opslaan in FM bewaart alles in je save.", "Saving in FM keeps it in your save."),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(theme::ic(icon::CHECK_CIRCLE).color(theme::SUCCESS));
                        ui.add(egui::Label::new(theme::dim(l)).wrap());
                    });
                }
            });
        });
        if go_overview {
            self.page = Page::Overview;
        }
        ui.add_space(14.0);

        if !connected {
            self.connect_card(ui, ctx);
            return;
        }

        // ---- logboek
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::ic(icon::CALENDAR_DOTS).color(theme::INFO).size(16.0));
                ui.label(theme::h2(t!("Logboek", "Activity")));
            });
            ui.add_space(4.0);
            if self.live.log.is_empty() {
                ui.label(theme::faint(t!(
                    "Nog geen aanpassingen. Zodra FM een week anders inplant, zie je hier wat er is teruggezet.",
                    "No changes yet. When FM re-plans a week, you'll see here what was put back."
                )));
            }
            for e in self.live.log.iter().take(6) {
                ui.horizontal(|ui| {
                    ui.label(theme::faint(e.at.format("%H:%M:%S").to_string()));
                    theme::pill(ui, if e.auto { t!("automatisch", "automatic") } else { t!("handmatig", "manual") }, if e.auto { theme::SUCCESS } else { theme::ACCENT });
                    ui.add(egui::Label::new(RichText::new(&e.text).color(theme::TEXT)).wrap());
                });
            }
        });
        ui.add_space(14.0);

        let Some(o) = self.live.training.clone() else {
            if !self.live.busy() {
                self.live.training(ctx, &self.eff, &self.settings.planner, false, false);
            }
            return;
        };
        ui.horizontal(|ui| {
            ui.label(theme::h2(t!("Komende weken", "Coming weeks")));
            ui.add_space(10.0);
            legend(ui);
        });
        ui.add_space(6.0);
        for w in &o.weeks {
            week_card(ui, w, o.today, &self.live.fm_had);
            ui.add_space(10.0);
        }
    }

    // -------------------------------------------------------------------- planning

    fn page_planning(&mut self, ui: &mut Ui) {
        let mut tab = self.plan_tab;
        theme::page_header(
            ui,
            "Planning",
            t!("Week- en seizoensplan als overzicht en export (werkt ook zonder module).", "Weekly and season plan as overview and export (works without the module)."),
            |ui| {
                theme::segmented(ui, &mut tab, &[(PlanTab::Week, "Week"), (PlanTab::Season, t!("Seizoen", "Season"))]);
            },
        );
        self.plan_tab = tab;
        let (m, e) = (self.status.clone(), self.status_error);
        self.status_line(ui, &m, e);
        match self.plan_tab {
            PlanTab::Week => self.planning_week(ui),
            PlanTab::Season => self.planning_season(ui),
        }
    }

    fn planning_week(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(t!("Week vanaf", "Week from"));
                ui.add(egui::TextEdit::singleline(&mut self.week_start).desired_width(96.0));
                ui.separator();
                ui.label(t!("Wedstrijd op", "Match on"));
                for i in 0..7 {
                    ui.toggle_value(&mut self.week_matches[i], i18n::weekday_short(i));
                }
                ui.separator();
                ui.checkbox(&mut self.prev_sunday, t!("wedstrijd zondag ervoor", "match previous Sunday"));
                ui.checkbox(&mut self.next_monday, t!("wedstrijd maandag erna", "match next Monday"));
                kind_combo(ui, "weekkind", &mut self.week_kind);
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // bij een testopname (FM27_SNAPSHOT) meteen een plan laten zien
                let auto = self.week_plan.is_none() && std::env::var("FM27_SNAPSHOT").is_ok();
                if theme::primary_button(ui, icon::SPARKLE, t!("Maak weekplan", "Build week plan"), true).clicked() || auto {
                    let start = NaiveDate::parse_from_str(self.week_start.trim(), "%Y-%m-%d")
                        .map(planner::monday_of)
                        .unwrap_or_else(|_| planner::monday_of(chrono::Local::now().date_naive()));
                    let week = CalendarWeek { start, kind: self.week_kind, matches: self.week_matches, note: String::new() };
                    self.week_plan = Some(planner::plan_week(&week, self.prev_sunday, self.next_monday, &self.eff, &self.settings.planner, 1));
                }
                if let Some(p) = self.week_plan.clone() {
                    if theme::button(ui, icon::EXPORT, t!("Exporteer & open", "Export & open"), true).clicked() {
                        let r = export::write_all(&export::output_dir(), "weekplan", t!("FM27 weekplan", "FM27 week plan"), &self.eff, std::slice::from_ref(&p));
                        self.after_export(r);
                    }
                }
            });
        });
        ui.add_space(12.0);
        if let Some(p) = &self.week_plan {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::h2(&p.schedule_name));
                    theme::pill(ui, &format!("Focus: {}", p.focus), theme::ACCENT);
                });
                ui.add_space(8.0);
                plan_grid(ui, p);
                if !p.notes.is_empty() {
                    ui.add_space(8.0);
                    ui.label(theme::caps(t!("Buiten de training", "Beyond training")));
                    for n in &p.notes {
                        ui.add(egui::Label::new(theme::dim(format!("• {n}"))).wrap());
                    }
                }
            });
            ui.add_space(8.0);
            legend(ui);
        }
    }

    fn planning_season(&mut self, ui: &mut Ui) {
        theme::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Start");
                ui.add(egui::TextEdit::singleline(&mut self.year_start).desired_width(96.0));
                ui.label(t!("Competitie op", "League on"));
                weekday_combo(ui, "league", &mut self.cal_opts.league_day);
                ui.label(t!("Europees", "Europe"));
                let mut e = self.cal_opts.europe;
                egui::ComboBox::from_id_salt("europe")
                    .selected_text(match e {
                        None => t!("geen", "none"),
                        Some(Weekday::Tue) => t!("di (CL)", "Tue (CL)"),
                        Some(Weekday::Wed) => t!("wo (CL)", "Wed (CL)"),
                        _ => t!("do (EL/ECL)", "Thu (EL/ECL)"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut e, None, t!("geen", "none"));
                        ui.selectable_value(&mut e, Some(Weekday::Tue), t!("di (CL)", "Tue (CL)"));
                        ui.selectable_value(&mut e, Some(Weekday::Wed), t!("wo (CL)", "Wed (CL)"));
                        ui.selectable_value(&mut e, Some(Weekday::Thu), t!("do (EL/ECL)", "Thu (EL/ECL)"));
                    });
                self.cal_opts.europe = e;
                ui.checkbox(&mut self.cal_opts.cup, t!("beker", "cup"));
                ui.add(egui::DragValue::new(&mut self.cal_opts.weeks).range(4..=60).suffix(t!(" weken", " weeks")));
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::button(ui, icon::CALENDAR_BLANK, t!("Standaardkalender", "Default calendar"), true).clicked() {
                    if let Ok(d) = NaiveDate::parse_from_str(self.year_start.trim(), "%Y-%m-%d") {
                        self.cal_opts.start = d;
                    }
                    self.calendar = planner::default_calendar(&self.cal_opts);
                    self.year.clear();
                    self.save_calendar();
                }
                if theme::primary_button(ui, icon::SPARKLE, t!("Maak seizoensplan", "Build season plan"), true).clicked() {
                    self.year = planner::plan_year(&self.calendar, &self.eff, &self.settings.planner);
                    self.save_calendar();
                }
                if !self.year.is_empty() {
                    if theme::button(ui, icon::EXPORT, t!("Exporteer & open", "Export & open"), true).clicked() {
                        let r = export::write_all(&export::output_dir(), "seasonplan", t!("FM27 seizoensplan", "FM27 season plan"), &self.eff, &self.year);
                        self.after_export(r);
                    }
                    let (n, u) = (self.year.len(), unique_schedules(&self.year).len());
                    ui.label(theme::faint(t!(format!("{n} weken · {u} verschillende weekschema's"), format!("{n} weeks · {u} distinct weekly schedules"))));
                }
            });
        });
        ui.add_space(12.0);
        let mut edited = false;
        theme::card(ui, |ui| {
            egui::Grid::new("cal").num_columns(5).spacing([14.0, 8.0]).striped(true).show(ui, |ui| {
                for h in [t!("Week van", "Week of"), t!("Periode", "Period"), t!("Wedstrijden", "Matches"), "Focus", t!("Tactisch", "Tactical")] {
                    ui.label(theme::caps(h));
                }
                ui.end_row();
                for (wi, week) in self.calendar.iter_mut().enumerate() {
                    ui.label(i18n::date(week.start));
                    let before = week.kind;
                    kind_combo(ui, &format!("k{wi}"), &mut week.kind);
                    edited |= week.kind != before;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        for d in 0..7 {
                            if ui.toggle_value(&mut week.matches[d], &i18n::weekday_short(d)[..1]).changed() {
                                edited = true;
                            }
                        }
                        if !week.note.is_empty() {
                            ui.label(theme::faint(&week.note));
                        }
                    });
                    if let Some(p) = self.year.get(wi) {
                        theme::pill(ui, p.focus, theme::ACCENT);
                        let t: Vec<String> = p.targets.iter().map(|(f, n)| format!("{}×{}", planner::short(*f), n)).collect();
                        ui.label(theme::faint(t.join("  ")));
                    } else {
                        ui.label("");
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
        });
        if edited {
            self.year.clear();
            self.save_calendar();
        }
    }

    fn after_export(&mut self, res: Result<PathBuf>) {
        match res {
            Ok(p) => {
                open_path(&p);
                self.set_status(t!(format!("Opgeslagen: {}", p.display()), format!("Saved: {}", p.display())), false);
            }
            Err(e) => self.set_status(format!("{e:#}"), true),
        }
    }

    // -------------------------------------------------------------------- club

    fn page_club(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let busy = self.live.busy();
        let connected = self.live.connected;
        let name = self.live.club_name().unwrap_or_else(|| "Club".into());
        let mut action: Option<(String, Value)> = None;
        let mut backup = false;
        theme::page_header(
            ui,
            &name,
            t!("Financiën live aanpassen. Wijzigingen blijven bewaard als je in FM opslaat.", "Edit finances live. Changes persist once you save in FM."),
            |ui| {
                if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, t!("Ongedaan maken", "Undo"), connected && !busy).clicked() {
                    action = Some((t!("ongedaan maken", "undo").into(), json!({"cmd": "undo"})));
                }
                if theme::button(ui, icon::FLOPPY_DISK, t!("Backup saves", "Back up saves"), !busy).clicked() {
                    backup = true;
                }
            },
        );
        if backup {
            match agent::backup_saves() {
                Ok(d) => self.live.status = t!(format!("Backup gemaakt: {}", d.display()), format!("Backup created: {}", d.display())),
                Err(e) => self.live.status = format!("{e:#}"),
            }
        }
        if let Some((l, r)) = action.take() {
            self.live.refresh(ctx, &l, Some(r));
        }
        let (m, e) = (self.live.status.clone(), self.live.status_error);
        self.status_line(ui, &m, e);
        if !connected {
            self.connect_card(ui, ctx);
            return;
        }
        let Some(c) = self.live.club.clone() else { return };
        let g = |k: &str| c.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        let rate = self.settings.rate;
        let eur = |v: i64| format!("€ {}", money((v as f32 * rate) as i64));
        let gbp = |v: i64| format!("£ {}", money(v));
        ui.columns(3, |cols| {
            theme::stat_tile(&mut cols[0], icon::COINS, t!("Saldo", "Balance"), &eur(g("balance")), &gbp(g("balance")), theme::INFO, None);
            theme::stat_tile(&mut cols[1], icon::BANK, t!("Transferbudget", "Transfer budget"), &eur(g("transfer_budget")), &gbp(g("transfer_budget")), theme::ACCENT, None);
            theme::stat_tile(&mut cols[2], icon::USERS_THREE, t!("Loonbudget p/w", "Wage budget p/w"), &eur(g("wage_budget")), &gbp(g("wage_budget")), theme::SUCCESS, None);
        });
        ui.add_space(14.0);
        ui.columns(2, |cols| {
            theme::card(&mut cols[0], |ui| {
                ui.label(theme::h2(t!("Transferbudget", "Transfer budget")));
                ui.label(theme::faint(t!("Bedrag in miljoen euro", "Amount in million euro")));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.live.transfer_input).hint_text("50").desired_width(90.0));
                    ui.label(t!("mln €", "M €"));
                    if theme::primary_button(ui, icon::CHECK_CIRCLE, t!("Zet", "Set"), !busy).clicked() {
                        if let Ok(m) = self.live.transfer_input.replace(',', ".").trim().parse::<f64>() {
                            action = Some((t!("transferbudget", "transfer budget").into(), json!({"cmd": "set_finances", "transfer_budget": (m * 1e6 / rate as f64) as i64})));
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for add in [10, 25, 50, 100] {
                        if theme::small_button(ui, &format!("+€{add}M"), !busy).clicked() {
                            let v = g("transfer_budget") + (add as f64 * 1e6 / rate as f64) as i64;
                            action = Some((t!("transferbudget", "transfer budget").into(), json!({"cmd": "set_finances", "transfer_budget": v})));
                        }
                    }
                });
            });
            theme::card(&mut cols[1], |ui| {
                ui.label(theme::h2(t!("Loonbudget", "Wage budget")));
                ui.label(theme::faint(t!("Bedrag in duizend euro per week", "Amount in thousand euro per week")));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.live.wage_input).hint_text("900").desired_width(90.0));
                    ui.label("k € p/w");
                    if theme::primary_button(ui, icon::CHECK_CIRCLE, t!("Zet", "Set"), !busy).clicked() {
                        if let Ok(k) = self.live.wage_input.replace(',', ".").trim().parse::<f64>() {
                            action = Some((t!("loonbudget", "wage budget").into(), json!({"cmd": "set_finances", "wage_budget": (k * 1e3 / rate as f64) as i64})));
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for add in [50, 100, 250] {
                        if theme::small_button(ui, &format!("+€{add}k"), !busy).clicked() {
                            let v = g("wage_budget") + (add as f64 * 1e3 / rate as f64) as i64;
                            action = Some((t!("loonbudget", "wage budget").into(), json!({"cmd": "set_finances", "wage_budget": v})));
                        }
                    }
                });
            });
        });
        ui.add_space(8.0);
        ui.label(theme::faint(t!(
            format!("FM rekent intern in ponden; euro's zijn omgerekend met koers {rate:.2} (aan te passen bij Instellingen)."),
            format!("FM stores pounds internally; euros use rate {rate:.2} (change it in Settings).")
        )));
        ui.add_space(12.0);
        // de vlaggen van FM's in-game editor op je manager: registratie en rugnummers altijd
        let hf = self.live.human_flags.clone();
        theme::card(ui, |ui| {
            ui.label(theme::h2(t!("Spelregels", "Rules")));
            ui.label(theme::faint(t!(
                "Dezelfde schakelaars als in FM's in-game editor, direct op je manager gezet. Controleer het in FM op het registratiescherm van de competitie.",
                "The same switches as FM's in-game editor, set straight on your manager. Check it in FM on the competition's registration screen."
            )));
            ui.add_space(6.0);
            match hf.as_ref().and_then(|v| v.get("supported")).and_then(|v| v.as_bool()) {
                Some(true) => {
                    let rows = [
                        ("registration_anytime", t!("Spelers altijd registreren", "Register players anytime"), t!("Selectieregistratie ook buiten de registratieperiode (competitie, UEFA).", "Squad registration outside the registration window too (league, UEFA).")),
                        ("numbers_anytime", t!("Rugnummers altijd aanpassen", "Change squad numbers anytime"), t!("Rugnummers ook wijzigen als de competitie ze heeft vastgezet.", "Change squad numbers even when the competition has locked them.")),
                    ];
                    for (key, label, hint) in rows {
                        let mut on = hf.as_ref().and_then(|v| v.get(key)).and_then(|v| v.as_bool()).unwrap_or(false);
                        ui.horizontal(|ui| {
                            if theme::toggle(ui, &mut on).changed() && connected && !busy {
                                let mut m = serde_json::Map::new();
                                m.insert("cmd".into(), Value::String("set_human_flags".into()));
                                m.insert(key.into(), Value::Bool(on));
                                action = Some((label.to_string(), Value::Object(m)));
                            }
                            ui.label(theme::label_strong(label));
                            ui.label(theme::faint(hint));
                        });
                    }
                }
                Some(false) => {
                    ui.label(theme::faint(t!("Op deze FM-versie of dit platform nog niet beschikbaar.", "Not available on this FM build or platform yet.")));
                }
                None => {
                    ui.label(theme::faint(t!("Nog niet uitgelezen.", "Not read yet.")));
                }
            }
        });
        if let Some((l, r)) = action {
            self.live.refresh(ctx, &l, Some(r));
        }
    }

    // -------------------------------------------------------------------- selectie

    fn page_squad(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let busy = self.live.busy();
        let connected = self.live.connected;
        let injured = self.live.squad.iter().filter(|p| p.get("injuries").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty())).count();
        let banned = self.live.squad.iter().filter(|p| p.get("bans").and_then(|v| v.as_u64()).unwrap_or(0) > 0).count();
        let mut action: Option<(String, Value)> = None;
        let mut sell: Option<u32> = None;
        let n = self.live.squad.len();
        let subtitle = t!(format!("{n} spelers · {injured} geblesseerd · {banned} geschorst"), format!("{n} players · {injured} injured · {banned} suspended"));
        theme::page_header(ui, t!("Selectie", "Squad"), &subtitle, |ui| {
            if theme::button(ui, icon::SMILEY, t!("Moraal top voor iedereen", "Max morale for all"), connected && !busy).clicked() {
                action = Some((t!("moraal", "morale").into(), json!({"cmd": "inspire", "morale": 20, "clear_unhappiness": true})));
            }
            if theme::button(ui, icon::WARNING, t!("Schorsingen opheffen", "Lift suspensions"), connected && !busy && banned > 0).clicked() {
                action = Some((t!("schorsingen", "suspensions").into(), json!({"cmd": "lift_bans"})));
            }
            let heal = t!(format!("Genees iedereen ({injured})"), format!("Heal all ({injured})"));
            if theme::primary_button(ui, icon::FIRST_AID, &heal, connected && !busy && injured > 0).clicked() {
                action = Some((t!("genezen", "healing").into(), json!({"cmd": "heal"})));
            }
        });
        let (m, e) = (self.live.status.clone(), self.live.status_error);
        self.status_line(ui, &m, e);
        if !connected {
            self.connect_card(ui, ctx);
            return;
        }
        // tactisch begrip (FM: Tactical Understanding), te maximaliseren zoals FM's in-game editor dat kan
        let tu = self.live.understanding.clone();
        let tu_players: std::collections::HashMap<u64, f32> = tu
            .as_ref()
            .and_then(|u| u.get("players"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let uid = p.get("uid")?.as_u64()?;
                        let levels: Vec<f32> = p.get("levels")?.as_array()?.iter().filter_map(|x| x.as_u64()).map(|x| x as f32 / 100.0).collect();
                        (!levels.is_empty()).then(|| (uid, levels.iter().sum::<f32>() / levels.len() as f32))
                    })
                    .collect()
            })
            .unwrap_or_default();
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(theme::h2(t!("Tactisch begrip", "Tactical understanding")));
                    ui.add(
                        egui::Label::new(theme::faint(t!(
                            "Hoe goed de selectie je huidige tactiek kent, per onderdeel (gemiddelde van de spelers). Maximaliseren doet wat de in-game editor van FM doet: alles naar 100 %.",
                            "How well the squad knows your current tactic, per area (average over the players). Maximise does what FM's in-game editor does: everything to 100%."
                        )))
                        .wrap(),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if theme::primary_button(ui, icon::BRAIN, t!("Maximaliseer tactisch begrip", "Maximise tactical understanding"), connected && !busy).clicked() {
                        action = Some((t!("tactisch begrip", "tactical understanding").into(), json!({"cmd": "max_understanding"})));
                    }
                });
            });
            match tu.as_ref().and_then(|u| u.get("team")).and_then(|v| v.as_array()) {
                Some(team) if !team.is_empty() => {
                    ui.add_space(6.0);
                    // twee rijen van vier: in bezit, dan zonder bal
                    for row in team.iter().take(UNDERSTANDING_AREAS.len()).enumerate().collect::<Vec<_>>().chunks(4) {
                        ui.horizontal(|ui| {
                            for (i, v) in row {
                                let pct = v.as_u64().unwrap_or(0) as f32 / 100.0;
                                theme::pill(ui, &format!("{} · {pct:.0}%", UNDERSTANDING_AREAS[*i]), theme::rgb(level_color(pct)));
                            }
                        });
                    }
                    let without = tu.as_ref().and_then(|u| u.get("without_record")).and_then(|v| v.as_u64()).unwrap_or(0);
                    if without > 0 {
                        ui.label(theme::faint(t!(
                            format!("{without} spelers hebben nog geen begrip-record (nog niet met dit team getraind); die doen pas na een week training mee."),
                            format!("{without} players have no understanding record yet (not trained with this team yet); they join after a week of training.")
                        )));
                    }
                }
                _ => {
                    ui.label(theme::faint(t!("Nog niet uitgelezen.", "Not read yet.")));
                }
            }
            // FM laat het begrip elke speelweek een stuk zakken: op verzoek houdt de app het op 100 %
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let keep = self.settings.keep_understanding;
                if theme::toggle(ui, &mut self.settings.keep_understanding).changed() {
                    self.save_settings();
                }
                ui.label(theme::label_strong(t!("Op 100 % houden", "Keep at 100%")));
                ui.label(RichText::new(if keep { t!("AAN", "ON") } else { t!("UIT", "OFF") }).strong().color(if keep { theme::SUCCESS } else { theme::TEXT_FAINT }));
                ui.add(
                    egui::Label::new(theme::faint(t!(
                        "Zolang de app open is, zet hij elke 20 seconden terug wat FM heeft laten zakken (FM Live Editor doet dit niet vanzelf).",
                        "While the app is open, every 20 seconds it puts back whatever FM has let drop (FM Live Editor does not do this by itself)."
                    )))
                    .wrap(),
                );
            });
        });
        theme::card(ui, |ui| {
            egui::Grid::new("squad").num_columns(8).spacing([18.0, 10.0]).striped(true).show(ui, |ui| {
                for h in [t!("Speler", "Player"), t!("Moraal", "Morale"), t!("Conditie", "Condition"), t!("Begrip", "Understanding"), t!("Blessure", "Injury"), t!("Schorsing", "Ban"), t!("Ontevreden", "Unhappy"), ""] {
                    ui.label(theme::caps(h));
                }
                ui.end_row();
                for p in &self.live.squad {
                    let uid = p.get("uid").and_then(|v| v.as_u64()).unwrap_or(0);
                    let m = p.get("morale").and_then(|v| v.as_u64()).unwrap_or(0);
                    ui.label(theme::label_strong(p.get("name").and_then(|v| v.as_str()).unwrap_or("?")));
                    theme::pill(ui, &format!("{m} · {}", morale_label(m)), theme::rgb(level_color((m as f32 - 1.0) / 19.0 * 100.0)));
                    let cond = p.get("condition").and_then(|v| v.as_u64()).unwrap_or(0) as f32 / 100.0;
                    ui.scope(|ui| {
                        ui.set_width(90.0);
                        theme::progress(ui, cond, 6.0, theme::rgb(level_color(cond)));
                    });
                    match tu_players.get(&uid) {
                        Some(pct) => {
                            theme::pill(ui, &format!("{pct:.0}%"), theme::rgb(level_color(*pct)));
                        }
                        None => {
                            ui.label(theme::faint("—"));
                        }
                    }
                    let inj: Vec<String> = p
                        .get("injuries")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().map(|i| i.get("name").and_then(|n| n.as_str()).unwrap_or("injury").to_string()).collect())
                        .unwrap_or_default();
                    if inj.is_empty() {
                        ui.label(theme::faint("—"));
                    } else {
                        theme::pill(ui, &inj.join(", "), theme::DANGER);
                    }
                    let bans = p.get("bans").and_then(|v| v.as_u64()).unwrap_or(0);
                    if bans > 0 {
                        theme::pill(ui, t!("geschorst", "suspended"), theme::WARNING);
                    } else {
                        ui.label(theme::faint("—"));
                    }
                    let unhappy = p.get("unhappy_reasons").and_then(|v| v.as_bool()).unwrap_or(false);
                    if unhappy {
                        theme::pill(ui, t!("ja", "yes"), theme::WARNING);
                    } else {
                        ui.label(theme::faint("—"));
                    }
                    ui.horizontal(|ui| {
                        if !inj.is_empty() && theme::small_button(ui, t!("Genees", "Heal"), !busy).clicked() {
                            action = Some((t!("genezen", "healing").into(), json!({"cmd": "heal", "uids": [uid]})));
                        }
                        if (m < 20 || unhappy) && theme::small_button(ui, t!("Inspireer", "Inspire"), !busy).clicked() {
                            action = Some((t!("moraal", "morale").into(), json!({"cmd": "inspire", "uids": [uid], "morale": 20, "clear_unhappiness": true})));
                        }
                        let coach = p.get("player_coach").and_then(|v| v.as_bool()).unwrap_or(false);
                        if !coach && theme::small_button(ui, t!("Verkoop", "Sell"), !busy).clicked() {
                            sell = Some(uid as u32);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some((l, r)) = action {
            self.live.refresh(ctx, &l, Some(r));
        }
        if let Some(uid) = sell {
            self.start_sale(uid);
        }
    }

    /// Naar de verkooppagina met deze speler gekozen; de vraagprijs begint bij zijn waarde.
    fn start_sale(&mut self, uid: u32) {
        let value = self.live.squad.iter().find(|p| p.get("uid").and_then(|v| v.as_u64()) == Some(uid as u64)).and_then(|p| p.get("value")).and_then(|v| v.as_i64()).unwrap_or(0);
        self.live.sell.player = Some(uid);
        self.live.sell.buy = false;
        self.live.sell.plan = None;
        self.live.sell.status.clear();
        self.live.sell.fee_input = format!("{:.1}", value as f64 * self.settings.rate as f64 / 1e6);
        self.page = Page::Sell;
    }

    // -------------------------------------------------------------------- verkopen

    fn page_sell(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let busy = self.live.busy() || self.live.sell_busy();
        let connected = self.live.connected;
        let rate = self.settings.rate as f64;
        let mut undo = false;
        let buying = self.live.sell.buy;
        theme::page_header(
            ui,
            "Transfers",
            if buying {
                t!(
                    "Haal een speler van een andere club direct naar je eerste elftal. Jij betaalt de transfersom.",
                    "Bring a player from another club straight into your first team. You pay the fee."
                )
            } else {
                t!(
                    "Verkoop een speler direct aan een club naar keuze. De transfersom wordt meteen geboekt.",
                    "Sell a player straight to a club of your choice. The fee is booked at once."
                )
            },
            |ui| {
                if theme::button(ui, icon::ARROW_COUNTER_CLOCKWISE, t!("Ongedaan maken", "Undo"), connected && !busy).clicked() {
                    undo = true;
                }
            },
        );
        if undo {
            self.live.sell.plan = None;
            self.live.refresh(ctx, t!("ongedaan maken", "undo"), Some(json!({"cmd": "undo"})));
        }
        let (m, e) = (self.live.status.clone(), self.live.status_error);
        if !m.is_empty() && (e || undo) {
            self.status_line(ui, &m, e);
        }
        let (m, e) = (self.live.sell.status.clone(), self.live.sell.error);
        self.status_line(ui, &m, e);
        if !connected {
            self.connect_card(ui, ctx);
            return;
        }
        let mut buy = self.live.sell.buy;
        if theme::segmented(ui, &mut buy, &[(false, t!("Verkopen", "Sell")), (true, t!("Kopen", "Buy"))]) {
            self.live.sell.buy = buy;
            self.live.sell.plan = None;
            self.live.sell.status.clear();
        }
        ui.add_space(10.0);
        if self.live.sell.buy {
            self.page_buy(ui, ctx);
            return;
        }

        let eur = |pounds: i64| format!("€ {}", money((pounds as f64 * rate) as i64));
        let squad = self.live.squad.clone();
        let chosen = self.live.sell.player.and_then(|uid| squad.iter().find(|p| p.get("uid").and_then(|v| v.as_u64()) == Some(uid as u64)).cloned());
        let mut pick: Option<u32> = None;
        let mut search = false;
        let mut check: Option<(u32, u32, i64, Option<i64>)> = None;
        let mut confirm: Option<(u32, u32, i64, Option<i64>)> = None;

        ui.columns(2, |cols| {
            // 1. speler
            theme::card(&mut cols[0], |ui| {
                ui.label(theme::h2(t!("1 · Speler", "1 · Player")));
                ui.label(theme::faint(t!("Kies wie je wilt verkopen.", "Choose who to sell.")));
                ui.add_space(6.0);
                ScrollArea::vertical().id_salt("sell_players").max_height(430.0).show(ui, |ui| {
                    let mut sorted = squad.clone();
                    sorted.sort_by_key(|p| std::cmp::Reverse(p.get("value").and_then(|v| v.as_i64()).unwrap_or(0)));
                    for p in &sorted {
                        let uid = p.get("uid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let coach = p.get("player_coach").and_then(|v| v.as_bool()).unwrap_or(false);
                        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let value = p.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                        let until = p.get("contract_ends").and_then(|d| d.get("year")).and_then(|y| y.as_u64());
                        let selected = self.live.sell.player == Some(uid);
                        ui.horizontal(|ui| {
                            let label = if coach { format!("{name} · {}", t!("speler-trainer", "player-coach")) } else { name.to_string() };
                            let r = ui.add_enabled(!coach, egui::SelectableLabel::new(selected, RichText::new(label).size(14.0)));
                            if r.clicked() {
                                pick = Some(uid);
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if let Some(y) = until {
                                    ui.label(theme::faint(format!("{} {y}", t!("tot", "until"))));
                                }
                                ui.label(RichText::new(eur(value)).color(theme::TEXT));
                                if p.get("listed").and_then(|v| v.as_u64()).unwrap_or(0) & 1 == 1 {
                                    theme::pill(ui, t!("transferlijst", "listed"), theme::WARNING);
                                }
                            });
                        });
                    }
                });
            });

            // 2. koper en 3. bedrag
            let ui = &mut cols[1];
            theme::card(ui, |ui| {
                ui.label(theme::h2(t!("2 · Koper", "2 · Buyer")));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.live.sell.query).hint_text(t!("Zoek een club, bijv. AC Milan", "Search a club, e.g. AC Milan")).desired_width(260.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        search = true;
                    }
                    if theme::small_button(ui, t!("Zoek", "Search"), !busy && self.live.sell.query.trim().len() >= 2).clicked() {
                        search = true;
                    }
                });
                // zoeken terwijl je typt
                let q = self.live.sell.query.trim().to_string();
                if q.len() >= 3 && q != self.live.sell.searched && !self.live.sell_busy() {
                    search = true;
                }
                ui.add_space(4.0);
                let clubs = self.live.sell.clubs.clone();
                for c in clubs.iter().take(8) {
                    let uid = c.get("uid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                    let selected = self.live.sell.club.as_ref().is_some_and(|(u, _)| *u == uid);
                    ui.horizontal(|ui| {
                        if ui.add(egui::SelectableLabel::new(selected, RichText::new(&name).size(14.0))).clicked() {
                            self.live.sell.club = Some((uid, name.clone()));
                            self.live.sell.plan = None;
                        }
                        if c.get("women").and_then(|v| v.as_bool()).unwrap_or(false) {
                            theme::pill(ui, t!("vrouwen", "women"), theme::INFO);
                        }
                    });
                }
                if clubs.is_empty() && !self.live.sell.searched.is_empty() && !self.live.sell_busy() {
                    ui.label(theme::faint(t!("Geen clubs gevonden.", "No clubs found.")));
                }
            });
            ui.add_space(12.0);
            theme::card(ui, |ui| {
                ui.label(theme::h2(t!("3 · Bedrag", "3 · Fee")));
                ui.add_space(4.0);
                let value = chosen.as_ref().and_then(|p| p.get("value")).and_then(|v| v.as_i64()).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.live.sell.fee_input).hint_text("10").desired_width(90.0));
                    ui.label(t!("mln €", "M €"));
                    for (label, f) in [(t!("waarde", "value"), 1.0), ("×1.5", 1.5), ("×2", 2.0), ("×3", 3.0)] {
                        if theme::small_button(ui, label, value > 0).clicked() {
                            self.live.sell.fee_input = format!("{:.1}", value as f64 * f * rate / 1e6);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(t!("Nieuw salaris", "New wage"));
                    ui.add(egui::TextEdit::singleline(&mut self.live.sell.wage_input).hint_text(t!("blijft", "unchanged")).desired_width(90.0));
                    ui.label("k € p/w");
                });
                let fee = self.live.sell.fee_input.replace(',', ".").trim().parse::<f64>().ok().map(|m| (m * 1e6 / rate).round() as i64);
                let wage = self.live.sell.wage_input.replace(',', ".").trim().parse::<f64>().ok().map(|k| (k * 1e3 / rate).round() as i64);
                let key = match (self.live.sell.player, &self.live.sell.club, fee) {
                    (Some(p), Some((c, _)), Some(f)) if f >= 0 => Some((p, *c, f, wage)),
                    _ => None,
                };
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::button(ui, icon::MAGNIFYING_GLASS, t!("Controleer", "Check"), key.is_some() && !busy).clicked() {
                        check = key;
                    }
                    let ready = self.live.sell.plan.as_ref().is_some_and(|(v, k)| Some(*k) == key && v.get("allowed").and_then(|a| a.as_bool()) == Some(true));
                    let label = match (&chosen, &self.live.sell.club) {
                        (Some(p), Some((_, c))) => t!(
                            format!("Verkoop {} aan {}", p.get("name").and_then(|v| v.as_str()).unwrap_or("?"), c),
                            format!("Sell {} to {}", p.get("name").and_then(|v| v.as_str()).unwrap_or("?"), c)
                        ),
                        _ => t!("Verkoop", "Sell").to_string(),
                    };
                    if theme::primary_button(ui, icon::HANDSHAKE, &label, ready && !busy).clicked() {
                        confirm = key;
                    }
                    if self.live.sell_busy() {
                        ui.spinner();
                    }
                });
                if let Some((v, k)) = &self.live.sell.plan {
                    if Some(*k) == key {
                        ui.add_space(8.0);
                        let s = |n: &str| v.get(n).and_then(|x| x.as_str()).unwrap_or("?").to_string();
                        let i = |n: &str| v.get(n).and_then(|x| x.as_i64()).unwrap_or(0);
                        let b = |n: &str| v.get(n).and_then(|x| x.as_bool()).unwrap_or(false);
                        let mut lines = vec![t!(
                            format!("{} gaat naar {} voor {}.", s("player"), s("to"), eur(i("fee"))),
                            format!("{} moves to {} for {}.", s("player"), s("to"), eur(i("fee")))
                        )];
                        lines.push(if b("buyer_pays") {
                            t!(format!("{} betaalt uit saldo en transferbudget.", s("to")), format!("{} pays from its balance and transfer budget.", s("to")))
                        } else {
                            t!(format!("{} heeft geen volledige financiën in je save: alleen jij krijgt het geld.", s("to")), format!("{} has no full finances in your save: only you receive the money.", s("to")))
                        });
                        lines.push(t!(
                            format!("Jouw saldo wordt {} en je transferbudget {}.", eur(i("balance_after")), eur(i("transfer_budget_after"))),
                            format!("Your balance becomes {} and your transfer budget {}.", eur(i("balance_after")), eur(i("transfer_budget_after")))
                        ));
                        if i("selections") > 0 {
                            lines.push(t!(format!("Zijn plek in {} opstelling(en) wordt leeg.", i("selections")), format!("His place in {} selection(s) is emptied.", i("selections"))));
                        }
                        if b("was_captain") {
                            lines.push(t!("Hij was aanvoerder: de band is vrij voor iemand anders.", "He was captain: the armband is free for someone else.").into());
                        }
                        if !b("allowed") {
                            lines.push(t!("Deze FM-versie is niet getest: sta aanpassen eerst toe in Instellingen.", "This FM build is untested: allow changes in Settings first.").into());
                        }
                        for l in lines {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(theme::ic(icon::CHECK_CIRCLE).color(theme::SUCCESS));
                                ui.label(l);
                            });
                        }
                    }
                }
            });
        });
        ui.add_space(12.0);
        theme::card(ui, |ui| {
            ui.label(theme::h2(t!("Goed om te weten", "Good to know")));
            ui.add_space(4.0);
            for l in [
                t!("Dit is een directe verkoop, geen bod: FM zet de transfer niet in het nieuws of de transferhistorie.", "This is a direct sale, not an offer: FM does not put the transfer in the news or the transfer history."),
                t!("De speler krijgt bij de koper hetzelfde contract (of het nieuwe salaris), vanaf vandaag, zonder rugnummer.", "The player keeps his contract at the buyer (or the new wage), from today, without a squad number."),
                t!("Ongedaan maken zet alles terug zolang je nog niet verder hebt gespeeld. Maak eerst een backup en sla pas op als alles klopt.", "Undo puts everything back as long as you have not played on. Back up first and only save when everything is right."),
                t!("Speler-trainers en gehuurde spelers kunnen (nog) niet worden verkocht.", "Player-coaches and loaned players cannot be sold (yet)."),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(theme::ic(icon::INFO).color(theme::INFO));
                    ui.label(theme::dim(l));
                });
            }
        });

        if let Some(uid) = pick {
            self.start_sale(uid);
        }
        if search {
            self.live.search_clubs(ctx);
        }
        if let Some((p, c, f, w)) = check {
            self.live.plan_sale(ctx, p, c, f, w);
        }
        if let Some((p, c, f, w)) = confirm {
            self.live.sell_now(ctx, p, c, f, w);
        }
    }

    /// Kopen: een speler van een andere club naar je eerste elftal halen.
    fn page_buy(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let busy = self.live.busy() || self.live.sell_busy();
        let rate = self.settings.rate as f64;
        let eur = |pounds: i64| format!("€ {}", money((pounds as f64 * rate) as i64));
        let club = self.live.sell.club.clone();
        let squad = self.live.sell.buy_squad.clone();
        let chosen = self.live.sell.buy_player.and_then(|uid| squad.iter().find(|p| p.get("uid").and_then(|v| v.as_u64()) == Some(uid as u64)).cloned());
        let mut search = false;
        let mut load: Option<u32> = None;
        let mut pick: Option<(u32, i64, Option<i64>)> = None;
        let mut check: Option<(u32, u32, i64, Option<i64>)> = None;
        let mut confirm: Option<(u32, u32, i64, Option<i64>)> = None;

        ui.columns(2, |cols| {
            // 1. club
            theme::card(&mut cols[0], |ui| {
                ui.label(theme::h2(t!("1 · Club", "1 · Club")));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.live.sell.query).hint_text(t!("Zoek een club, bijv. Real Madrid", "Search a club, e.g. Real Madrid")).desired_width(260.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        search = true;
                    }
                    if theme::small_button(ui, t!("Zoek", "Search"), !busy && self.live.sell.query.trim().len() >= 2).clicked() {
                        search = true;
                    }
                });
                let q = self.live.sell.query.trim().to_string();
                if q.len() >= 3 && q != self.live.sell.searched && !self.live.sell_busy() {
                    search = true;
                }
                ui.add_space(4.0);
                let clubs = self.live.sell.clubs.clone();
                for c in clubs.iter().take(8) {
                    let uid = c.get("uid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                    let selected = self.live.sell.club.as_ref().is_some_and(|(u, _)| *u == uid);
                    ui.horizontal(|ui| {
                        if ui.add(egui::SelectableLabel::new(selected, RichText::new(&name).size(14.0))).clicked() {
                            self.live.sell.club = Some((uid, name.clone()));
                            self.live.sell.plan = None;
                            load = Some(uid);
                        }
                        if c.get("women").and_then(|v| v.as_bool()).unwrap_or(false) {
                            theme::pill(ui, t!("vrouwen", "women"), theme::INFO);
                        }
                    });
                }
                if clubs.is_empty() && !self.live.sell.searched.is_empty() && !self.live.sell_busy() {
                    ui.label(theme::faint(t!("Geen clubs gevonden.", "No clubs found.")));
                }
            });
            cols[0].add_space(12.0);
            // 2. speler
            theme::card(&mut cols[0], |ui| {
                ui.label(theme::h2(t!("2 · Speler", "2 · Player")));
                match &club {
                    None => {
                        ui.label(theme::faint(t!("Kies eerst een club.", "Choose a club first.")));
                    }
                    Some((_, name)) => {
                        ui.label(theme::faint(t!(format!("Eerste elftal van {name}, duurste eerst."), format!("First team of {name}, most valuable first."))));
                        if squad.is_empty() && self.live.sell_busy() {
                            ui.spinner();
                        }
                        ui.add_space(6.0);
                        ScrollArea::vertical().id_salt("buy_players").max_height(360.0).show(ui, |ui| {
                            for p in &squad {
                                let uid = p.get("uid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                let coach = p.get("player_coach").and_then(|v| v.as_bool()).unwrap_or(false);
                                let pname = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let value = p.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                                let wage = p.get("wage").and_then(|v| v.as_i64());
                                let until = p.get("contract_ends").and_then(|d| d.get("year")).and_then(|y| y.as_u64());
                                let selected = self.live.sell.buy_player == Some(uid);
                                ui.horizontal(|ui| {
                                    let label = if coach { format!("{pname} · {}", t!("speler-trainer", "player-coach")) } else { pname.to_string() };
                                    if ui.add_enabled(!coach, egui::SelectableLabel::new(selected, RichText::new(label).size(14.0))).clicked() {
                                        pick = Some((uid, value, wage));
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if let Some(y) = until {
                                            ui.label(theme::faint(format!("{} {y}", t!("tot", "until"))));
                                        }
                                        if let Some(w) = wage {
                                            ui.label(theme::faint(format!("{} p/w", eur(w))));
                                        }
                                        ui.label(RichText::new(eur(value)).color(theme::TEXT));
                                    });
                                });
                            }
                        });
                    }
                }
            });

            // 3. bedrag
            let ui = &mut cols[1];
            theme::card(ui, |ui| {
                ui.label(theme::h2(t!("3 · Bedrag", "3 · Fee")));
                ui.add_space(4.0);
                let value = chosen.as_ref().and_then(|p| p.get("value")).and_then(|v| v.as_i64()).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.live.sell.fee_input).hint_text("10").desired_width(90.0));
                    ui.label(t!("mln €", "M €"));
                    for (label, f) in [(t!("waarde", "value"), 1.0), ("×0.5", 0.5), ("×1.5", 1.5), ("0", 0.0)] {
                        if theme::small_button(ui, label, value > 0).clicked() {
                            self.live.sell.fee_input = format!("{:.1}", value as f64 * f * rate / 1e6);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(t!("Salaris bij jou", "Wage with you"));
                    ui.add(egui::TextEdit::singleline(&mut self.live.sell.wage_input).hint_text(t!("blijft", "unchanged")).desired_width(90.0));
                    ui.label("k € p/w");
                });
                let fee = self.live.sell.fee_input.replace(',', ".").trim().parse::<f64>().ok().map(|m| (m * 1e6 / rate).round() as i64);
                let wage = self.live.sell.wage_input.replace(',', ".").trim().parse::<f64>().ok().map(|k| (k * 1e3 / rate).round() as i64);
                let key = match (self.live.sell.buy_player, &self.live.sell.club, fee) {
                    (Some(p), Some((c, _)), Some(f)) if f >= 0 => Some((p, *c, f, wage)),
                    _ => None,
                };
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::button(ui, icon::MAGNIFYING_GLASS, t!("Controleer", "Check"), key.is_some() && !busy).clicked() {
                        check = key;
                    }
                    let ready = self.live.sell.plan.as_ref().is_some_and(|(v, k)| Some(*k) == key && v.get("allowed").and_then(|a| a.as_bool()) == Some(true));
                    let label = match (&chosen, &club) {
                        (Some(p), Some((_, c))) => t!(
                            format!("Koop {} van {}", p.get("name").and_then(|v| v.as_str()).unwrap_or("?"), c),
                            format!("Buy {} from {}", p.get("name").and_then(|v| v.as_str()).unwrap_or("?"), c)
                        ),
                        _ => t!("Koop", "Buy").to_string(),
                    };
                    if theme::primary_button(ui, icon::USER_PLUS, &label, ready && !busy).clicked() {
                        confirm = key;
                    }
                    if self.live.sell_busy() {
                        ui.spinner();
                    }
                });
                if let Some((v, k)) = &self.live.sell.plan {
                    if Some(*k) == key {
                        ui.add_space(8.0);
                        let s = |n: &str| v.get(n).and_then(|x| x.as_str()).unwrap_or("?").to_string();
                        let i = |n: &str| v.get(n).and_then(|x| x.as_i64()).unwrap_or(0);
                        let b = |n: &str| v.get(n).and_then(|x| x.as_bool()).unwrap_or(false);
                        let mut lines = vec![t!(
                            format!("{} komt van {} naar jouw eerste elftal voor {}.", s("player"), s("from"), eur(i("fee"))),
                            format!("{} moves from {} to your first team for {}.", s("player"), s("from"), eur(i("fee")))
                        )];
                        lines.push(if b("buyer_pays") {
                            t!(format!("{} ontvangt het geld op saldo en transferbudget.", s("from")), format!("{} receives the money on its balance and transfer budget.", s("from")))
                        } else {
                            t!(format!("{} heeft geen volledige financiën in je save: alleen jij betaalt.", s("from")), format!("{} has no full finances in your save: only you pay.", s("from")))
                        });
                        lines.push(t!(
                            format!("Jouw saldo wordt {} en je transferbudget {}.", eur(i("balance_after")), eur(i("transfer_budget_after"))),
                            format!("Your balance becomes {} and your transfer budget {}.", eur(i("balance_after")), eur(i("transfer_budget_after")))
                        ));
                        if b("was_captain") {
                            lines.push(t!(format!("Hij was aanvoerder bij {}; die band wordt daar vrij.", s("from")), format!("He was captain at {}; that armband becomes free there.", s("from"))));
                        }
                        if !b("allowed") {
                            lines.push(t!("Deze FM-versie is niet getest: sta aanpassen eerst toe in Instellingen.", "This FM build is untested: allow changes in Settings first.").into());
                        }
                        for l in lines {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(theme::ic(icon::CHECK_CIRCLE).color(theme::SUCCESS));
                                ui.label(l);
                            });
                        }
                    }
                }
            });
        });
        ui.add_space(12.0);
        theme::card(ui, |ui| {
            ui.label(theme::h2(t!("Goed om te weten", "Good to know")));
            ui.add_space(4.0);
            for l in [
                t!("Dit is een directe overgang, geen bod: FM zet de transfer niet in het nieuws of de transferhistorie.", "This is a direct move, not an offer: FM does not put the transfer in the news or the transfer history."),
                t!("De speler houdt zijn huidige contract (of krijgt het nieuwe salaris), vanaf vandaag bij jou, zonder rugnummer. Geef hem in FM een rugnummer en registreer hem waar nodig.", "The player keeps his current contract (or gets the new wage), from today at your club, without a squad number. Give him a number in FM and register him where needed."),
                t!("Ongedaan maken zet alles terug zolang je nog niet verder hebt gespeeld. Maak eerst een backup en sla pas op als alles klopt.", "Undo puts everything back as long as you have not played on. Back up first and only save when everything is right."),
                t!("Speler-trainers en gehuurde spelers kunnen (nog) niet worden overgenomen.", "Player-coaches and loaned players cannot be taken over (yet)."),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(theme::ic(icon::INFO).color(theme::INFO));
                    ui.label(theme::dim(l));
                });
            }
        });
        if search {
            self.live.search_clubs(ctx);
        }
        if let Some(uid) = load {
            self.live.load_club_squad(ctx, uid);
        }
        if let Some((uid, value, wage)) = pick {
            self.live.sell.buy_player = Some(uid);
            self.live.sell.plan = None;
            self.live.sell.fee_input = format!("{:.1}", value as f64 * rate / 1e6);
            self.live.sell.wage_input = wage.map(|w| format!("{:.1}", w as f64 * rate / 1e3)).unwrap_or_default();
        }
        if let Some((p, c, f, w)) = check {
            self.live.plan_buy(ctx, p, c, f, w);
        }
        if let Some((p, c, f, w)) = confirm {
            self.live.buy_now(ctx, p, c, f, w);
        }
    }

    // -------------------------------------------------------------------- instellingen

    fn page_settings(&mut self, ui: &mut Ui) {
        theme::page_header(ui, t!("Instellingen", "Settings"), "", |_| {});
        let mut dirty = false;
        ui.columns(2, |cols| {
            theme::card(&mut cols[0], |ui| {
                ui.label(theme::h2(t!("Algemeen", "General")));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(theme::ic(icon::TRANSLATE).color(theme::ACCENT));
                    ui.label(t!("Taal", "Language"));
                    let mut lang = self.settings.lang;
                    if theme::segmented(ui, &mut lang, &[(Lang::Nl, "Nederlands"), (Lang::En, "English")]) {
                        self.settings.lang = lang;
                        i18n::set(lang);
                        dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(theme::ic(icon::COINS).color(theme::ACCENT));
                    ui.label(t!("Koers € per £", "Rate € per £"));
                    dirty |= ui.add(egui::DragValue::new(&mut self.settings.rate).range(0.5..=2.0).speed(0.01).fixed_decimals(2)).changed();
                });
                dirty |= ui.checkbox(&mut self.settings.capture_delay, t!("FM eerst naar voren halen bij uitlezen", "Bring FM to front before reading")).changed();
            });
            theme::card(&mut cols[1], |ui| {
                ui.label(theme::h2("Planner"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(t!("Team Bonding onder Buy-In", "Team Bonding below Buy-In"));
                    dirty |= ui.add(egui::Slider::new(&mut self.settings.planner.bonding_below, 0.0..=100.0).fixed_decimals(0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label(t!("Standaardsituaties elke (weken, 0 = uit)", "Set pieces every (weeks, 0 = off)"));
                    dirty |= ui.add(egui::DragValue::new(&mut self.settings.planner.set_pieces_every).range(0..=8)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label(t!("Doelen bijstellen elke (weken)", "Re-target every (weeks)"));
                    dirty |= ui.add(egui::DragValue::new(&mut self.settings.planner.replan_every_weeks).range(1..=8)).changed();
                });
            });
        });
        ui.add_space(14.0);
        if agent::LIVE_SUPPORTED {
            theme::card(ui, |ui| {
                ui.label(theme::h2(t!("Live in FM", "Live in FM")));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let mut on = self.settings.allow_untested;
                    if theme::toggle(ui, &mut on).changed() {
                        self.settings.allow_untested = on;
                        self.live.allow_untested = on;
                        dirty = true;
                        if self.live.connected {
                            self.live.refresh(ui.ctx(), "", None);
                        }
                    }
                    ui.label(RichText::new(t!("Aanpassen op een niet-geteste FM-versie", "Allow changes on an untested FM build")).strong());
                });
                ui.add(egui::Label::new(theme::dim(t!(
                    "FM27 Coach past alleen iets aan op FM-versies die het kent (Steam op Windows en Mac, Game Pass / Xbox-app op Windows). Op andere versies (na een FM-update, of bijvoorbeeld Epic) staat de app op alleen lezen. Kloppen club, budget en selectie in de app met wat FM laat zien? Dan kun je aanpassen hier toestaan. Elke wijziging wordt eerst gecontroleerd, kan ongedaan worden gemaakt en blijft pas bewaard als je in FM opslaat. Maak eerst een backup van je save.",
                    "FM27 Coach only changes things on FM builds it knows (Steam on Windows and Mac, Game Pass / Xbox app on Windows). On other builds (after an FM update, or Epic for example) the app is read-only. Do club, budget and squad in the app match what FM shows? Then you can allow changes here. Every change is checked first, can be undone and only sticks once you save in FM. Back up your save first."
                )))
                .wrap());
                if self.live.connected {
                    ui.add_space(4.0);
                    let (c, txt) = if !self.live.untested_build {
                        (theme::SUCCESS, t!("Deze FM-versie is getest.", "This FM build is tested."))
                    } else if self.settings.allow_untested {
                        (theme::WARNING, t!("Niet-geteste FM-versie: aanpassen toegestaan.", "Untested FM build: changes allowed."))
                    } else {
                        (theme::WARNING, t!("Niet-geteste FM-versie: alleen lezen.", "Untested FM build: read-only."))
                    };
                    ui.horizontal(|ui| {
                        theme::status_dot(ui, c);
                        ui.label(RichText::new(txt).color(theme::TEXT));
                    });
                }
            });
            ui.add_space(14.0);
        }
        theme::card(ui, |ui| {
            ui.label(theme::h2(t!("Mappen en toestemming", "Folders and permissions")));
            ui.add_space(6.0);
            ui.label(theme::dim(format!("Export: {}", export::output_dir().display())));
            ui.label(theme::dim(format!("{}: {}", t!("Instellingen", "Settings"), crate::data_dir().display())));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::button(ui, icon::EXPORT, t!("Open exportmap", "Open export folder"), true).clicked() {
                    let _ = std::fs::create_dir_all(export::output_dir());
                    open_path(&export::output_dir());
                }
                // daar staat ook de laatste schermopname (last-breakdown.png): handig om mee te sturen
                // als een meter verkeerd gelezen wordt
                if theme::button(ui, icon::EXPORT, t!("Open app-map (laatste schermopname en uitlezing)", "Open app folder (last screenshot and reading)"), true).clicked() {
                    let _ = std::fs::create_dir_all(crate::data_dir());
                    open_path(&crate::data_dir());
                }
                if cfg!(target_os = "macos") && theme::button(ui, icon::CAMERA, t!("Schermopname-toestemming", "Screen Recording permission"), true).clicked() {
                    open_path(std::path::Path::new("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"));
                }
            });
        });
        ui.add_space(14.0);
        theme::card(ui, |ui| {
            ui.label(theme::h2(t!("Over", "About")));
            ui.add(egui::Label::new(theme::dim(t!(
                "Sessienamen, limieten en presets komen uit FM27's eigen game-data. De module leest en schrijft alleen na controle, met undo. Alleen voor single-player. Lettertype: Inter (SIL OFL). Iconen: Phosphor (MIT).",
                "Session names, limits and presets come from FM27's own game data. The module only reads and writes after checks, with undo. Single-player only. Font: Inter (SIL OFL). Icons: Phosphor (MIT)."
            )))
            .wrap());
            ui.add_space(4.0);
            ui.hyperlink_to(
                t!("Gebaseerd op FM27 Trainer door SemHiel2003 (MIT-licentie)", "Based on FM27 Trainer by SemHiel2003 (MIT licence)"),
                "https://github.com/SemHiel2003/fm27-trainer",
            );
        });
        if dirty {
            self.save_settings();
            self.week_plan = None;
            self.year.clear();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.snapshot(ctx);
        self.poll_analysis(ctx);
        self.live.allow_untested = self.settings.allow_untested;
        self.live.keep_understanding = self.settings.keep_understanding;
        self.live.tick(ctx, &self.eff, &self.settings.planner, self.settings.auto_training);
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin { left: 28, right: 28, top: 24, bottom: 16 }))
            .show(ctx, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(1400.0));
                    match self.page {
                        Page::Overview => self.page_overview(ui, ctx),
                        Page::Training => self.page_training(ui, ctx),
                        Page::Planning => self.page_planning(ui),
                        Page::Club => self.page_club(ui, ctx),
                        Page::Squad => self.page_squad(ui, ctx),
                        Page::Sell => self.page_sell(ui, ctx),
                        Page::Settings => self.page_settings(ui),
                    }
                    ui.add_space(20.0);
                });
            });
        if self.analysis_rx.is_some() || self.live.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }
}

// ------------------------------------------------------------------------ helpers

fn open_path(p: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).status();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).status();
}

fn money(v: i64) -> String {
    let a = v.unsigned_abs() as f64;
    let s = if a >= 1e6 { format!("{:.1} M", a / 1e6) } else if a >= 1e3 { format!("{:.0}k", a / 1e3) } else { format!("{a:.0}") };
    if v < 0 { format!("-{s}") } else { s }
}

/// De acht onderdelen van FM's Tactical Understanding, in de volgorde van de module.
const UNDERSTANDING_AREAS: [&str; 8] = ["Passing Style", "Build Up", "Chance Creation", "In Possession Shape", "Line of Engagement", "Pressing Intensity", "Pressing Trap", "Out Of Possession Shape"];

fn morale_label(m: u64) -> &'static str {
    match m {
        0..=4 => "Abysmal",
        5..=7 => "Very Poor",
        8..=10 => "Poor",
        11..=13 => "Okay",
        14..=16 => "Good",
        17..=18 => "Very Good",
        _ => "Superb",
    }
}

/// Korte, leesbare sessienaam voor in chips.
fn chip_name(name: &str) -> String {
    name.replace("Out Of Possession", "OOP")
        .replace("In Possession", "IP")
        .replace("Defensive", "Def.")
        .replace("Attacking", "Att.")
        .replace("Possession Ball Work", "Poss. Ball Work")
        .replace("Conditioning", "Cond.")
        .replace("Individual Development", "Individual")
        .replace("Line of Engagement", "Line of Eng.")
        .replace("Pressing Intensity", "Press. Intensity")
}

fn chip(ui: &mut Ui, name: &str, highlight: bool, faded: bool) {
    let col = if name == "Match" { theme::DANGER } else { theme::rgb(category_of(name).color()) };
    let col = if faded { theme::tint(col, 0.45) } else { col };
    theme::session_chip(ui, &chip_name(name), col, highlight).on_hover_text(name);
}

fn step_card(ui: &mut Ui, n: u8, icon_s: &str, title: &str, body: impl FnOnce(&mut Ui)) {
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            Frame::new().fill(theme::ACCENT_SOFT).corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(n.to_string()).color(theme::ACCENT).strong());
            });
            ui.label(theme::ic(icon_s).color(theme::ACCENT).size(16.0));
            ui.label(theme::h2(title));
        });
        ui.add_space(4.0);
        body(ui);
    });
}

fn codes_text(c: &[u16; 3]) -> String {
    c.iter().map(|x| chip_name(session_by_code(*x).unwrap_or("?"))).collect::<Vec<_>>().join(" · ")
}

fn week_card(ui: &mut Ui, w: &live_training::WeekPreview, today: Option<NaiveDate>, fm_had: &std::collections::HashMap<NaiveDate, [u16; 3]>) {
    theme::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::h2(t!(format!("Week van {}", i18n::date_short(w.start)), format!("Week of {}", i18n::date_short(w.start)))));
            if w.rest {
                theme::pill(ui, t!("Rustweek: blijft van FM", "Rest week: kept from FM"), theme::WARNING);
                return;
            }
            theme::pill(ui, &t!(format!("{} wedstrijd(en)", w.matches), format!("{} match(es)", w.matches)), theme::DANGER);
            theme::pill(ui, &w.focus, theme::ACCENT);
            let pending = (0..7).filter(|d| w.current[*d] != w.planned[*d]).count();
            if w.written > 0 {
                theme::pill(ui, &t!(format!("{} dagen teruggezet", w.written), format!("{} days restored", w.written)), theme::SUCCESS);
            } else if pending > 0 {
                theme::pill(ui, &t!(format!("{pending} dagen wijken af van je plan"), format!("{pending} days differ from your plan")), theme::WARNING);
            } else {
                theme::pill(ui, t!("volgens plan", "on plan"), theme::SUCCESS);
            }
        });
        if w.rest {
            return;
        }
        ui.add_space(8.0);
        ui.columns(7, |cols| {
            for d in 0..7 {
                let ui = &mut cols[d];
                let date = w.start + Duration::days(d as i64);
                let past = today.is_some_and(|t| date <= t);
                ui.label(theme::caps(i18n::weekday_short(d)));
                ui.label(theme::faint(i18n::date_short(date)));
                // wat FM wilde: nu afwijkend (wordt aangepast) of eerder teruggezet
                let differs = w.current[d] != w.planned[d];
                let was = if differs { Some(w.current[d]) } else { fm_had.get(&date).copied().filter(|h| *h != w.planned[d]) };
                for code in w.planned[d] {
                    let name = session_by_code(code).unwrap_or("?");
                    chip(ui, name, was.is_some() && !past, past || !w.editable[d]);
                }
                if let Some(h) = was.filter(|_| !past && w.editable[d]) {
                    ui.add(egui::Label::new(RichText::new(format!("FM: {}", codes_text(&h))).size(10.5).color(theme::TEXT_FAINT).strikethrough()).truncate())
                        .on_hover_text(t!(
                            format!("FM had ingepland: {}", codes_text(&h)),
                            format!("FM had planned: {}", codes_text(&h))
                        ));
                }
            }
        });
    });
}

fn plan_grid(ui: &mut Ui, p: &WeekPlan) {
    ui.columns(7, |cols| {
        for (i, d) in p.days.iter().enumerate() {
            let ui = &mut cols[i];
            ui.label(theme::caps(i18n::weekday_short(i)));
            ui.label(theme::faint(format!("{} · {}", i18n::date_short(d.date), d.role.label())));
            for s in &d.slots {
                chip(ui, s, false, false);
            }
        }
    });
}

fn legend(ui: &mut Ui) {
    use crate::model::Category::*;
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::faint(t!("Omlijnd = door FM27 Coach gezet · doorgestreept = wat FM wilde ·", "Outlined = set by FM27 Coach · struck through = what FM wanted ·")));
        for c in [TacticalUnderstanding, MatchPreparation, FootballingDevelopment, PhysicalRecovery, ExtraCurricular, Fixed] {
            theme::status_dot(ui, theme::rgb(c.color()));
            ui.label(theme::faint(c.label()));
            ui.add_space(8.0);
        }
    });
}

fn kind_combo(ui: &mut Ui, id: &str, kind: &mut WeekKind) {
    egui::ComboBox::from_id_salt(id).selected_text(kind.label()).width(170.0).show_ui(ui, |ui| {
        for k in WeekKind::ALL {
            ui.selectable_value(kind, k, k.label());
        }
    });
}

fn weekday_combo(ui: &mut Ui, id: &str, day: &mut Weekday) {
    let names = [(Weekday::Fri, t!("vrijdag", "Friday")), (Weekday::Sat, t!("zaterdag", "Saturday")), (Weekday::Sun, t!("zondag", "Sunday"))];
    let cur = names.iter().find(|(d, _)| d == day).map(|(_, n)| *n).unwrap_or("—");
    egui::ComboBox::from_id_salt(id).selected_text(cur).show_ui(ui, |ui| {
        for (d, n) in names {
            ui.selectable_value(day, d, n);
        }
    });
}
