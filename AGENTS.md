# AGENTS.md — zasady dla agentów pracujących w tym repozytorium

Przeczytaj w tej kolejności: `AGENTS.md` → `docs/PLAN.md` (decyzje i zakres) → `docs/MODULE_GUIDE.md` (jak dodać moduł).

## Czym jest ten projekt

SerwisKit — open‑source narzędzie serwisowe dla Windows 10/11 x64 w Rust z GUI (egui), zastępujące AIDA64 + HD Tune + CrystalDiskInfo + migrację profili przeglądarek. Jeden plik `.exe`, praca offline, moduły dokładane przez kontrybutorów.

## Komendy (zweryfikowane na tej maszynie: Rust 1.99.1 MSVC, Win11 26300)

```powershell
cargo fmt --all                       # formatowanie
cargo clippy --all-targets -- -D warnings
cargo test --all                      # testy bez sprzętu (mocki)
cargo test --all -- --ignored         # testy sprzętowe, WYMAGAJĄ administratora
cargo run -p sk-app                   # GUI
cargo run -p sk-app -- report --json out.json     # tryb CLI/headless
cargo build --release                 # artefakt: target\release\serwiskit.exe
```

Wszystkie cztery bramki (`fmt`, `clippy`, `test`, `build`) muszą być zielone przed zgłoszeniem pracy.

## Twarde reguły

1. `unsafe` wyłącznie w `crates/sk-win`. Moduł potrzebujący nowego wywołania systemowego zgłasza to, nie dopisuje FFI u siebie.
2. Zero sieci w kodzie produktu: brak HTTP, telemetrii, auto‑update w modułach.
3. Zero `unwrap()`/`expect()` na danych zewnętrznych (WMI, pliki, rejestry, I/O). Błąd → `ModuleError` z kontekstem.
4. Żadnych tekstów widocznych dla użytkownika poza i18n (`pl` + `en`).
5. Nie blokuj wątku UI: operacje > ~200 ms to `TaskDescriptor` w wątku roboczym.
6. Nie zmieniaj publicznego kontraktu (`sk-core`), `Cargo.toml` ani `docs/PLAN.md` w zadaniu modułowym — to wymaga zgody lidera i ADR.
7. Nie wymyślaj API, flag ani wersji. Sprawdź w dokumentacji Microsoft/crates.io; jeśli nie potwierdzone — napisz „niepotwierdzone”.
8. Operacje destrukcyjne (zapis na dysk, nadpisywanie danych) nigdy nie są domyślne i zawsze wymagają jawnego potwierdzenia.
9. Licencja projektu to **GPL-3.0-only**. Nowa zależność musi być GPL-kompatybilna (MIT, Apache-2.0, BSD, MPL-2.0 — tak; kod proprietary — nie). Nie dodawaj zależności o niejasnej licencji.
10. Wersja release ma manifest `requireAdministrator` (build.rs, tylko profil release). Nie usuwaj tej bramki i nie zakładaj, że proces działa bez uprawnień administratora — sprawdzaj to w kodzie, zamiast zgadywać.

## Zakresy zapisu przy pracy równoległej

| Plik | Właściciel |
|---|---|
| `crates/sk-modules/src/modules/<moduł>.rs` | autor modułu (nowy plik) |
| `crates/sk-modules/src/modules/mod.rs` | autor modułu (2 linie: `mod x;` + `registry.add::<x::X>();`, alfabetycznie; przy konflikcie zachowaj obie pary) |
| `crates/sk-core/locales/{pl,en}/<rodzina>.json` | autor modułu (swoje klucze) |
| `crates/sk-core/**`, `crates/sk-app/**`, `crates/sk-win/**`, `Cargo.toml`, `docs/PLAN.md` | lider |

Jeden bot = jeden moduł = jeden nowy plik. Nie edytuj plików innych modułów.

## Jak dodać moduł

Pełna recepta: `docs/MODULE_GUIDE.md` §3. Skrót: nowy plik w `sk-modules/src/modules/`, implementacja `Module` (`meta` + `collect`), dwie linie w `modules/mod.rs`, klucze i18n w `pl`/`en`, test na makiecie (`sk_core::testing::Harness`), checklista DoD.

