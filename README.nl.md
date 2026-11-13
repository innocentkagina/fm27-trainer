<p align="center">
  <img src="docs/images/icon.png" width="112" height="112" alt="FM27 Coach-icoon">
</p>

<h1 align="center">FM27 Coach</h1>

<p align="center">
  <strong>Lees je Tactical Effectiveness uit, laat de app je zwakste gebieden trainen<br>
  en beheer je club live, voor Football Manager 27.</strong>
</p>

<p align="center">
  <em>FM27 Coach is gebaseerd op <a href="https://github.com/SemHiel2003/fm27-trainer">FM27 Trainer</a> door SemHiel2003 (MIT-licentie).</em>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="#functies">Functies</a> ·
  <a href="#installeren">Installeren</a> ·
  <a href="#hoe-het-werkt">Hoe het werkt</a> ·
  <a href="docs/TECHNICAL.md">Technische notities</a>
</p>

<p align="center">
  <img src="docs/images/training-nl.png" alt="Trainingspagina: de komende weken gepland op je zwakste tactische gebieden" width="92%">
</p>

---

FM27 introduceerde **Tactical Effectiveness**: vier onderdelen (Tactical Understanding, Player Ability, Player
Buy-In, Next Match Info) en 17 key factors die allemaal omhoog moeten via training en management. FM27 Coach
leest die waarden uit je spel, maakt een trainingsplan dat op je zwakste gebieden mikt, en zet het rechtstreeks
in FM's trainingskalender, week na week.

## Functies

- **Tactical Effectiveness uitlezen.** Open de *Breakdown* in FM en klik *Lees FM-scherm*. De app legt het
  venster vast, leest de labels met OCR op je eigen Mac en meet alle 17 taartmeters en 4 balken uit.
- **Training automatisch in FM.** *Zet training in FM* plant elke komende week rond je echte wedstrijden en
  schrijft de sessies in FM's eigen kalender. Het ritme komt uit de presets van Sports Interactive (herstel na
  een wedstrijd, Match Preparation de dag ervoor, conditie in de voorbereiding); de tactische sessies gaan naar je
  zwakste gebieden. Wedstrijd- en reisdagen blijven van FM, en niets op of vóór vandaag wordt aangepast. FM's
  assistent plant elke week de komende 1–2 weken opnieuw in (en zodra wedstrijden veranderen); met *Training
  automatisch bijhouden* controleert de app elke paar seconden en zet afwijkingen terug. Het logboek laat zien
  wat FM wilde en wat er is teruggezet.
- **Club en selectie.** Transfer- en loonbudget instellen, blessures genezen, schorsingen opheffen, moraal naar de
  top. Alles is ongedaan te maken en blijft pas bewaard als je in FM opslaat.
- **Transfers.** Verkoop een speler direct aan elke club, of koop er een: kies een club, kies een speler uit het
  eerste elftal, bepaal som en salaris, controleer, en hij staat in je selectie; het geld gaat meteen tussen de
  clubs over. Alles is ongedaan te maken tot je in FM opslaat.
- **Spelregels.** De Club-pagina heeft de twee schakelaars die de in-game editor van FM voor je manager kent:
  spelers altijd registreren en rugnummers altijd aanpassen.
- **Tactisch begrip.** De selectiepagina laat per onderdeel zien hoe goed de selectie je huidige tactiek kent, en
  zet het met één klik op het maximum, zoals de in-game editor van FM: het begrip van elke speler voor elke
  instelling naar 100 %. FM laat het elke speelweek weer zakken; met een schakelaar houdt de app het op 100 %
  zolang hij open staat. Spelers die nog nooit met het team trainden, doen pas mee als ze dat hebben gedaan.
- **Planning en export.** Week- en seizoensplan als HTML (printbaar), CSV en JSON, ook zonder module.
- **Nederlands en Engels.** Volgt de taal van je systeem; wisselen kan altijd in de zijbalk.

## Installeren

### macOS

Er is nog geen macOS-versie van FM27 Coach. Gebruik op een Mac de originele
[FM27 Trainer](https://github.com/SemHiel2003/fm27-trainer).

### Windows

1. Download [FM27-Coach-Windows.zip](../../releases/latest/download/FM27-Coach-Windows.zip) (altijd de nieuwste versie), pak hem uit en start **FM27 Coach.exe**.
   De exe is nog niet ondertekend: waarschuwt SmartScreen, klik dan op *Meer informatie → Toch uitvoeren*.
2. Start FM27 zoals altijd (of met *Start FM27* in de app) en laad je save. De app leest `fm.exe` rechtstreeks: er
   hoeft niets geïnstalleerd of meegestart te worden.

> Live getest op de Steam-build van november 2026 (26340721): uitlezen, budget, moraal, blessures, training en
> verkopen, elk met ongedaan maken. De volledige release van 10 november en de update van 12 november (Steam
> 26385295 en 26577676, Game Pass 0.9.2435.0) worden ook herkend. Na een FM-patch blijft de app lezen (hij zoekt
> de lijsten van de game zelf op) maar staat hij op alleen lezen tot de nieuwe build is gecontroleerd; aanpassen
> kun je dan zelf toestaan in Instellingen (maak eerst een backup van je save).

## Hoe het werkt

| Onderdeel | Wat het doet |
|---|---|
| Scherm uitlezen | Legt het FM-venster vast, vindt de 17 labels met OCR en meet elke taartmeter: FM vult die met de klok mee vanaf 12 uur en kleurt van rood naar groen. |
| Planner | Gebruikt de 59 sessies en 99 presets uit FM27's eigen game-data (namen, limieten per week, effecten), houdt het ritme van SI rond wedstrijden aan en verdeelt de tactische sessies naar tekort. |
| Live-module | Op macOS een kleine bibliotheek die met FM meestart (zoals [FMSuperScout](https://github.com/mavarobli/FMSuperScout)); op Windows leest de app `fm.exe` rechtstreeks. Hij vindt de objecten van de game via hun C++-klassenamen en de tabellen van de game, en leest of schrijft alleen op verzoek, na controle. |

## Veiligheid

- **Alleen single-player.**
- **Alleen geteste builds.** Op een niet-geteste build (na een FM-patch, tot de nieuwe indeling is gecontroleerd)
  staat de app op alleen lezen, tot je aanpassen zelf toestaat in Instellingen.
- **Ongedaan maken** voor elke wijziging en **Backup saves** met één klik.
- Er gaat niets van je computer af: geen account, geen telemetrie.

## Zelf bouwen

Nodig: Rust (stable, MSVC-toolchain) en de Visual Studio Build Tools ("Desktop development with C++").

```powershell
cargo test -p fm27-agent -p fm27-trainer       # tests
cargo run -p fm27-trainer                      # de app starten
cargo build --release -p fm27-trainer          # target\release\fm27-trainer.exe
```

Het interne pakket heet nog `fm27-trainer`; zo blijven updates van het originele project makkelijk over te nemen.

## Disclaimer

FM27 Coach is een onafhankelijk fanproject, gebaseerd op FM27 Trainer door SemHiel2003, en niet verbonden aan Sports Interactive of SEGA. Football Manager
is een handelsmerk van Sports Interactive / SEGA. Gebruik op eigen risico en bewaar backups van je saves.

## Licentie

[MIT](LICENSE) © 2026 Sem Hiel
