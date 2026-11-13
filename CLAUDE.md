# FM27 Trainer: handover for Claude

Read this first. This is a fork of SemHiel2003/fm27-trainer (MIT). It summarises what the original author worked out
and built (macOS and Windows, November 2026), translated from the original Dutch notes and adapted for this fork.
Answer in English, short and concrete.

**Always ask before deciding.** Any choice (product rules, wording users see, rebrand details, scope, git pushes,
releases, file placement) goes to the user via AskUserQuestion: the recommended option first and labelled
"(Recommended)", each option with a one-line trade-off. No silent defaults. Record the answers in this file or in
`docs/`.

## Decisions (user-confirmed)

- 2026-11-13: Fork on GitHub (`innocentkagina/fm27-trainer`), source in `D:\FM27-Trainer-Windows`.
- 2026-11-13: Pushing — ask before every push, tag or release to `origin`; never push to `upstream`.
- 2026-11-13: Dutch comments/identifiers — leave them; new code in English; translate an area only after asking.
- 2026-11-13: The downloaded upstream `FM27 Trainer.exe` stays in the folder, excluded from git, for comparison.
- 2026-11-13: Goals for this fork: build it from source, rebrand it, and change/add features (details still to be
  asked).

## What it is

A Rust app (egui) for Football Manager 27:

1. **Read Tactical Effectiveness.** Screenshot of FM's *Breakdown*, OCR (Apple Vision / Windows.Media.Ocr),
   measure the pie gauges → 4 components + 17 key factors.
