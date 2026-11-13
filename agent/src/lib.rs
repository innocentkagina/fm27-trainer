//! FM27 Trainer-module: leest en schrijft (op verzoek) in het geheugen van FM27. Doet niets uit
//! zichzelf en schrijft alleen na controle van wat er nu in het geheugen staat (met undo).
//!
//! - macOS: de app start FM27 met deze bibliotheek (DYLD_INSERT_LIBRARIES; FM27 staat dat toe
//!   via zijn entitlements, zie FMSuperScout docs/macos.md). Ze luistert op een lokale socket.
//! - Windows: de app gebruikt dezelfde code rechtstreeks (`request`) en leest fm.exe van buitenaf.
#![cfg(any(target_os = "macos", windows))]

pub mod game;
mod mem;
mod rtti;
mod sell;
#[cfg(test)]
mod tests;
pub mod understanding;

use serde_json::{Value, json};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---- macOS: module in FM, socket naar de app ---------------------------------------------

#[cfg(all(target_os = "macos", not(test)))]
mod server {
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::Duration;

    fn data_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join("Library/Application Support/FM27 Coach")
    }

    fn log(msg: &str) {
        let p = data_dir().join("agent.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let _ = writeln!(f, "{t} {msg}");
        }
    }

    #[used]
    #[unsafe(link_section = "__DATA,__mod_init_func")]
    static INIT: extern "C" fn() = init;

    extern "C" fn init() {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        // alleen in FM zelf, niet in hulpprocessen die de variabele erven
        if !exe.contains(".app/Contents/MacOS/Football Manager 2") {
            return;
        }
        unsafe { std::env::remove_var("DYLD_INSERT_LIBRARIES") };
        let _ = std::fs::create_dir_all(data_dir());
        log(&format!("geladen in {exe} (pid {})", std::process::id()));
        let _ = std::thread::Builder::new().name("fm27-trainer".into()).stack_size(8 << 20).spawn(|| {
            // wachten tot FM zelf is opgestart
            std::thread::sleep(Duration::from_secs(8));
            serve();
        });
    }

    fn serve() {
        let sock = data_dir().join("agent.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                log(&format!("socket mislukt: {e}"));
                return;
            }
        };
        log("luistert");
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            handle(stream);
        }
    }

    fn handle(stream: UnixStream) {
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => match super::guarded(&req) {
                Ok(v) => json!({"ok": true, "data": v}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            Err(e) => json!({"ok": false, "error": format!("ongeldig verzoek: {e}")}),
        };
        let mut s = stream;
        let _ = writeln!(s, "{reply}");
    }
}

// ---- Windows: de app roept dit rechtstreeks aan ------------------------------------------

/// pid van fm.exe als FM27 draait.
#[cfg(windows)]
pub fn fm_pid() -> Option<u32> {
    mem::find_fm()
}

/// Voert één opdracht uit tegen fm.exe (verbindt eerst, of opnieuw na een herstart van FM).
/// Opdrachten lopen na elkaar, zoals op de Mac via de socket.
#[cfg(windows)]
pub fn request(req: &Value) -> Result<Value, String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_, fresh) = mem::attach()?;
    if fresh {
        game::reset();
    }
    guarded(req)
}

/// `dispatch`, maar een paniek wordt een foutmelding in plaats van een crash.
fn guarded(req: &Value) -> Result<Value, String> {
    std::panic::catch_unwind(|| dispatch(req)).unwrap_or_else(|_| Err("interne fout in de module".into()))
}

