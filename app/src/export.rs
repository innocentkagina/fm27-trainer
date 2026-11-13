//! Export van week- en jaarplannen: HTML (leesbaar/printbaar), CSV en JSON.

use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::model::{Component, Effectiveness, Factor, category_of, level_color, level_name};
use crate::planner::{WeekPlan, short, unique_schedules};

fn day(i: usize) -> &'static str {
    crate::i18n::weekday_short(i)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn rgb(c: [u8; 3]) -> String {
    format!("rgb({},{},{})", c[0], c[1], c[2])
}

fn nl_date(d: NaiveDate) -> String {
    crate::i18n::date(d)
}

fn session_cell(name: &str) -> String {
    if name.is_empty() {
        return "<td></td>".into();
    }
    let c = category_of(name).color();
    format!(
        "<td><span class=\"s\" style=\"--c:{}\">{}</span></td>",
        rgb(c),
        esc(name)
    )
}

fn week_table(p: &WeekPlan) -> String {
    let mut h = format!("<table class=\"wk\"><thead><tr><th>{}</th><th>{}</th><th>Session 1</th><th>Session 2</th><th>Extra</th></tr></thead><tbody>", crate::t!("dag", "day"), crate::t!("type", "type"));
    for (i, d) in p.days.iter().enumerate() {
        let _ = write!(
            h,
            "<tr><td class=\"d\">{} {}</td><td class=\"r\">{}</td>{}{}{}</tr>",
            day(i),
            d.date.format("%d-%m"),
            d.role.label(),
            session_cell(&d.slots[0]),
            session_cell(&d.slots[1]),
            session_cell(&d.slots[2])
        );
    }
    h.push_str("</tbody></table>");
    h
}

