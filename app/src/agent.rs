//! Verbinding met FM27. macOS: met de module die in FM meedraait (via een socket), en FM starten
//! met die module. Windows: de app leest fm.exe zelf, met dezelfde code (crate fm27-agent).

use anyhow::Result;
#[cfg(not(windows))]
use anyhow::bail;
#[cfg(target_os = "macos")]
use anyhow::{Context, anyhow};
use serde_json::{Value, json};
#[cfg(target_os = "macos")]
use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "macos")]
use std::os::unix::net::UnixStream;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::time::Duration;

/// Of live-functies (lezen en schrijven in FM) op dit platform beschikbaar zijn.
pub const LIVE_SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

const STEAM_APP_ID: &str = "4530370";

#[cfg(target_os = "macos")]
fn socket_path() -> PathBuf {
    crate::data_dir().join("agent.sock")
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn request(_req: Value) -> Result<Value> {
    bail!("{}", crate::t!("Live-functies zijn niet beschikbaar op dit systeem.", "Live features are not available on this system."))
}

/// Voert één opdracht uit tegen fm.exe.
#[cfg(windows)]
pub fn request(req: Value) -> Result<Value> {
    fm27_agent::request(&req).map_err(|e| anyhow::anyhow!(e))
}

/// Of er iets is om mee te verbinden: de socket van de module (macOS) of een draaiende fm.exe.
pub fn module_ready() -> bool {
    #[cfg(target_os = "macos")]
    {
        // na het afsluiten van FM blijft het socketbestand achter: dan niet verbinden
        socket_path().exists() && fm_running()
    }
    #[cfg(windows)]
    {
        fm27_agent::fm_pid().is_some()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// Stuurt één opdracht naar de module en geeft `data` terug.
#[cfg(target_os = "macos")]
pub fn request(req: Value) -> Result<Value> {
    let mut s = UnixStream::connect(socket_path()).context(crate::t!(
        "module niet bereikbaar (draait FM27 met de module?)",
        "module not reachable (is FM27 running with the module?)"
    ))?;
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    writeln!(s, "{req}")?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    let v: Value = serde_json::from_str(&line).context("module reply")?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    } else {
        bail!("{}", v.get("error").and_then(|e| e.as_str()).unwrap_or("?"))
    }
}

#[allow(dead_code)]
pub fn ping() -> bool {
    request(json!({"cmd": "ping"})).is_ok()
}

#[cfg(not(windows))]
fn pgrep(name: &str) -> bool {
    Command::new("pgrep").args(["-x", name]).stdout(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

pub fn fm_running() -> bool {
    #[cfg(windows)]
    {
        fm27_agent::fm_pid().is_some()
    }
    #[cfg(not(windows))]
    {
        pgrep("Football Manager 27")
    }
}

#[cfg(target_os = "macos")]
fn fm_executable() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home)
        .join("Library/Application Support/Steam/steamapps/common/Football Manager 27/fm.app/Contents/MacOS/Football Manager 27");
    if p.exists() {
        return Some(p);
    }
    let out = Command::new("mdfind").arg("kMDItemCFBundleIdentifier == 'com.SportsInteractive.FootballManager27'").output().ok()?;
    let app = String::from_utf8_lossy(&out.stdout).lines().next()?.to_string();
    let p = PathBuf::from(app).join("Contents/MacOS/Football Manager 27");
    p.exists().then_some(p)
}

/// De module: naast het programma (in de .app) of in target/release (ontwikkeling).
#[cfg(target_os = "macos")]
fn agent_source() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for c in [
        dir.join("libfm27_agent.dylib"),
        dir.join("../Resources/libfm27_agent.dylib"),
        dir.join("../release/libfm27_agent.dylib"),
        dir.join("libfm27_agent.dylib"),
    ] {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn start_fm() -> Result<String> {
    bail!("{}", crate::t!("Live-functies zijn niet beschikbaar op dit systeem.", "Live features are not available on this system."))
}

/// Windows: FM27 gewoon via Steam starten; de app leest het proces van buitenaf.
#[cfg(windows)]
pub fn start_fm() -> Result<String> {
    use std::os::windows::process::CommandExt;
    if fm_running() {
        return Ok(crate::t!("FM27 draait al. Laad je save; de app verbindt vanzelf.", "FM27 is already running. Load your save; the app connects automatically.").into());
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("cmd")
        .args(["/C", "start", "", &format!("steam://rungameid/{STEAM_APP_ID}")])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(crate::t!(
        "FM27 wordt gestart via Steam. Laad je save; de app verbindt vanzelf.",
        "FM27 is starting through Steam. Load your save; the app connects automatically."
    )
    .into())
}

/// Start FM27 met de module. FM mag nog niet draaien; Steam moet open en ingelogd zijn.
#[cfg(target_os = "macos")]
pub fn start_fm() -> Result<String> {
    if fm_running() {
        bail!("{}", crate::t!("FM27 draait al. Sla je spel op, sluit FM27 en probeer opnieuw.", "FM27 is already running. Save your game, quit FM27 and try again."));
    }
    let exe = fm_executable().ok_or_else(|| anyhow!("Football Manager 27 not found"))?;
    let src = agent_source().ok_or_else(|| anyhow!("libfm27_agent.dylib not found next to the app"))?;
    std::fs::create_dir_all(crate::data_dir())?;
    let dst = crate::data_dir().join("fm27_agent.dylib");
    std::fs::copy(&src, &dst).context("copy module")?;
    // een gedownloade (gequarantaineerde) kopie mag FM gewoon laden
    let _ = Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(&dst).stderr(Stdio::null()).status();
    let _ = std::fs::remove_file(socket_path());

    if !pgrep("steam_osx") {
        let _ = Command::new("open").args(["-g", "-a", "Steam"]).status();
        for _ in 0..90 {
            if pgrep("steam_osx") {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        std::thread::sleep(Duration::from_secs(8)); // inloggen
    }
    Command::new(&exe)
        .env("DYLD_INSERT_LIBRARIES", &dst)
        .env("SteamAppId", STEAM_APP_ID)
        .env("SteamGameId", STEAM_APP_ID)
        .current_dir(exe.parent().unwrap_or(&exe))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("start FM27")?;
    Ok(crate::t!(
        "FM27 wordt gestart met de module. Laad je save; de app verbindt vanzelf.",
        "FM27 is starting with the module. Load your save; the app connects automatically."
    )
    .into())
}

/// Kopie van de nieuwste save(s) naar ~/Documents/FM27 Coach/backups.
pub fn backup_saves() -> Result<PathBuf> {
    let games = crate::fm_user_dir().join("games");
    let mut saves: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&games)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "fm"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    saves.sort_by(|a, b| b.0.cmp(&a.0));
    let dir = crate::export::output_dir().join("backups").join(chrono::Local::now().format("%Y-%m-%d %H.%M").to_string());
    std::fs::create_dir_all(&dir)?;
    for (_, p) in saves.iter().take(2) {
        std::fs::copy(p, dir.join(p.file_name().unwrap_or_default()))?;
    }
    Ok(dir)
}