fn uids(req: &Value) -> Option<Vec<u32>> {
    req.get("uids").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("oneven hex".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

fn addr(req: &Value, key: &str) -> Result<u64, String> {
    let v = req.get(key).ok_or(format!("{key} ontbreekt"))?;
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    let s = v.as_str().ok_or(format!("{key} ongeldig"))?;
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
}

pub fn dispatch(req: &Value) -> Result<Value, String> {
    let cmd = req.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
    match cmd {
        "ping" => {
            let (id, tested) = game::build();
            Ok(json!({
                "pid": mem::pid(),
                "version": VERSION,
                "build": id,
                "known_build": tested,
                "build_label": game::build_label(),
                // waar de vaste plekken vandaan komen: "profile" (bekende build), "discovered"
                // (zelf opgezocht op een onbekende build) of null (nog niets gevonden)
                "statics": game::statics_source(),
                "platform": if mem::abi() == mem::Abi::Msvc { "windows" } else { "macos" },
                "untested_writes": game::allow_untested(),
            }))
        }
        "allow_untested" => {
            let on = req.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
            game::set_allow_untested(on);
            Ok(json!({"untested_writes": on}))
        }
        "club" => game::club().map(|c| json!(c)),
        "set_finances" => {
            let g = |k: &str| req.get(k).and_then(|v| v.as_i64());
            game::set_finances(g("transfer_budget"), g("wage_budget"), g("balance")).map(|c| json!(c))
        }
        "squad" => game::squad().map(|s| json!(s)),
        "heal" => game::heal(&uids(req)).map(|n| json!({"players": n})),
        "lift_bans" => game::lift_bans(&uids(req)).map(|n| json!({"bans": n})),
        "inspire" => {
            let morale = req.get("morale").and_then(|v| v.as_u64()).unwrap_or(20) as u8;
            let clear = req.get("clear_unhappiness").and_then(|v| v.as_bool()).unwrap_or(true);
            game::inspire(&uids(req), morale, clear).map(|n| json!({"players": n}))
        }
        "game_date" => game::game_date().map(|d| json!(d)),
        "training_weeks" => game::training_weeks().map(|(today, w)| json!({"today": today, "weeks": w})),
        "set_week" => {
            let start: game::FmDate = serde_json::from_value(req.get("start").cloned().ok_or("start ontbreekt")?).map_err(|e| e.to_string())?;
            let days: Vec<[u16; 3]> = serde_json::from_value(req.get("days").cloned().ok_or("days ontbreekt")?).map_err(|e| e.to_string())?;
            game::set_week(start, &days).map(|n| json!({"days": n}))
        }
        "undo" => game::undo_last().map(|l| json!({"undone": l})),
        "human_flags" => game::human_flags().map(|f| json!(f)),
        "set_human_flags" => {
            let b = |k: &str| req.get(k).and_then(|v| v.as_bool());
            game::set_human_flags(b("registration_anytime"), b("numbers_anytime")).map(|f| json!(f))
        }
        "understanding" => understanding::understanding().map(|u| json!(u)),
        "max_understanding" => {
            let dry = req.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            understanding::maximise(&uids(req), dry).map(|m| json!(m))
        }
        "clubs" => {
            let q = req.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = req.get("limit").and_then(|v| v.as_u64()).unwrap_or(20).min(100) as usize;
            sell::find_clubs(q, limit).map(|c| json!(c))
        }
        "sell" | "buy" => {
            let n = |k: &str| req.get(k).and_then(|v| v.as_i64());
            let uid = n("uid").ok_or("uid ontbreekt")? as u32;
            let club = n("club").ok_or("club ontbreekt")? as u32;
            let fee = n("fee").ok_or("fee ontbreekt")?;
            let dry = req.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            if cmd == "buy" { sell::buy(uid, club, fee, n("wage"), dry) } else { sell::sell(uid, club, fee, n("wage"), dry) }.map(|s| json!(s))
        }
        "club_squad" => {
            let uid = req.get("club").and_then(|v| v.as_u64()).ok_or("club ontbreekt")? as u32;
            sell::club_squad(uid).map(|s| json!(s))
        }
        // ---- onderzoek (alleen lezen) -------------------------------------------------
        "chain" => game::chain().map(|c| {
            json!({"human": format!("{:#x}", c.human), "contract": format!("{:#x}", c.contract), "team": format!("{:#x}", c.team), "club": format!("{:#x}", c.club), "finance": format!("{:#x}", c.finance)})
        }),
        "discover" => game::discover_report(),
        "census" => {
            let img = mem::game_plugin().ok_or("game_plugin niet geladen")?;
            let filter = req.get("filter").and_then(|v| v.as_str()).unwrap_or("TRAIN");
            let classes = rtti::census(&img, filter, 4000);
            let count = req.get("count").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut out = Vec::new();
            if count {
                let all: Vec<u64> = classes.iter().flat_map(|(_, v)| v.clone()).collect();
                let set: std::collections::HashSet<u64> = all.iter().copied().collect();
                let hits = mem::scan_u64(&set, 2_000_000);
                for (name, vts) in &classes {
                    let objs: Vec<String> = hits.iter().filter(|(_, v)| vts.contains(v)).take(8).map(|(a, _)| format!("{a:#x}")).collect();
                    let n = hits.iter().filter(|(_, v)| vts.contains(v)).count();
                    out.push(json!({"class": rtti::demangle(name), "instances": n, "first": objs}));
                }
            } else {
                for (name, vts) in &classes {
                    out.push(json!({"class": rtti::demangle(name), "vtables": vts.iter().map(|v| format!("{v:#x}")).collect::<Vec<_>>()}));
                }
            }
            Ok(json!(out))
        }
        "class_of" => {
            let a = addr(req, "addr")?;
            Ok(json!(rtti::class_name(a).map(|n| rtti::demangle(&n))))
        }
        "read" => {
            let a = addr(req, "addr")?;
            let len = req.get("len").and_then(|v| v.as_u64()).unwrap_or(256).min(1 << 16) as usize;
            let b = mem::read_bytes(a, len).ok_or("niet leesbaar")?;
            Ok(json!({"addr": format!("{a:#x}"), "hex": hex(&b)}))
        }
        "find_u16" => {
            let pat: Vec<u16> = req
                .get("pattern")
                .and_then(|v| v.as_array())
                .ok_or("pattern ontbreekt")?
                .iter()
                .filter_map(|x| x.as_u64().map(|n| n as u16))
                .collect();
            let bytes: Vec<u8> = pat.iter().flat_map(|x| x.to_le_bytes()).collect();
            let limit = req.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
            let hits = mem::scan_bytes(&bytes, 2, limit);
            Ok(json!(hits.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>()))
        }
        "find_ptr" => {
            let target = addr(req, "target")?;
            let set: std::collections::HashSet<u64> = [target].into_iter().collect();
            let limit = req.get("limit").and_then(|v| v.as_u64()).unwrap_or(200).clamp(1, 1_000_000) as usize;
            let hits = mem::scan_u64(&set, limit);
            Ok(json!(hits.iter().map(|(a, _)| format!("{a:#x}")).collect::<Vec<_>>()))
        }
        "write" => {
            // alleen schrijven als het geheugen precies `expect` bevat (onderzoek/training)
            let a = addr(req, "addr")?;
            let expect = unhex(req.get("expect").and_then(|v| v.as_str()).ok_or("expect ontbreekt")?)?;
            let new = unhex(req.get("hex").and_then(|v| v.as_str()).ok_or("hex ontbreekt")?)?;
            mem::write_checked(a, &expect, &new)?;
            Ok(json!({"written": new.len()}))
        }
        _ => Err(format!("onbekende opdracht '{cmd}'")),
    }
}