2. **Plan training** on the weakest areas, using the 59 sessions and 99 presets from FM27's own data.
3. **Live in FM** (via the agent): write training into FM's calendar and keep it there (FM's assistant re-plans the
   next 1–2 weeks every week; the app puts them back), budget, injuries, suspensions, morale, **sell players
   directly** to and **buy** from another club, and **maximise tactical understanding** (like FM's in-game editor).

## Repos and remotes

- `origin` = `github.com/innocentkagina/fm27-trainer` — **this fork** (public). Work happens here.
- `upstream` = `github.com/SemHiel2003/fm27-trainer` — the original project. Pull updates with
  `git fetch upstream && git merge upstream/main`. **Never push to `upstream`.**
- Releases: the GitHub Actions workflow (`.github/workflows/release.yml`) builds the Windows zip on a `v*` tag
  (notes in `docs/releases/<tag>.md`) or manually (`platform=windows`). The macOS job needs Apple signing secrets
  this fork does not have; it is skipped on tags unless the repo variable `MACOS_CI` is `true`.
- The working folder `D:\FM27-Trainer-Windows` also holds the originally downloaded `FM27 Trainer.exe` (upstream
  v0.2.4), excluded locally via `.git/info/exclude`. Keep it for comparison; don't commit it.
- MIT licence: keep `LICENSE` and the original copyright line, also after a rebrand.

## Layout

```
app/                 the app (bin fm27-trainer): egui UI, OCR, planner, live client
  src/main.rs        CLI: analyze, year, agent '<json>', agent-serve, train [--write], start-fm
  src/gui.rs         all pages (Overview, Training, Planning, Club, Squad, Sell, Settings)
  src/live.rs        connection state, background jobs, keep training on plan automatically (every 8 s)
  src/live_training.rs  read calendar → plan → write per week
  src/agent.rs       macOS: socket to the module; Windows: calls fm27_agent::request directly
  src/planner.rs, model.rs, vision.rs, export.rs, theme.rs, i18n.rs (t!("nl", "en") — Dutch first, then English)
  src/labels.rs      the Breakdown labels in all 20 FM languages (generated, tools/research/breakdown_labels.py)
agent/               crate fm27-agent: the game logic, shared
  src/lib.rs         dispatch of all commands (JSON); macOS: module inside FM + socket server
  src/mem.rs         memory: macOS (Mach, own process), Windows (ReadProcessMemory/WriteProcessMemory on fm.exe),
                     tests (simulated memory)
  src/rtti.rs        class recognition: Itanium (clang/macOS) and MSVC (Windows)
  src/game.rs        field offsets per platform (Layout MAC/WIN), builds (BUILDS), manager/club/squad/training/undo
  src/sell.rs        sell and buy a player directly (one move function `relocate`), club search, a club's squad
  src/understanding.rs  read and maximise tactical understanding per player
  src/tests.rs       whole flow against simulated FM memory, both layouts; MSVC RTTI against testdata/rtti_test.dll
docs/TECHNICAL.md    architecture and offsets; docs/research/fm27-transfer-offers.md: research on transfer offers
tools/research/      fmq.py (research client), offers.py, breakdown_labels.py (labels from FM's languages.fmf),
                     vision_regress.py (screen-reading regression test: variants of a capture, also other languages)
```

Code comments and many identifiers are in Dutch (the original author's language). Keep the existing style when
editing nearby code; ask before mass-translating comments (it makes merging upstream updates harder).

All agent commands: `ping`, `club`, `squad`, `set_finances`, `heal`, `lift_bans`, `inspire`, `undo`,
`game_date`, `training_weeks`, `set_week`, `clubs`, `club_squad`, `sell`, `buy`, `understanding`, `max_understanding`
(with `dry_run`), `human_flags`, `set_human_flags`, `allow_untested`, and for research `chain`, `census`, `class_of`,
`read`, `find_u16`, `find_ptr`, `write` (only with `expect`), `discover`. See `agent/src/lib.rs`.

## Build and test on Windows

- Rust stable with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`) and Visual Studio Build Tools
  (workload "Desktop development with C++"). On this PC both were installed via winget on 13 Nov 2026; a terminal
  that was already open only sees `%USERPROFILE%\.cargo\bin` on PATH after a restart.
- `cargo test -p fm27-agent`: 11 tests, `cargo test -p fm27-trainer`: 3 tests (synthetic pie gauges); all should pass.
- `cargo run -p fm27-trainer`: the app. `cargo build --release -p fm27-trainer` → `target\release\fm27-trainer.exe`
  (static C runtime via `.cargo/config.toml`, so no VC++ Redistributable needed).
- **The release exe has no console** (`windows_subsystem = "windows"`). Use the debug build for the CLI:
  `cargo run -p fm27-trainer -- agent "{\"cmd\":\"ping\"}"`. For scripts: `agent-serve` (stdin/stdout, also works
  with the release exe), see `tools/research/fmq.py`.
- Python tools: run with `-X utf8`, otherwise the console chokes on accents in player names.
- The agent crate is optimised even in the debug build (`[profile.dev.package.fm27-agent]` in Cargo.toml):
  without it a heap scan takes over a minute instead of 1.5 s.
- The Mac DMG can only be built on a Mac with a Developer ID (`./release.sh`, signing + notarisation).

## How live works on Windows

- The app finds `fm.exe` (Toolhelp), opens it with `PROCESS_VM_READ | VM_WRITE | VM_OPERATION | QUERY_INFORMATION`
  and reads/writes from outside. Just start FM via Steam; no module or injection.
- The native core is `game_plugin.dll`. A build is labelled `PE-<linktime>-<SizeOfImage>`; Steam 26340721 =
  `PE-6AE565AF-1821C000`, Game Pass / Xbox app 0.9.2409.0 = `PE-6AE563A1-175DA000` (same update, same object
  layout; fixed addresses from FMLibreEditor `profiles/fm27/gamepass-0.9.2409.0.toml`; writing enabled,
  **not live-tested by the original author**). Unknown build → no fixed addresses (staff list, club list, date).
  Game Pass also runs as `fm.exe` (from `...\Content\`); an Xbox console cannot be read.
- **Builds after the release** (12 Nov 2026, after "not connected" reports on sortitoutsi and GitHub #1/#2; all fixed
  addresses from FMLibreEditor `profiles/fm27/`, object layout unchanged according to them): Steam 26385295 (10 Nov) =
  `PE-6AE8B5A4-17CB9000` and 26577676 (12 Nov) = `PE-6AF38E5C-185FA000`, both staff 0x5820858, clubs 0x58209C8, date
  0x57CA3B8; Game Pass 0.9.2435.0 = `PE-6AE8CF77-17969000`, staff 0x53B7E28, clubs 0x53B7F98, date 0x53597C8; Mac
  arm64 26385295 (`2A2D0ED6-…`) and 26577676 (`A8550B47-FDE0-3D0D-A6CB-1B63DEE8DA19`), both staff 0x576EE98, clubs
  0x576F008, date 0x574DDB8; the Intel halves are in `BUILDS` too. Writing: Mac arm64 26577676 **live-tested**
  (12 Nov 2026); the three Windows builds are enabled **on the strength of FMLibreEditor's measurements** (the
  original author's decision; not live-tested on Windows); Mac 26385295 and Intel: read-only.
- **Unknown build** (`game::statics` → `discover`): the app looks for the lists itself. In all 11 measured profiles
  the club pair sits 0x170 after the staff pair; it searches for two pointer-list pairs at that distance (largest
  first) where the second holds clubs and the first a manager with a valid chain ending at a club from that list.
  Date: within 0x400 of the date↔staff-list distance of the known builds of the same platform, matching the team
  date; otherwise the team date as before. Writing stays off until `allow_untested`. Measured on the Mac: 1,945
  double pairs in `__DATA`, 21 date-like values, exactly one within 0x400; the game date has flag bits above the day
  (0x19). Research command `discover` compares the search with the profile; `ping` returns
  `statics: profile|discovered|null` and `build_label`. On a new FM patch: copy the profile from FMLibreEditor into
  `BUILDS` and run the Task 1 procedure.
- **Writing is enabled on Windows** for Steam build 26340721 (`writes_tested: true` in `BUILDS`,
  `agent/src/game.rs`; live-checked 8 Nov 2026). On an unknown build (after an FM patch) the app is read-only; the
  user can allow it per session (Settings → "Modify on an untested FM version" → command `allow_untested`).
- `game_plugin.dll` is **packed**: sections are named `.xtext`, `.data1`, `.shared`, `.rodata`, `.sbss` (307 MB,
  filled on load), `.rdata` (small). The protector renames them per build: on Game Pass the TypeDescriptors are in
  `.00cfg` and the big RWX section is called `.rsrc`. The MSVC RTTI search therefore scans **all** sections,
  regardless of name (`msvc_areas` in `agent/src/rtti.rs`; until 9 Nov 2026 it skipped `.rsrc`/`.00cfg`, so the app
  found no manager on Game Pass); a block that can't be read in one go is tried in smaller pieces
  (`mem::for_each_chunk`). `class_of` on the manager object itself returns None (vbptr); the manager is found via the
  staff list. Looking up a class costs ~1.5 s once, a full heap scan (3.5 GB) ~1.5 s in a release build.

## Safety rules (always)

- Write only via `write_checked`/`Patch`: first check the memory is still exactly what you expect, then read it back.
  Every change has an undo (`Change` in game.rs; money as a difference).
- Experiments only in a **separate test save**, and the user must **not play on or save** during an experiment.
  After an experiment: close FM without saving, or restore cleanly.
- Never shift lists whose position means something (the offers list: offer number = index).
- `find_ptr` also finds the module's own buffers: always verify with `u64(a) == target`. Limit 200.
- FMLibreEditor (github.com/tozmoz/FMLibreEditor) is GPL: use their **facts** (offsets, behaviour), never copy code.
  This project is MIT.
- Don't push to `origin`, create tags or releases without asking. Never push to `upstream`.

## Offsets (Steam build 26340721)

Players and staff differ per platform; clubs, finances, contracts, teams, injuries and suspensions don't.

| Field | macOS | Windows |
|---|---|---|
| Manager: contract / identity block (+ shift 0x340) | 0x170 / 0x208 | 0x1A8 / 0x100 |
| Player: team, injury record | 0x128, 0xF0 | 0x130, 0xF8 |
| Player: sharpness, condition, morale (u8 1–20) | 0x250, 0x254, 0x264 | 0x258, 0x25C, 0x26C |
| Player: names (first, last, common) | 0x298, 0x2A0, 0x2A8 | 0x2D8, 0x2E0, 0x2E8 |
| Player: contract, identity block, UID | 0x2F0, 0x388, 0x394 | 0x330, 0x288, 0x294 |
| Player: transfer value, asking price (i32 pounds) | 0x22C, 0x230 | 0x234, 0x238 |
| Player: tactical understanding record (ptr, see below) | 0xF8 (not measured) | 0x100 |
| Internal person id | identity block + 8 | identity block + 8 |
| Player-coach: everything from the person block on shifts by | 0xF8 | 0xF8 |

Shared: contract `+0x08` owner (→ identity block), `+0x10` team, `+0x18` senior team, `+0x20` wage,
`+0x26` job (1 = player), `+0x30` unhappiness (ptr), `+0x44/+0x48/+0x4C` signed/expires/joined (date),
`+0x57` transfer/loan list (bit 0/1), `+0x5A` happiness (**i8**), `+0x5D` squad number (255 = none).
Team `+0x30` club, `+0x38` player list (begin, end, cap), `+0x88/+0x90` (vice-)captain.
Club `+0x0C` UID, `+0x18` teams (first = first team), `+0xB9` women's club, `+0xC0` name, `+0x150` finances.
Finances `+0x0C` balance, `+0x7C4` transfer budget, `+0x808` total wage budget, `+0x814` wage budget (i32 pounds),
`+0x864` ticket price (f32), `+0x8A9` corporate (u8). Injury record: list of current injuries (healed = end := begin),
`+0x18` absences (suspension: `0xFFFF` at +0x16). Date = u16 day-of-year (low 9 bits, rest flags) + u16 year.

Fixed addresses (RVA in game_plugin):

| | macOS arm64 | Windows |
|---|---|---|
| staff list (begin, end) | 0x56D2E60 | 0x5705818 |
| club list (begin, end) | 0x56D2FD0 | 0x5705988 |
| game date | 0x56B1DB8 | 0x56AF378 |

The manager is found via the staff list (Windows first) or RTTI `db::HUMAN_NON_PLAYER`.
**Game date:** always the fixed address; team+0xA0→+0x94 is a *different* date (around the last match).

**Training** (measured on macOS and Windows, both **+0x28**): one block per team with the team pointer and at +0x28 a
list of week records (0x60 bytes: `+0x00` schedule name (FM string), `+0x0C` Monday, `+0x10` 7 × [u16 0,
session 1, session 2, extra], `+0x48` u32 schedule template number). On Windows the code also tries 0x08–0x80 with
strict checks, in case a later build differs. **Search** (9 Nov 2026, after "training calendar not found"
reports): all heap words equal to the team pointer, no limit (it used to stop at 20,000; Sunderland save: 2,042
hits, calendar was number 1,462). A week counts if the Monday is valid, the first word per day is small (≤ 0xFF,
never written) and ≥ 2/3 of the 21 slots hold a known code (≤ 600), of which ≥ 5 non-zero; `set_week` leaves a day
with an unknown code alone. If nothing is found, the error states the number of hits and why near-candidates were
rejected. The real cause for the reporting users was never seen (no save from them).
Session codes: 0–7 Tactical Understanding, 100–108 Match Preparation, 200–228 Footballing Development,
300–307 Physical, 400 Team Bonding, 401 Community, 402 travel, 500 match.

**Line-ups**: `simatch::PREPARED_TACTICS_CREATOR_ARRAY`, `+0x08` list, `+0x28` 26 × u32 person id
(0xFFFFFFFF = empty). Allocator list blocks: Mac 144…4096, Windows 144…512 (`heap_blocks`).

**Game rules (manager editor flags)**: u32 at manager+0x3FC (Windows; Mac unknown, `Layout.human_flags`),
bit 0x100 = "Allow squad registration anytime", 0x200 = "Allow squad numbers anytime" (FM's in-game editor,
actions `TEAM_CONTAINER_ALLOW_SQUAD_REGISTRATION_ANYTIME_ACTION` / `..._SQUAD_NUMBERS_...`; the action sets or clears
the bit on the object the HUMAN record returns via vtable+0x698, which is the manager object itself: the subobject at
manager+0x440 has the HUMAN_NON_PLAYER vtable, which the app checks before writing). Default 0. That a set bit means
"always allow" is inferred, not seen on FM's screen: verify on a competition's registration screen. Commands
`human_flags`/`set_human_flags`, switches on the Club page ("Rules").

**Tactical understanding** (FM27's "Tactical Understanding", successor of tactical familiarity; found 8 Nov 2026 by
disassembling FM's editor action `TEAM_MAXIMISE_TACTICAL_UNDERSTANDING_ACTION`, see `agent/src/understanding.rs`):
player `+0x100` → record of 0xE0 bytes. Six blocks of 0x20 (area 0–5: Passing Style, Build Up, Chance Creation,
In Possession Shape, Line of Engagement, Pressing Intensity): (begin, end, cap) list of u32 items (bits 0–13 level
0–10000, bits 14–19 tactic setting 0–20, rest junk) + u64 area number at `+0x18`; at `+0xC0` a list of 16-byte items
for Pressing Trap and Out Of Possession Shape (u16 role/position, 11 bytes shape, u8 flags, u16 level at `+0x0E`);
`+0xD8` date, `+0xDC` "changed". Understanding for a setting = max over items of (level − 1000 × distance), floor
1000. Maximising sets every item to 10000 and adds the most common setting for players missing it, only within the
capacity of FM's list (no own allocation). Live-tested on Windows (8 Nov 2026, Ajax save, 27 players). **FM lets
the levels drop every game week** (after four months of play everything had fallen from 10000 to 7620), hence the
"Keep at 100 %" switch (setting `keep_understanding`): every 20 s, with the regular connection check, the app sends
`max_understanding`, which only writes what has dropped. TRAINING_MANAGER's per-team training block (above) and
`TACTICS_MANAGER` do *not* contain this. Disassemble with capstone (`pip install capstone`) on code fetched from
fm.exe via `read`; on Windows game_plugin.dll is packed on disk but unpacked in memory.

## What has been tested, where (by the original author)

| | macOS (live, Ajax save) | Windows |
|---|---|---|
| read club/squad/calendar | ✅ | ✅ (8 Nov 2026, Ajax save) |
| budget, heal, morale, write training, undo | ✅ | ✅ (8 Nov 2026) |
| sell (`sell`) | simulation only | ✅ dry run and one real sale + undo (8 Nov 2026) |
| buy (`buy`, `club_squad`) | simulation only | dry run live (8 Nov 2026); real purchase untested |
| game rules (`set_human_flags`) | n/a (location unknown) | read (0); effect in FM unverified |
| tactical understanding (`understanding`, `max_understanding`) | simulation only (offset 0xF8 is a guess) | ✅ read and maximised (8 Nov 2026) |
| MSVC RTTI | n/a | ✅ testdata and live (census, line-ups) |
| Game Pass / Xbox app (0.9.2409.0) | n/a | simulation only (build, date, staff list, calendar); offsets from FMLibreEditor |

Mac arm64, Steam build 26577676 (12 Nov 2026, Ajax save 25 July 2027, sequence via `fm27-trainer agent`): club,
squad (30), calendar (108 weeks), clubs, tactical understanding (offset 0xF8 reads plausible values), date ✅;
`set_finances` +1, `inspire` (17→20), `set_week` (one day), each with `undo` and exactly restored ✅; `heal` with no
injured players (nothing to do); `sell`/`buy`/`max_understanding` only `dry_run`. Windows on the new builds:
**not live-tested yet**. The same sequence was repeated with the 0.2.4 release binaries; `discover` found exactly the
profile's addresses there in 64 ms (`same: true`).

## Task 1: verify Windows live — procedure (done upstream on 8 Nov 2026 for build 26340721)

Steps below were all green on Steam build 26340721: reading matched FM (club, balance, budgets, date, squad,
calendar), and `set_finances`, `inspire`, `heal`, `set_week` and `sell` (dry run and one real sale) each worked with
`undo`. Found and fixed then: the RTTI search only looked in `.data`/`.rdata` (packed DLL, see above) so selling
failed, and the debug build was ~40x too slow. After an FM patch (unknown build: `known_build false`) repeat this.

Use a test save. Start FM27 via Steam, load the save, then:

```
cargo build -p fm27-trainer
python -X utf8 -i tools\research\fmq.py
>>> q({"cmd":"ping"})            # check build label and known_build
>>> q({"cmd":"club"})            # name, balance, budgets: compare with FM (FM shows euros: pounds × ~1.17)
>>> q({"cmd":"squad"})           # names, morale, condition, injuries, value, contract
>>> q({"cmd":"game_date"})       # must be the date in FM
>>> q({"cmd":"training_weeks"})  # Mondays 7 days apart, codes matching FM's calendar
```

If all is right, allow writing (`q({"cmd":"allow_untested","on":True})`) and test, each followed by `undo` and a
check in FM: `set_finances` (transfer budget +1), `inspire` for one player, `heal`, `set_week` for one future day,
`sell` with `dry_run`, then one real sale + `undo`. If everything works: set `writes_tested: true` for the Windows
build in `BUILDS`, note at which offset the training block was found (`State.training_block`; make `TB_WEEKS` per
platform if needed), update docs/TECHNICAL.md and the README, and ask whether a release should follow.

If something goes wrong: use `census`, `describe`, `refs` to see where it differs; compare with FMLibreEditor's
`profiles/fm27/layout.toml`.

## Task 2: transfer offers (see docs/research/fm27-transfer-offers.md) — open upstream research

The original goal: say "sell player X to club Y for Z" in the app and get a **real offer** in FM.

Known (macOS): an offer is `FULL_TRANSFER_OFFER` (0xD0 bytes, from a pool), all offers are in
`TRANSFER_MANAGER+0x08` (offer number = index). FM only shows an offer via its inbox message `TRANSFER_OFFER_NEWS`
(large object, ~40 references, in a news stream without a list) and a detail record with total, offer number,
sequence number and bonuses. An offer without a message is invisible and never handled (tested).

Options, route not chosen yet:
- **A. Rewrite an incoming offer** (buyer, player, fee) in offer + detail record + both copies of the message, then
  accept in FM → FM does a real transfer. Next step: with a fresh offer in a test save, map all fields of the message
  and the detail record (on Windows first verify the offer offsets: `tools/research/offers.py`).
- **B. Direct sale** (built, `sell`): reliable but without message/history.
- **C. Let FM bid itself**: transfer list + asking price + give the buyer more budget.

FM27 uses fake names for unlicensed clubs: AC Milan = **Casciavit Milano** (UID 1099), Inter =
**Blu-neri Milano** (UID 1135). Useful for the app's search field (aliases?).

## Screen reading (Breakdown) on Windows, measured 8 Nov 2026

- Each pie gauge has **10 segments**, filled clockwise from 12 o'clock; the empty part is **almost white**
  (s < 0.25, v > 0.66), the spokes are light too, the background of a highlighted row is mid-grey
  (s ≈ 0.12, v ≈ 0.34) and used to fall under "gauge grey". The score is now filled segments × 10 (last truly
  coloured sector, rounded to a segment); the hue (orange low, yellow mid, green high) is kept only. Bars: 11
  positions, one coloured (the value) and one white tick; value = position from the first tick / 10. The
  translucent panel background can drown some ticks in noise.
- Windows OCR reads "Player Buy-In" as "Player Buy-ln" (alias in `model.rs`).
- **Since 9 Nov 2026** (report: "Build Up ring not half full, app says 70"): a pie is cut out as the contiguous rows
  around the middle of the label (a neighbouring gauge in the search band no longer counts; with larger text,
  text/row ≥ 0.7, the old code read 7 segments as 80 or 50), only pixels in the fill hue count (most common hue in
  the inner ring, ±25°), and FM's colour is a check: 20–40 % orange (~43°), 50–70 % yellow (~60°), 90 % green
  (~120°), 40 % was yellow at Ajax. If a reading falls outside its colour's band it is clamped to the edge and
  flagged (`GaugeReading.doubt`; the app asks the user to check it). Tests: `pies_on_any_background`,
  `pies_in_tight_rows` in `vision.rs`. On the original author's captures (Ajax, Sunderland) old and new read
  everything correctly, also scaled down to 1366×768; the reporting user's capture was never seen. On a new report,
  ask for `last-breakdown.png` (Settings → "Open app folder").
- **Resolution-independent since 8 Nov 2026** (report: "Read 8 of 17 gauges", key factors at 0 or 50):
  `analyze_image` first reads at true size and, if labels aren't ~20 px high (`REF_TEXT`, the size at 2560×1440 at
  100 %), again on a copy scaled to that size (with no label recognised: 1.5× and 2×); the reference-size reading
  wins on a tie. Further: labels are matched tolerantly (Levenshtein, ~1 letter per 6), a missed label gets its row
  from the row spacing and the gauge column of the found labels, and a column with no label at all from the "Key
  Factors" header (first row 2.15 × header height below it, gauge = last round blob on the row). Every pie gauge is
  scaled to 72 px before measuring; the fill is the contiguous run of coloured sectors from 12 o'clock (one-sector
  gap = spoke). Bar gauges are read as brightness peaks (ticks) with the frame edge removed (columns light over more
  than 60 % of the height); value = position of the coloured tick between first and last tick × 10. Tested by
  scaling `last-breakdown.png` with Pillow to 3840×2160 … 1280×720: all 21 values equal from 1366×768 to 4K; at
  720p (very blurry) 15 of 17. `FM27_VISION_DEBUG=1` prints the bar measurement.
- **Since 9 Nov 2026 (evening, version 0.2.4)**, after "still 8 of 17, first 8 at 0, bars 90/100/100/80" and
  "reads nothing at all, does FM have to be in English?": labels in all FM languages (not Arabic; US English has
  "Coaching Support" and "Head Coach Tendencies"), matching on letters only (spaces/accents stripped, CJK OCR has no
  spaces), titles first (German: Player Ability = Player Capability = "Spielerfähigkeit"), a label is at least half
  its line. A pie must be a dense, round disc (a letter "Q" or the icon before the label was read as an empty gauge
  = 0); 0 is always flagged. If OCR glues the pie to the label as "O", the label end comes from the median letter
  width. "Light" is relative to the background (on a dark capture every bar read 90–100 because the frame edge
  counted as a tick; now the regularity of the ticks counts); "coloured" must also be 0.2 lighter than the
  background, and less saturated pixels in the fill hue count too (washed-out HDR). A bar is also read if its title
  isn't recognised (on the row of the other titles, in its own panel). On Windows the capture takes the fm.exe
  window (by pid), never a browser (which can be the same size with an FM page open). OCR falls back to other
  languages (Windows: installed OCR languages; Mac: Vision's languages via `recognitionLanguages`, not live-tested).
  Every reading writes `last-analysis.txt`; the app asks for it on an incomplete reading. Tested with
  `tools/research/vision_regress.py` on the original author's capture: 18 of 18 variants correct (0.2.3 got 7 of a
  wider set of 27 right, 0.2.4 got 23). The reporters' actual capture still hasn't been seen: whether this was
  their cause is unconfirmed.
- Every reading saves `last-capture.png`, `last-analysis.txt` and, if a Breakdown was on screen,
  `last-breakdown.png` in `%APPDATA%\FM27 Trainer`; replay with `target\debug\fm27-trainer.exe analyze <png> --ocr`
  (that output equals `last-analysis.txt`). Pixel-inspection helper scripts (pie_debug.py, montage.py) were not kept;
  recreate with Pillow if needed (`python -m pip install --user pillow`).

## Other

- The Windows exe is not signed (SmartScreen: "More info → Run anyway").
- GUI test helpers: `FM27_PAGE=training|planning|club|squad|sell|settings`, `FM27_LANG=nl|en`,
  `FM27_SNAPSHOT=path.png` (screenshot of its own window, then quit), `FM27_WINDOW=1320x1000`.
  With `FM27_SNAPSHOT` the app skips automatic training upkeep (since 8 Nov 2026): a test capture never writes to
  FM. Without that variable, a started app with "keep on plan automatically" on writes the training plan into FM
  within seconds; keep that in mind when testing against a real save.