const CSS: &str = r#"
:root{--bg:#f6f5f2;--fg:#1d1d1f;--mut:#6b6b70;--card:#fff;--line:#e3e1dc;--acc:#c8102e}
@media (prefers-color-scheme:dark){:root{--bg:#141416;--fg:#ececef;--mut:#9a9aa2;--card:#1d1d21;--line:#2c2c32;--acc:#ff4d5e}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.45 -apple-system,BlinkMacSystemFont,"Inter",sans-serif}
main{max-width:1100px;margin:0 auto;padding:28px 16px 60px}
h1{font-size:28px;margin:0 0 4px}h2{font-size:19px;margin:36px 0 12px}h3{font-size:15px;margin:0 0 8px}
.sub{color:var(--mut);margin:0 0 20px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px}
.bar{height:8px;border-radius:4px;background:var(--line);overflow:hidden;margin:4px 0 8px}.bar i{display:block;height:100%}
.f{display:flex;justify-content:space-between;font-size:13px}.f b{font-variant-numeric:tabular-nums}
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--line);font-size:13px;vertical-align:top}
th{color:var(--mut);font-weight:600}
.wk .d{white-space:nowrap;color:var(--mut)}.wk .r{white-space:nowrap}
.s{display:inline-block;padding:1px 7px;border-radius:6px;border-left:4px solid var(--c);background:color-mix(in srgb,var(--c) 14%,transparent)}
.wk td{padding:4px 5px}.wk .r{color:var(--mut);font-size:12px}
.legend span{margin-right:10px;font-size:12px}.legend i{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:4px;vertical-align:-1px}
.cal td{white-space:nowrap}.m{display:inline-block;width:16px;height:16px;border-radius:3px;background:var(--line);margin-right:2px;font-size:10px;text-align:center;line-height:16px;color:var(--mut)}
.m.on{background:var(--acc);color:#fff}
.note{font-size:13px;color:var(--mut)}
.scroll{overflow-x:auto}
svg text{fill:var(--mut);font-size:10px}
@media print{body{background:#fff;color:#000}.card{break-inside:avoid}}
"#;

fn legend() -> String {
    use crate::model::Category::*;
    let mut h = String::from("<div class=\"legend\">");
    for c in [TacticalUnderstanding, MatchPreparation, FootballingDevelopment, PhysicalRecovery, ExtraCurricular, Fixed] {
        let _ = write!(h, "<span><i style=\"background:{}\"></i>{}</span>", rgb(c.color()), c.label());
    }
    h.push_str("</div>");
    h
}

fn effectiveness_cards(eff: &Effectiveness) -> String {
    let mut h = String::from("<div class=\"grid\">");
    for c in Component::ALL {
        let v = eff.component(c);
        let _ = write!(
            h,
            "<div class=\"card\"><h3>{}</h3><div class=\"f\"><span>{}</span><b>{:.0}</b></div><div class=\"bar\"><i style=\"width:{:.0}%;background:{}\"></i></div>",
            c.label(),
            crate::t!("onderdeel", "component"),
            v,
            v,
            rgb(level_color(v))
        );
        for f in Factor::ALL.iter().filter(|f| f.component() == c) {
            let v = eff.get(*f);
            let _ = write!(
                h,
                "<div class=\"f\"><span>{}</span><b>{:.0}</b></div><div class=\"bar\"><i style=\"width:{:.0}%;background:{}\"></i></div>",
                f.label(),
                v,
                v,
                rgb(level_color(v))
            );
        }
        h.push_str("</div>");
    }
    h.push_str("</div>");
    h
}

fn projection_svg(plans: &[WeekPlan], start: &Effectiveness) -> String {
    if plans.is_empty() {
        return String::new();
    }
    let w = 1000.0;
    let hgt = 220.0;
    let n = plans.len() as f32;
    let colors = ["#e4572e", "#f3a712", "#a8c256", "#29335c", "#669bbc", "#c8102e", "#8e44ad", "#16a085"];
    let mut svg = format!("<svg viewBox=\"0 -10 {w} {}\" width=\"100%\" style=\"overflow:visible\" role=\"img\" aria-label=\"Geschatte ontwikkeling Tactical Areas\">", hgt + 24.0);
    for y in [0, 25, 50, 75, 100] {
        let yy = hgt - y as f32 / 100.0 * hgt;
        let _ = write!(svg, "<line x1=\"30\" x2=\"{w}\" y1=\"{yy}\" y2=\"{yy}\" stroke=\"currentColor\" stroke-opacity=\".12\"/><text x=\"0\" y=\"{}\">{y}</text>", yy + 3.0);
    }
    for (k, f) in Factor::TACTICAL_AREAS.iter().enumerate() {
        let mut pts = format!("30,{:.1} ", hgt - start.get(*f) / 100.0 * hgt);
        for (i, p) in plans.iter().enumerate() {
            let v = p.projected.iter().find(|(x, _)| x == f).map(|(_, v)| *v).unwrap_or(0.0);
            let x = 30.0 + (i as f32 + 1.0) / n * (w - 40.0);
            let _ = write!(pts, "{:.1},{:.1} ", x, hgt - v / 100.0 * hgt);
        }
        let _ = write!(svg, "<polyline fill=\"none\" stroke=\"{}\" stroke-width=\"2\" points=\"{}\"/>", colors[k], pts.trim());
    }
    // maandlabels
    let mut last_month = 0;
    for (i, p) in plans.iter().enumerate() {
        if p.start.month() != last_month {
            last_month = p.start.month();
            let x = 30.0 + (i as f32 + 0.5) / n * (w - 40.0);
            let _ = write!(svg, "<text x=\"{x:.0}\" y=\"{}\">{}</text>", hgt + 16.0, p.start.format("%m/%y"));
        }
    }
    svg.push_str("</svg><div class=\"legend\">");
    for (k, f) in Factor::TACTICAL_AREAS.iter().enumerate() {
        let _ = write!(svg, "<span><i style=\"background:{}\"></i>{}</span>", colors[k], f.label());
    }
    svg.push_str("</div>");
    svg
}

pub fn html_report(title: &str, eff: &Effectiveness, plans: &[WeekPlan]) -> String {
    let mut h = String::new();
    let _ = write!(
        h,
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style></head><body><main>",
        crate::t!("nl", "en"),
        esc(title),
        CSS
    );
    let _ = write!(
        h,
        "<h1>{}</h1><p class=\"sub\">{} {} · {}: {} · {} {}</p>",
        esc(title),
        crate::t!("Gemaakt", "Created"),
        chrono::Local::now().format("%d-%m-%Y %H:%M"),
        crate::t!("bron", "source"),
        esc(&eff.source),
        plans.len(),
        crate::t!("weken", "weeks")
    );

    h.push_str(crate::t!("<h2>Tactical Effectiveness nu</h2>", "<h2>Tactical Effectiveness now</h2>"));
    h.push_str(&effectiveness_cards(eff));

    h.push_str(crate::t!("<h2>Prioriteiten</h2><div class=\"card\"><table><tbody>", "<h2>Priorities</h2><div class=\"card\"><table><tbody>"));
    for (f, v) in eff.weakest().into_iter().take(8) {
        let _ = write!(
            h,
            "<tr><td><b>{}</b><br><span class=\"note\">{} · {}</span></td><td style=\"width:60%\">{}</td></tr>",
            f.label(),
            level_name(v),
            format!("{v:.0}"),
            esc(f.advice())
        );
    }
    h.push_str("</tbody></table></div>");

    if plans.len() > 1 {
        h.push_str(crate::t!("<h2>Geschatte ontwikkeling</h2><div class=\"card\">", "<h2>Projected development</h2><div class=\"card\">"));
        h.push_str(&projection_svg(plans, eff));
        h.push_str(crate::t!("<p class=\"note\">Grove schatting (winst per sessie, verval als een gebied niet getraind wordt). Lees de Breakdown elke 2–4 weken opnieuw uit en maak het plan opnieuw.</p></div>", "<p class=\"note\">Rough estimate (gain per session, decay when an area is not trained). Re-read the Breakdown every 2–4 weeks and regenerate the plan.</p></div>"));
    }

    let uniq = unique_schedules(plans);
    let _ = write!(h, "<h2>{} ({})</h2>", crate::t!("Weekschema's", "Weekly schedules"), uniq.len());
    h.push_str(crate::t!("<p class=\"note\">Met de module zet FM27 Coach deze schema's automatisch in je kalender. Zonder module: maak ze aan in FM (Training → Create new schedule).</p>", "<p class=\"note\">With the module, FM27 Coach writes these schedules into your calendar automatically. Without it: create them in FM (Training → Create new schedule).</p>"));
    h.push_str(&legend());
    h.push_str("<div class=\"grid\" style=\"grid-template-columns:repeat(auto-fit,minmax(min(100%,520px),1fr))\">");
    for (name, dates, p) in &uniq {
        let weeks: Vec<String> = dates.iter().take(6).map(|d| d.format("%d-%m").to_string()).collect();
        let more = if dates.len() > 6 { format!(" +{}", dates.len() - 6) } else { String::new() };
        let _ = write!(
            h,
            "<div class=\"card\"><h3>{}</h3><p class=\"note\">{} · focus {} · {} {} · {}: {}{}</p><div class=\"scroll\">{}</div></div>",
            esc(name),
            p.kind.label(),
            p.focus,
            p.matches,
            crate::t!("wedstrijd(en)", "match(es)"),
            crate::t!("weken", "weeks"),
            weeks.join(", "),
            more,
            week_table(p)
        );
    }
    h.push_str("</div>");

    h.push_str(crate::t!("<h2>Kalender</h2><div class=\"card scroll\"><table class=\"cal\"><thead><tr><th>week van</th><th>periode</th><th>wedstrijden</th><th>focus</th><th>schema</th><th>tactische sessies</th></tr></thead><tbody>", "<h2>Calendar</h2><div class=\"card scroll\"><table class=\"cal\"><thead><tr><th>week of</th><th>period</th><th>matches</th><th>focus</th><th>schedule</th><th>tactical sessions</th></tr></thead><tbody>"));
    for p in plans {
        let mut m = String::new();
        for (i, d) in p.days.iter().enumerate() {
            let on = d.slots.iter().any(|s| s == "Match");
            let _ = write!(m, "<span class=\"m{}\">{}</span>", if on { " on" } else { "" }, &day(i)[..1]);
        }
        let t: Vec<String> = p.targets.iter().map(|(f, n)| format!("{}×{}", short(*f), n)).collect();
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            nl_date(p.start),
            p.kind.label(),
            m,
            p.focus,
            esc(&p.schedule_name),
            t.join(" ")
        );
    }
    h.push_str("</tbody></table></div>");

    if let Some(p) = plans.first() {
        if !p.notes.is_empty() {
            h.push_str(crate::t!("<h2>Buiten de training</h2><div class=\"card\"><ul>", "<h2>Beyond training</h2><div class=\"card\"><ul>"));
            for n in &p.notes {
                let _ = write!(h, "<li>{}</li>", esc(n));
            }
            h.push_str("</ul></div>");
        }
    }
    h.push_str(crate::t!("<p class=\"note\" style=\"margin-top:30px\">Sessienamen en limieten komen uit de game-data van FM27. Het ritme rond wedstrijden volgt de presets van Sports Interactive.</p></main></body></html>", "<p class=\"note\" style=\"margin-top:30px\">Session names and limits come from FM27's game data. The rhythm around matches follows Sports Interactive's presets.</p></main></body></html>"));
    h
}

pub fn csv(plans: &[WeekPlan]) -> String {
    let mut s = String::from(crate::t!("datum,dag,periode,type,sessie1,sessie2,extra,schema\n", "date,day,period,type,session1,session2,extra,schedule\n"));
    for p in plans {
        for (i, d) in p.days.iter().enumerate() {
            let _ = writeln!(
                s,
                "{},{},{},{},{},{},{},\"{}\"",
                d.date,
                day(i),
                p.kind.label(),
                d.role.label(),
                d.slots[0],
                d.slots[1],
                d.slots[2],
                p.schedule_name
            );
        }
    }
    s
}

pub fn output_dir() -> PathBuf {
    crate::home().join("Documents").join("FM27 Coach")
}

/// Schrijft HTML, CSV en JSON; geeft het pad van de HTML terug.
pub fn write_all(dir: &Path, stem: &str, title: &str, eff: &Effectiveness, plans: &[WeekPlan]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let html = dir.join(format!("{stem}.html"));
    std::fs::write(&html, html_report(title, eff, plans))?;
    std::fs::write(dir.join(format!("{stem}.csv")), csv(plans))?;
    std::fs::write(
        dir.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&serde_json::json!({ "effectiveness": eff, "weeks": plans }))?,
    )?;
    Ok(html)
}