Rejestracja jest **jawna** (`ModuleRegistry::add::<T>()`). Nie wracaj do `inventory`/auto-rejestracji: linker MSVC gubił statyki i aplikacja pokazywała pustą listę modułów bez błędu kompilacji.

## Wiedza o środowisku (nie odkrywaj ponownie)

**egui 0.37 (zweryfikowane kompilacją i uruchomieniem):**

- `eframe::App` wymaga `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)` — **nie ma `update(ctx, frame)`**.
- `egui::SidePanel` i `egui::TopBottomPanel` **nie istnieją**; jest `egui::Panel::left/right/top/bottom(id)`, a `show(...)` przyjmuje `&mut Ui` (nie `&Context`). `CentralPanel::default().show(ui, ...)`.
- `egui_plot::Line::new(name, series)` — nazwa pierwsza; `Plot::new(id)` przyjmuje `impl AsIdSalt`.
- `Context::style_mut` → `global_style_mut`; `Line`/`HLine` łatwo pomylić (są w osobnych modułach).
- `Harness::run()` panikuje przy animowanych widgetach (spinner, progress bar) — w testach używaj `run_steps(n)`.

**Windows / build:**

- `eframe` 0.37 domyślnie używa **wgpu** (nie glow). Wariant `default-features = false, features = ["glow", ...]` jest o ~7 MB mniejszy, ale wymaga sprawnego OpenGL — decyzja architektoniczna, nie „optymalizacja”.
- Manifest `requireAdministrator` wchodzi tylko w release przez `/MANIFESTUAC` w `build.rs`. Plik `.manifest` + `/MANIFESTINPUT` daje błąd `manifest authoring error` (konflikt z domyślnym fragmentem linkera).
- Bez administratora: `\\.\PhysicalDriveN` → `Odmowa dostępu`, `Get-StorageReliabilityCounter` → brak dostępu do CIM. Inwentarz WMI działa bez admina.
- `Win32_VideoController.AdapterRAM` obcina VRAM do 4 GiB (uint32) — potwierdzone na RX 6700 XT 12 GB. VRAM bierz z DXGI/rejestru.
- Klasy, których nie ma / nie zwracają danych: `Win32_ProcessorPowerManagement` (nie istnieje), `MSAcpi_ThermalZoneTemperature` (na tej maszynie brak), `Win32_Fan` (brak instancji), `Win32_Battery` (desktop — 0 instancji; baterię testuj na fixture'ach).
- Temperatury rdzeni CPU, napięcia i wentylatory wymagają kodu w ring 0 — poza zakresem 1.0, planowane w Fazie 5 (helper LHM + PawnIO).
- Przenoszenie haseł Chrome/Edge przez kopiowanie plików nie działa (DPAPI per maszyna, ABE `v20`). Nie implementuj obejść ABE.
- `profiles.ini` Firefoksa czytaj jako UTF‑8 (nie CP1250), inaczej nazwy profili się rozjadą.

## Styl pracy

- Najpierw przeczytaj plik, potem go zmieniaj. Diff ma być minimalny i zgodny z otoczeniem.
- Nowa zależność wymaga uzasadnienia w opisie PR.
- Test najpierw dla logiki, której nie widać w UI (parsery, liczenie, agregacja); UI przez `egui_kittest` tylko jako smoke.
- Nie zostawiaj TODO, stubów ani fałszywych danych w kodzie dostarczonym jako gotowy. Brak możliwości implementacji → jawnie w opisie.
- Commit: `feat(module): hw.cpu — inwentarz procesora i live taktowanie`, `fix(storage): poprawne wyrównanie bufora dla 4Kn`.

## Checklista przed zgłoszeniem

- [ ] cztery bramki zielone (wklej wynik),
- [ ] testy sprzętowe `--ignored` wykonane albo jawnie zaznaczone jako niewykonane,
- [ ] brak `unsafe` poza `sk-win`, brak `unwrap()` na danych zewnętrznych,
- [ ] klucze i18n w `pl` i `en`,
- [ ] brak sekretów i danych osobowych w logach, fixture'ach i testach,
- [ ] dokumentacja zaktualizowana, jeśli zmienił się kontrakt.
