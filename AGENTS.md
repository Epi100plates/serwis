# AGENTS.md — zasady dla agentów pracujących w tym repozytorium

Przeczytaj w tej kolejności: `AGENTS.md` → `docs/PLAN.md` (decyzje i zakres) → `docs/MODULE_GUIDE.md` (jak dodać moduł).

## Czym jest ten projekt

SerwisKit — open‑source narzędzie serwisowe dla Windows 10/11 x64 w Rust z GUI (egui), zastępujące AIDA64 + HD Tune + CrystalDiskInfo + migrację profili przeglądarek. Jeden plik `.exe`, praca offline, moduły dokładane przez kontrybutorów.

## Komendy (zweryfikowane na tej maszynie: Rust 1.99.1 MSVC, Win11 26300)

```powershell
cargo fmt --all                       # formatowanie
cargo clippy --all-targets -- -D warnings
cargo test --all                      # testy bez sprzętu (mocki)
cargo test -p sk-modules --test hardware -- --ignored   # testy sprzętowe: prawdziwe WMI tej maszyny
cargo test -p sk-modules --test golden                  # golden widoków (kształt raportu)
cargo test -p sk-win -- --ignored     # testy sprzętowe warstwy Windows (rejestr/EDID/firmware)
cargo run -p sk-app                   # GUI
cargo run -p sk-app -- report --json out.json              # tryb CLI: raport JSON
cargo run -p sk-app -- report --json out.json --html out.html   # JSON + HTML
cargo build --release                 # artefakt: target\release\serwiskit.exe
```

Wszystkie cztery bramki (`fmt`, `clippy`, `test`, `build`) muszą być zielone przed zgłoszeniem pracy.

## Twarde reguły

1. `unsafe` wyłącznie w `crates/sk-win`. Moduł potrzebujący nowego wywołania systemowego zgłasza to, nie dopisuje FFI u siebie.
2. Zero sieci w kodzie produktu: brak HTTP, telemetrii, auto‑update w modułach.
3. Zero `unwrap()`/`expect()` na danych zewnętrznych (WMI, pliki, rejestry, I/O). Błąd → `ModuleError` z kontekstem.
4. Żadnych tekstów widocznych dla użytkownika poza i18n (`pl` + `en`). Dotyczy to też **wartości** w widoku: jeśli wartość jest zdaniem lub stanem (Tak/Nie, „Nieznane”), użyj `KeyValue::state(..., klucz)`; jeśli przyszła z WMI — to dana i ma być neutralna językowo (`2010-W46`, nie „2010, tydzień 46”).
5. Nie blokuj wątku UI: operacje > ~200 ms to `TaskDescriptor` w wątku roboczym.
6. Nie zmieniaj publicznego kontraktu (`sk-core`), `Cargo.toml` ani `docs/PLAN.md` w zadaniu modułowym — to wymaga zgody lidera i ADR.
7. Nie wymyślaj API, flag ani wersji. Sprawdź w dokumentacji Microsoft/crates.io; jeśli nie potwierdzone — napisz „niepotwierdzone”.
8. Operacje destrukcyjne (zapis na dysk, nadpisywanie danych) nigdy nie są domyślne i zawsze wymagają jawnego potwierdzenia.
9. Licencja projektu to **GPL-3.0-only**. Nowa zależność musi być GPL-kompatybilna (MIT, Apache-2.0, BSD, MPL-2.0 — tak; kod proprietary — nie). Nie dodawaj zależności o niejasnej licencji.
10. Wersja release ma manifest `requireAdministrator` (build.rs, tylko profil release). Nie usuwaj tej bramki i nie zakładaj, że proces działa bez uprawnień administratora — sprawdzaj to w kodzie, zamiast zgadywać.
11. **Żadnych zbędnych informacji.** Odbiorcą jest technik, który zna temat: nie opisuj w UI i dokumentacji tego, co widać na ekranie, nie tłumacz oczywistości („test tylko czyta dane”, „wymaga administratora” — to mówi plakietka uprawnień) i nie powtarzaj tej samej wartości w kilku miejscach. Nowy tekst ma nieść informację, której nie da się odczytać z widoku; jeśli nic nie wnosi — nie dodawaj go.
12. **Każda zmiana kończy się pushem.** Po zielonych bramkach: commit i `git push origin main`. Dotyczy to też dokumentacji, komentarzy i zmian w `.github/` — praca, która została tylko lokalnie, nie istnieje. CI (`.github/workflows/ci.yml`) uruchamia cztery bramki na `windows-latest` przy każdym pushu, więc czerwony wynik widać od razu; push odrzucony jako non‑fast‑forward rozwiązuj przez `git pull --rebase`, nigdy `--force` na `main`.

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
- **Wydanie startuje bez okna konsoli** — w `sk-app/src/main.rs` jest `#![cfg_attr(all(windows, not(debug_assertions), not(test)), windows_subsystem = "windows")]`. Każdy exe z podsystemem konsoli dostaje od Windows własne okno cmd obok GUI, dlatego podsystem zmienia się dopiero w release (build deweloperski ma konsolę: `cargo run`, paniki, testy).
  - Tryb CLI (`serwiskit report …`) dba o wyjście sam: `sk_win::ensure_console()` dołącza do konsoli wywołującego, a gdy to się nie uda (proces podniesiony przez UAC nie ma konsoli rodzica) tworzy własną (`AllocConsole`). Uchwyty przekierowane do pliku (`> log.txt`) zostają nietknięte.
  - Błąd startu GUI w release pokazuje `sk_win::show_error` (`MessageBoxW`). **Nie dodawaj `println!`/`eprintln!` w ścieżce GUI** — w wydaniu nie ma konsoli, a makra panikują na błędzie zapisu; `tracing` bez zainstalowanego subscribera jest bezpieczny.
  - Sprawdzenie podsystemu w gotowym exe: `[BitConverter]::ToUInt16($bytes, $pe + 24 + 0x44)` — `2` = GUI, `3` = konsola.
- Bez administratora: `\\.\PhysicalDriveN` → `Odmowa dostępu`, `Get-StorageReliabilityCounter` → brak dostępu do CIM. Inwentarz WMI działa bez admina.
- `Win32_VideoController.AdapterRAM` obcina VRAM do 4 GiB (uint32) — potwierdzone na RX 6700 XT 12 GB. VRAM bierz z DXGI/rejestru.
- Klasy, których nie ma / nie zwracają danych: `Win32_ProcessorPowerManagement` (nie istnieje), `MSAcpi_ThermalZoneTemperature` (na tej maszynie brak), `Win32_Fan` (brak instancji), `Win32_Battery` (desktop — 0 instancji; baterię testuj na fixture'ach).
- Temperatury rdzeni CPU, napięcia, moc pakietu i wentylatory z Super I/O są już czytane — wymagają podpisanego sterownika **PawnIO** (`sk-win/src/pawnio.rs`, `cpu.rs`, `superio.rs`); bez niego moduł pokazuje notkę, a nie błąd.
- Przenoszenie haseł Chrome/Edge przez kopiowanie plików nie działa (DPAPI per maszyna, ABE `v20`). Nie implementuj obejść ABE.
- `profiles.ini` Firefoksa czytaj jako UTF‑8 (nie CP1250), inaczej nazwy profili się rozjadą.

**Faza 1 — potwierdzone na tej maszynie (2026‑11‑18):**

- `Win32_PhysicalMemoryArray.MaxCapacity` jest w **kilobajtach** (134217728 KB = 128 GiB). Preferuj `MaxCapacityEx`.
- `root\wmi ProcessorPerformance` ma właściwości **małymi literami**: `frequency`, `percentage`, `power`, `InstanceName` = `PPM_Processor_N`. Nie ma `PercentFrequency`.
- `Win32_VideoController.AdapterRAM` = **4 293 918 720** B (4 GiB − 1 MiB) dla 12 GB karty. Od tej wartości traktuj ją jako obciętą; realny VRAM: `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\NNNN\HardwareInformation.qwMemorySize` (QWORD), nazwa adaptera w `DriverDesc`.
- `WmiMonitorID.ManufacturerName`/`UserFriendlyName`/`ProductCodeID` to tablice kodów UTF‑16 (16 znaków, zera na końcu) — dekoduj, nie pokazuj liczb.
- EDID monitora: `HKLM\SYSTEM\CurrentControlSet\Enum\<InstanceName bez sufiksu _N>\Device Parameters\EDID`. `InstanceName` z WMI (`DISPLAY\SAM0928\7&…&UID264_0`) różni się tylko sufiksem. Parser: `crates/sk-modules/src/edid.rs`, fixtures w `docs/fixtures/edid/`.
- `GetFirmwareType` (kernel32) → 2 = UEFI, 1 = BIOS; dostępne przez trait `MachineInfo`.
- Klasy baterii (`BatteryStaticData`, `BatteryFullChargedCapacity`, `BatteryStatus`) na desktopie zwracają „Invalid class” — moduł ma pokazać brak baterii, nie błąd. Liczba cykli wymaga `IOCTL_BATTERY_QUERY_INFORMATION` (niezaimplementowane).
- `MockWmi` wybiera odpowiedź po **najdłuższym** pasującym fragmencie zapytania (inaczej `Win32_PhysicalMemoryArray` łapie odpowiedź dla `Win32_PhysicalMemory`).
- Raport HTML powstaje z modelu `View` (`sk-app/src/html.rs`), nie z szablonów per moduł; wykresy to inline SVG. Nie dodawaj `askama`.
- Telemetria: próbki zbiera wątek roboczy (`sk-app/src/live.rs`) raz na sekundę **tylko gdy `system.live` albo `hw.sensors` jest otwarte**; bufor 600 próbek. `collect()` modułu nie próbkuje.
- Zapytania o klasy używane w kilku miejscach żyją w `sk_core::queries` (`WIN32_OPERATING_SYSTEM`, `WIN32_VIDEO_CONTROLLER`, `PROCESSOR_PERFORMANCE`, `WIN32_COMPUTER_SYSTEM`). Cache jest kluczowany po `(namespace, WQL)`, więc inna lista pól = drugie zapytanie w rundzie. Statystyki widać w raporcie (`wmi_cache`) — na tej maszynie 18 zapytań i 5 trafień przy 14 modułach (2026‑11‑20).
- Stany w komórkach tabeli: `Table::with_value_key(row, column, "common.value.digital")`. Nie wpisuj do komórki słów typu „Digital”/„None”.
- Golden: `crates/sk-modules/tests/golden/*.json` pilnują kształtu widoków `hw.cpu`, `hw.memory` i `storage.smart`; odświeżanie tylko świadome: `$env:UPDATE_GOLDEN = "1"; cargo test -p sk-modules --test golden`.
- Sprawdzanie GUI: `PrintWindow` **z flagą `PW_RENDERFULLCONTENT` (2)** przechwytuje treść renderowaną przez wgpu — sprawdzone na tej maszynie (`tools/screenshot-window.ps1`, działa też dla okna zasłoniętego przez inną aplikację). Bez tej flagi obraz wychodzi biały. Gdy okno ma być naprawdę widoczne, `tools/screenshot-gui.ps1` stawia je na wierzchu (`SetWindowPos(HWND_TOPMOST)`) i robi `CopyFromScreen`; samo `SetForegroundWindow` bywa blokowane przez okno pełnoekranowe. Testy UI nadal przez `egui_kittest`.

**Faza 2 — SMART (stan: 2026‑11‑18, zweryfikowane kompilacją i działaniem):**

- Dyski fizyczne: `CreateFileW(\\.\PhysicalDriveN, GENERIC_READ|GENERIC_WRITE, FILE_SHARE_READ|WRITE, OPEN_EXISTING)`. Bez podniesionego procesu → `ERROR_ACCESS_DENIED` (`0x80070005`, w `windows-rs` jako `err.code().0`). Zamknięcie: `CloseHandle` w `Drop`.
- `GENERIC_READ`/`GENERIC_WRITE` są w `Win32::Foundation`, nie w `Storage::FileSystem`; `BusType*` i `STORAGE_BUS_TYPE` w `Win32::Storage::FileSystem`; `ATA_PASS_THROUGH_EX` i `IOCTL_ATA_PASS_THROUGH` w `Win32::Storage::IscsiDisc`; deskryptory i NVMe — `Win32::System::Ioctl`. Wymagane features: `Win32_Storage_FileSystem`, `Win32_Storage_IscsiDisc`, `Win32_System_IO`, `Win32_System_Ioctl`.
- Deskryptor: `IOCTL_STORAGE_QUERY_PROPERTY` + `StorageDeviceProperty` (BusType, RemovableMedia); `StorageDeviceSeekPenaltyProperty` → `IncursSeekPenalty` (false = SSD). BusType: NVMe 17, SATA 11, ATA 3, USB 7, RAID 8, Spaces 16.
- NVMe health log: jeden bufor `8 (offset AdditionalParameters) + 40 (STORAGE_PROTOCOL_SPECIFIC_DATA) + 8 (zapas) + 512` = 568 B (niektóre sterowniki liczą `ProtocolDataOffset` od początku deskryptora); `StorageDeviceProtocolSpecificProperty` z fallbackiem na `StorageAdapterProtocolSpecificProperty`; `DataType = NVMeDataTypeLogPage (2)`, `ProtocolDataRequestValue = 0x02`, `ProtocolDataOffset = 40`, `ProtocolDataLength = 512`. Deskryptor odpowiedzi: `Version` i `Size` muszą być równe `size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>()` (48), dane pod `8 + ProtocolDataOffset`.
- Offsety strony zdrowia NVMe: 0 critical warning, 1–2 temperatura (K), 3 spare, 4 próg spare, 5 % zużycia, 48–63 data units written (×1000×512 B), 128–143 godziny, 144–159 unsafe shutdowns, 160–175 media errors, 176–191 wpisy logu, 192–195/196–199 minuty powyżej progu, 200–215 czujniki.
- ATA SMART: `IOCTL_ATA_PASS_THROUGH` + `ATA_PASS_THROUGH_EX` (48 B) i 512 B danych w **jednym** buforze wyrównanym do 8; `DataBufferOffset = 48`, `AtaFlags = ATA_FLAGS_DATA_IN`, `CurrentTaskFile = [funkcja, 0x01, 0x4F, 0xC2, 0x00, 0xA0, 0xB0, 0x00]`, gdzie funkcja 0xD0 = atrybuty, 0xD1 = progi.
- Atrybuty ATA: bajt 0 = rewizja, od bajtu 2 trzydzieści wpisów po 12 B (`id`, `flags` 2 B, `value`, `worst`, `raw` 6 B). Progi: ten sam układ, próg pod offsetem +1. Przekroczony próg (value ≤ threshold) = stan przedawaryjny; niezerowe 5/10/184/187/188/196/197/198/199 (`SUSPICIOUS_ATTRIBUTE_IDS`) = ostrzeżenie; 1 i 7 bywają niezerowe na zdrowym dysku. NVMe: bity critical warning 2/3 = zły stan, pozostałe bity = ostrzeżenie; zużycie ≥ 80 % = ostrzeżenie (100 % to wyczerpany zapas projektowy, nie awaria).
- Trait `StorageProbe` w `CollectCtx` (`describe`, `smart`), makiety `MockStorage` (`with_nvme`, `with_ata`, `with_denied`, `with_unavailable`). Parsery w `sk-modules/src/smart.rs`, transport w `sk-win/src/storage.rs`. Moduł `storage.smart` ma `Capability::Elevated`.
- `Win32_DiskDrive` jest czytane przez `queries::WIN32_DISK_DRIVE` w `hw.disk_inventory` i `storage.smart` — jedna stała = jedno zapytanie w rundzie.
- Program uruchomiony jako administrator (`requireAdministrator` z manifestu release) widzi dyski; `cargo test` bez podniesienia nie. Testy sprzętowe dysków muszą akceptować oba warianty (asercja na `elevated` w danych), a nie zakładać dane.
- Widok `storage.smart` jest w układzie CrystalDiskInfo: sekcja przeglądu (dysk, stan, temperatura, magistrala, typ, zużycie, godziny, zapisane dane) + sekcja na dysk: `Block::status` (duży, kolorowany stan) → notki → wskaźniki (`nvme_rows` z logu zdrowia / `ata_rows` wyliczone z atrybutów) → tożsamość → pełna tabela atrybutów ATA z kolumną „Ocena”. Kolory: `StatusLevel` (`Ok`/`Warn`/`Bad`/`Unknown`) w `Block::status` i `Table::with_cell_level`; mapowanie kolorów jest jedno (`sk-app/src/render.rs::status_color` dla GUI, klasy `lvl-*` w HTML).
- Ocena pojedynczego atrybutu = ta sama funkcja co ocena dysku (`smart::attribute_state`, agregacja `max`), inaczej kolumna „Ocena” rozjedzie się z banerem. Liczniki LBA ATA (atrybuty 241/242) mnożymy przez `BytesPerSector` z `Win32_DiskDrive`, nie przez stałe 512 B.
- Tabele w GUI są w `ScrollArea::horizontal` — szeroka tabela przewija się, zamiast ucinać kolumny poza ekranem (okno startuje z 1180×780 px, panel modułów 280 px).

**Faza 2 — benchmark powierzchni (stan: 2026‑11‑19, zweryfikowane kompilacją i GUI; odczyt na dysku wymaga admina):**

- `storage.bench` to benchmark odczytu jak w HD Tune: mapa prędkości (MB/s od pozycji w GB), min/średnia/maks, losowy odczyt 4K (czas dostępu, IOPS), lista nieczytelnych bloków. Tylko odczyt — zero zapisu. Test **próbkuje całą powierzchnię** i mieści się w wybranym limicie czasu (domyślnie minuta), więc na dysku 2 TB kończy się w minutę, a nie po godzinach.
- Podział: typy i magazyn w `sk-core/src/surface.rs` (`SurfaceParams`, `SurfaceSample`, `SurfaceScan`, `SurfaceAccess`, `SurfaceStore` + trait `SurfaceSource` w `CollectCtx`, `IdleSurface` dla CLI); transport w `sk-win/src/surface.rs::scan_surface` (jedyne miejsce z `ReadFile`/`SetFilePointerEx`); czysta analiza w `sk-modules/src/bench.rs` (`stats`, `chart`, `chart_points`, `outcome`, `format_duration`; moduł jest `pub`, bo podgląd w GUI rysuje ten sam wykres); moduł `modules/storage_bench.rs` tylko czyta `ctx.surface.state()`; wątek testu w `sk-app/src/bench.rs`, karta sterowania i odświeżanie 4×/s w `sk-app/src/app.rs`.
- Odczyt: `CreateFileW` z `FILE_FLAG_NO_BUFFERING` i `GENERIC_READ`; bufor, offset i długość muszą być wielokrotnością 4 KiB (pokrywa 512e i 4Kn). Rozmiar: `GetFileSizeEx`, fallback `IOCTL_DISK_GET_LENGTH_INFO`. Blok dobiera `SurfaceParams::new` do limitu 16k próbek (1 GiB → 1 MiB, 512 GB → 32 MiB, 2 TB → 128 MiB).
- **Próbkowanie po całej powierzchni.** Z limitem krok liczy `next_sample_offset`: pozostałą odległość dzieli przez liczbę kroków, które zmieszczą się w pozostałym czasie (czas kroku z pozycjonowaniem, EMA 0,7/0,3, margines `PLAN_SAFETY` = 0,9). Dzięki temu ostatnia próbka trafia na koniec dysku razem z terminem, a liczba próbek zależy od budżetu, nie od rozmiaru nośnika. Bez limitu to zwykły odczyt sekwencyjny całego zakresu. Po terminie `sweep` robi jeden skok na ostatni blok, żeby mapa zawsze sięgała końca (`time_limit_reached: true`), a `BenchOutcome::TimeLimit` pojawia się tylko wtedy, gdy końca nie udało się objąć.
- Błędy `ReadFile` są danymi (`SurfaceScan::errors`, limit 128; po 8 pod rząd faza się zatrzymuje), nie `Err`. Anulowanie to `AtomicBool` sprawdzany między blokami; wynik częściowy ma `cancelled: true` i stan `Warn`.
- Limit czasu: `SurfaceParams::time_limit_ms` (`DEFAULT_TIME_LIMIT_MS` = 60 s, 0 = bez limitu). `Deadlines` w `sk-win` dzieli budżet na fazy: faza losowa dostaje `RANDOM_PHASE_RESERVE_MS` (max ¼ limitu), resztę mapa. Karta w panelu ma **tylko** dysk i czas testu (15 s / 30 s / 60 s / bez limitu → `BenchSpeed` w `sk-app/src/app.rs`) plus Start/Anuluj i pasek postępu; zakresu nie wybierasz — zawsze cała powierzchnia. Statystyki są w widoku modułu, nie w panelu (zero powtórzeń), a z widoku usunięto notki, które tłumaczyły oczywistości (`note.readonly`, `note.idle`, `note.running`, `note.whole_disk`).
- Postęp przy limicie liczy się z zegara (`SurfaceState::Running.elapsed_ms`, `progress_percent`) — pasek dochodzi do 100 %, bo limit wyznacza koniec testu. Wiersz „Pokryty zakres” pokazuje pozycję ostatniej próbki (`SurfaceScan::covered_bytes`), a nie liczbę odczytanych bajtów.
- **Wykres ma z góry ustalony układ.** Oś X to pozycja w GB (1 GB = 1024³ B, jak etykiety rozmiarów), rozpięta na cały dysk przez `Chart::with_x_range(0.0, dysk_gb)`; renderer używa `default_x_bounds` + `x_axis_formatter`, więc podziałka pokazuje „100 GB, 200 GB…”, a wykres rysuje się od razu, zanim przyjdzie pierwsza próbka (`covered_bytes` nie steruje już skalą). Golden `storage_bench_view.json` pilnuje kształtu widoku (ma `x_min`/`x_max` i punkty w GB).
- Bez administratora test kończy się `SurfaceState::Failed` z „brak uprawnień: \\.\PhysicalDrive0” — moduł pokazuje notkę, nie błąd. `cargo test -p sk-win --lib -- --ignored surface` bez podniesienia akceptuje `AccessDenied`, więc **nie potwierdza** odczytu; do tego potrzebny release uruchomiony jako administrator.
- **Zapis wykresu do PNG.** Przycisk „Zapisz PNG” otwiera `egui::Modal` z podglądem (model dysku, pojemność, czas, średnia + wykres 320 px). Dopiero ten obszar trafia do pliku: `ctx.request_screenshot` oddaje **całą klatkę w pikselach urządzenia**, a `chart_image::crop` wycina prostokąt przeliczony z proporcji `image.size / ctx.content_rect()` (działa przy skalowaniu 125/150 %, bez czytania `pixels_per_point`). Modal, a nie widok modułu, bo wykres bywa przewinięty poza okno; podgląd jest zawsze w całości widoczny. Podgląd rysuje `render::ChartMode::Still` (bez podpowiedzi kursora), żeby plik nie zależał od myszy. Zapis: `chart_image::write_png` na crate `png` (już w grafie zależności przez `egui`, licencja MIT/Apache-2.0). Nazwa pliku: `wykres-<model>-<YYYYMMDD-HHMMSS>.png` w katalogu roboczym, model przepuszczony przez `file_slug` (tylko litery i cyfry). Termin `CHART_SHOT_TIMEOUT` = 3 s na wypadek ukrytego okna, gdy klatka nie wraca.
- Zrzuty: `docs/screenshot-bench.png` — realny pomiar na CT1000MX500 (931,5 GB): limit 60 s (przebieg 57,0 s), 491 próbek, pokryty zakres 931,5 GB / 931,5 GB, średnia 535,5 MB/s (359,3–556,3), 9 779 IOPS; `docs/screenshot-bench-chart.png` — plik zapisany przyciskiem „Zapisz PNG” z tego samego przebiegu.

**Faza 2 — skan nieczytelnych sektorów `storage.sectors` (stan: 2026‑11‑20, zweryfikowane kompilacją, CLI i realnym skanem z GUI; odczyt wymaga admina):**

- Odpowiednik Error Scan z HD Tune: mapa powierzchni (zielone/czerwone komórki) i lista nieczytelnych zakresów. Dwa tryby: **dokładny** (każda jednostka 4 MiB od początku do końca) i **szybki** (64 próbki × 32 MiB = 2 GiB rozrzucone po całej powierzchni). Podstawa z instrukcji HD Tune: pełny skan „scans the entire disk surface”, quick „only certain parts across the disk surface will be tested”.
- Podział: typy, plan i mapa w `sk-core/src/sector.rs` (`SectorParams`, `SectorPlan`/`plan`, `quick_probes`, `SectorScan`, `SectorState::map`, `SectorStore` + trait `SectorSource` w `CollectCtx`, `IdleSectors` dla CLI); transport w `sk-win/src/sector.rs::scan_sectors` (współdzieli `RawDevice` z `surface.rs` — od teraz `pub(crate)`); moduł `modules/storage_sectors.rs` tylko czyta `ctx.sectors.state()`; wątek `serwiskit-sector` w `sk-app/src/sector.rs`, karta i odświeżanie 4×/s w `app.rs`.
- Jednostka odczytu `SECTOR_UNIT_BYTES` = 4 MiB: jeden `ReadFile` = jeden krok postępu = jeden wpis na liście błędów. Pozycja błędu ma rozdzielczość 4 MiB (jak „Error at … MB” w HD Tune); bez bisection do 4 KiB, bo każdy nieudany odczyt wywołuje retry dysku, a na końcu żywotności to minuty.
- Błędy `ReadFile`, odczyt 0 B i odczyt częściowy są **danymi**: wpis `SectorError { offset_bytes, length_bytes, detail }` (kod Windows w hex), limit `MAX_SECTOR_ERRORS` = 128 i flaga `errors_truncated`; przebieg przerywa tylko błąd pozycjonowania. Anulowanie między jednostkami → wynik częściowy `cancelled`.
- Postęp liczą kroki (`steps_done`/`steps_total`), nie zegar — skan nie ma limitu czasu, bo ma pokryć całą powierzchnię. „Pozostało” liczy moduł z tempa dotychczasowego przebiegu. Bez administratora skan kończy się `Failed` z komunikatem, a moduł pokazuje notkę o uprawnieniach.
- Mapa: `Block::Map` z `SurfaceMap { cells, columns }` i `MapCell::Ok | Bad | Pending`; 64×64 = 4096 komórek (`MAP_CELLS`) niezależnie od pojemności (2 TB → 512 MiB na komórkę, 512 GB → 120 MiB), tryb szybki 8×8 (jedna komórka na próbkę). `Pending` = jeszcze nieodczytane, więc mapa rośnie w trakcie. GUI: `render.rs::map_ui` (8 px, `rect_filled`), HTML: inline SVG z klasami `map-ok`/`map-bad`/`map-pending`.
- Start skanu i start benchmarku blokują się nawzajem: obie karty korzystają z jednego wyboru dysku, a dwa testy tego samego nośnika fałszują wynik.
- Testy: 10 w `sk-core` (plan, mapa, postęp, magazyn, zgodność JSON), 6 w `sk-win` (na plikach tymczasowych z `FILE_FLAG_NO_BUFFERING`, w tym 1 `#[ignore]`), 11 modułu, 5 GUI/HTML (mapa w SVG, etykiety, smoke renderu). Cały zestaw po zmianach widoku czujników: **329 testów zielonych, 11 `#[ignore]`**.
- Dowód z tej maszyny: skan szybki na CT1000MX500SSD1 (release z manifestu): 2,0 GB w 4,2 s (średnio 515,5 MB/s), 64 próbki po 931,5 GB, 0 nieczytelnych zakresów, mapa 8×8 zielona (`docs/screenshot-sectors.png`). Test `#[ignore]` uruchomiony z podniesionego procesu odczytał pierwsze 8 MiB `PhysicalDrive0` (2/2 kroki, bez błędów) — bez podniesienia akceptuje wyłącznie `AccessDenied`.
- Nazwa `storage.sectors` zamiast planowanego w PLAN `storage.surface` — mówi, co test sprawdza.

**Faza 2 — czujniki `hw.sensors` (stan: 2026‑11‑20, zweryfikowane na RX 6700 XT i Ryzen 5 5600, w GUI i CLI):**

- Kontrakt: `sk-core/src/sensors.rs` (`SensorReading`, `SensorKind`, `DeviceKind`, `SensorStore`), trait `SensorsSource` (`readings()`, `history()`) w `CollectCtx`, makieta `MockSensors`. Transport: `sk-win/src/sensors.rs::WinSensors`; moduł `modules/hw_sensors.rs`. Moduł **nie** próbkuje — próbki 1/s dopisuje wątek roboczy (`SensorStore`) razem z `LiveHistory`, także przy otwartym `hw.sensors`. Temperatury dysków moduł czyta sam przez `ctx.storage` (SMART), bo tam jest jedyne źródło.
- **AMD ADL.** `atiadlxx.dll` jest w `System32` razem ze sterownikiem; wołanie `Cdecl`, kontekst z `ADL2_Main_Control_Create(callback, 1, &ctx)`, callback alokuje `HeapAlloc` na stercie procesu. `ADLAdapterInfo` ma **1572 B**; jedna karta ma kilka wpisów (u mnie 7, aktywne 2) — filtruj `iPresent`/`iExist` + `ADL2_Adapter_Active_Get` i deduplikuj po `(bus, device, function)`.
- **Na RDNA2 (Overdrive w wersji 8) stare API nie działa:** `ADL2_OverdriveN_Temperature_Get` i całe Overdrive5 zwracają `-8`/`-1`. Działa `ADL2_New_QueryPMLogData_Get` z `ADLPMLogDataOutput` = 4 B nagłówka + 256 × `{i32 supported, i32 value}` (2052 B); `size` ustaw na rozmiar struktury. Indeksy: 1 GFXCLK (MHz), 2 MEMCLK (MHz), 8 EDGE (°C), 9 MEM (°C), 14 FAN_RPM, 15 FAN_PERCENT, 19 GFX_ACTIVITY (%), 21 GFX_VOLTAGE (mV), 23 ASIC_POWER (W), 27 HOTSPOT (°C), 28 GFX TEMP. Potwierdzone: 56 °C, hotspot 60 °C, 663 RPM, 20 %, 33–37 W, 502/1984 MHz, 0,843 V. Fallback dla starszych kart: OverdriveN (temperatura w **milistopniach**, EDGE=1/MEM=2/HOTSPOT=7), OD5 fan (RPM=2, %, typ 1) i OD6 power (`raw >> 8` = W).
- **NVIDIA NVML:** `nvml.dll` w `System32`; `nvmlInit_v2`, `nvmlDeviceGetCount_v2`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetTemperature(handle, 0, …)`, `nvmlDeviceGetFanSpeed`, `nvmlDeviceGetPowerUsage` (mW), `nvmlDeviceGetClockInfo` (0 = graphics, 2 = mem), `nvmlDeviceGetUtilizationRates`. Brak biblioteki = brak odczytów, nie błąd. Na tej maszynie nie ma karty NVIDIA — ścieżka **nieprzetestowana sprzętowo**.
- **ACPI i bateria (WMI):** `SELECT CurrentTemperature, InstanceName FROM MSAcpi_ThermalZoneTemperature` (dziesiąte kelwina) i `SELECT Temperature, Voltage, InstanceName FROM BatteryStatus` (temperatura w dziesiątych K, napięcie w mV). Klasa ACPI istnieje na desktopie, ale **nie ma instancji**; `BatteryStatus` → `Invalid class`. Parsowanie wierszy (`parse_acpi_zones`, `parse_battery`) ma testy jednostkowe; sprzętowo na tej maszynie **niepotwierdzone**.
- **Czego nie ma:** nazw kanałów Super I/O per model płyty („wentylator 1” to numer nagłówka, nie „CPU fan”), wentylatorów EC w laptopach i per-rdzeniowych temperatur CPU (AMD ma Tctl/Tdie i CCD, Intel — pakiet). Nie dodawaj pozorowanych odczytów CPU z ACPI — na desktopie ich nie ma.
- **Widok jest pogrupowany i ma stałą skalę (2026‑11‑20).** `Block::group` (nowy wariant kontraktu: `CollapsingHeader` w GUI, `<details open>` w HTML) dzieli odczyty na podzespół → rodzaj wielkości (`Procesor → Temperatury / Napięcia / Moc`, `Karta graficzna → …`, `Płyta główna → Temperatury / Wentylatory`). Podzespół z jednym rodzajem (dysk: sama temperatura) nie dostaje poziomu pośredniego. Wykres temperatur: skala Y 0–100 °C (`Chart::with_y_range`), okno 120 s liczone wstecz od najnowszej próbki (`x = 0` = teraz), więc przebieg wchodzi od prawej i przesuwa się w lewo. Serie idą w kolejności podzespołów (CPU, GPU, płyta), kolor bierze `sk_core::view::series_color`, a przy ≥ 2 seriach renderery rysują legendę (próbka koloru + nazwa) — bez niej kolor linii nic nie mówi. HTML skaluje SVG z tych samych `x_min/x_max/y_min/y_max`.
- **Stały zakres osi Y w egui_plot ucina etykiety.** Przy `default_y_bounds(0, 100)` domyślny krok siatki daje linie gęstsze niż 20 px, a `egui_plot` pomija etykiety, które się nie mieszczą — na osi zostaje sama kreska. Do tego domyślna oś Y ma 14 px szerokości i przycina „100” do ostatniej cyfry. Rozwiązanie w `render.rs`: własny `y_grid_spacer` co 20 jednostek (tylko wewnątrz zakresu, bo etykiety 0/100 nachodzą na opisy osi) + `custom_y_axes(AxisHints::new(Axis::Y).min_thickness(30.0))`.
- Zrzuty: `docs/screenshot-sensors-chart.png` (wykres 0–100 °C z legendą i oknem 120 s) i `docs/screenshot-sensors-cpu.png` (grupy: Procesor, Karta graficzna).

**Faza 2 — sensory ring 0 przez PawnIO (stan: 2026‑11‑20, zweryfikowane na Ryzen 5 5600 + NCT6795D):**

- **Dlaczego PawnIO, a nie własny driver:** podpisu WHQL nie zdobędziemy, a WinRing0 jest na liście blokowanych (CVE‑2020‑14979). PawnIO to podpisany sterownik z interpreterem Pawn: aplikacja wgrywa **podpisany moduł** i woła jego funkcje. Decyzja zastąpiła planowany helper .NET z LHM.
- Instalacja: `PawnIO_setup.exe /S`, potem usługa `PawnIO` (auto) i `C:\Program Files\PawnIO\PawnIOLib.dll`; wersja w `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO` (`DisplayVersion`). **Aplikacja sterownika nie pobiera i nie instaluje** — wykrywa bibliotekę i pokazuje notkę, gdy jej brak.
- API biblioteki (`PawnIOLib.h`): `pawnio_open`, `pawnio_load(handle, blob, size)`, `pawnio_execute(handle, name, in[], in_size, out[], out_size, &returned)`, `pawnio_close`. Wszystko `extern "system"`, wynik to HRESULT (`0x80070005` = odmowa dostępu). Jedna sesja = jeden moduł; `sk-win/src/pawnio.rs` trzyma uchwyt i zwalnia go w `Drop`.
- Moduły (`.bin`, **LGPL‑2.1**, wydanie 0.2.12) są wkompilowane przez `include_bytes!` w `crates/sk-win/assets/pawnio/`: `AMDFamily17.bin` (SMN/MSR AMD), `LpcIO.bin` (porty I/O i Super I/O), `IntelMSR.bin`. Instalator PawnIO ich **nie** zawiera, a podpisany sterownik odrzuca niepodpisane moduły — dlatego muszą być w binarce razem z `COPYING`.
- Funkcje modułów: `ioctl_read_smn(offset)`, `ioctl_read_msr(index)` (64-bit), `ioctl_select_slot(0|1)`, `ioctl_find_bars()`, `ioctl_pio_inb/outb(port[, value])`, `ioctl_superio_inb/inw/outb(reg[, value])`.
- **Zawsze wychodź z trybu konfiguracji.** `detect` pisze `0xAA` na każdej ścieżce (także gdy chip ID jest nieznany), bo chip zostawiony w config mode nie przyjmuje kolejnego `0x87`. Dodatkowo sprawdzamy, że `0x07 == 0x0B`, a baza z `0x60` ma sens (≥ 0x100) — bez tego odczyt z przypadkowego portu mógłby trafić w inny układ. Raz, po zabiciu GUI w trakcie odczytu, baza NCT6795D sczytała się jako `0x0004`; przywrócenie `0x60/0x61 = 0x0A20` w trybie konfiguracji naprawiło odczyty. Nie zapisuj tej bazy bez potrzeby.
- **Kolejność dla Super I/O jest krytyczna:** `select_slot` → `pio_outb(0x2E|0x4E, 0x87)` ×2 → `superio_inb(0x20)` (ID) → **`ioctl_find_bars()`** (rejestruje dozwolone porty; przed wejściem w tryb konfiguracji zwraca `DEVICE_NOT_READY`, a każdy `pio_*` poza zakresem kończy się `ACCESS_DENIED`) → `superio_outb(0x07, 0x0B)` (LDN monitoringu) → `superio_inw(0x60)` ×2 (baza) → wyzeruj bit 4 w `0x28` (blokada I/O) → `pio_outb(0x4E, 0xAA)` (wyjście).
- Rejestry runtime są **bankowane**: `pio_outb(base+5, 0x4E)`, `pio_outb(base+6, bank)`, `pio_outb(base+5, rejestr)`, `pio_inb(base+6)`. Adres 0x4B0 = bank 4, rejestr 0xB0.
- AMD (rodzina 0x17/0x19/0x1A): temperatura Tctl z SMN `0x00059800` = `(raw >> 21) * 0,125 °C`, a gdy bit 19 (`0x80000`) lub bity 17:16 (`0x30000`) są ustawione — minus 49 °C. CCD: Zen 2/3 `0x00059954`, Zen 4 `0x00059B08`, Zen 5 `0x000599F0`; `raw & 0xFFF`, bit 11 = obecność, temperatura = `raw * 0,125 − 305 °C`. Napięcia SVI2: `0x0005A000` + `0x10` (rdzeń) / `+0x0C` (SoC) dla Zen 2/3/4, `+0x0C`/`+0x10` dla starszych; kod w bitach 23:16, `1,550 − 0,00625 * kod`, ważność z `0x0005A008` (bit 0 = brak telemetrii rdzenia). Moc pakietu: MSR `0xC0010299` (ESU w bitach 12:8) i licznik `0xC001029B` → `Δ * 2^−ESU / Δt`.
- Intel (ścieżka **nieprzetestowana sprzętowo**): TjMax z MSR `0x1A2` (bity 23:16), temperatura pakietu z `0x1B1` (bity 22:16 jako odległość od TjMax), napięcie z `0x198` (bity 47:32, krok 1/8192 V). Per-rdzeniowe odczyty `0x19C` wymagają przypięcia wątku do rdzenia — niezaimplementowane.
- Nuvoton NCT67xx: ID z rejestru `0x20` (`0xD3` = NCT6795D na tej płycie, rewizja `0x52`, baza `0x0A20` na slocie 0x4E). Wentylatory: licznik 13-bitowy `0x4B0, 0x4B2, …` → `1,35e6 / ((high << 5) | (low & 0x1F))`, `count >= 0x1FFF` = brak wentylatora. Temperatury: CPUTIN `0x075/0x076` (bit 7 połówki), SYSTIN `0x077/0x078`, AUXTIN0‑2 `0x079…0x07E`, AUXTIN3 `0x4A0/0x49E` (bit 6); wartość to `i8` plus 0,5 °C. PWM: `0x001, 0x003, 0x011, 0x013, 0x015, 0x017/0x029` (starsze) albo `…, 0x0A09, 0x0B09` (NCT6796+), procent = `raw / 2,55`; pokazujemy tylko dla nagłówków z wentylatorem.
- Dowód (2026‑11‑20): Tctl 53–62 °C, CCD 45–57 °C, rdzeń 0,99 V, SoC 1,17–1,18 V, pakiet 39–54 W; wentylatory 1080/1028 RPM, płyta 42–54 °C, PWM 30 %/45 %. `#[ignore]` testy: `cargo test -p sk-win --lib -- --ignored hardware`.
- `SensorsSource::probe()` → `SensorProbe { ring0, cpu, board }`: moduł wybiera notkę „zainstaluj PawnIO” (`ring0=false`) albo „ten procesor nie ma czujników” (`ring0=true, cpu=false`). `WinSensors::probe()` wypełnia to z realnych odczytów, nie z obecności pliku.
- Zrzut: `docs/screenshot-sensors-cpu.png` (Czujniki z CPU, Super I/O i GPU).

**PowerShell w tym środowisku (5.1, tylko `powershell.exe`):**

- Uruchamianie skryptów wymaga `-ExecutionPolicy Bypass -File` (polityka blokuje `.ps1`).
- `-Encoding utf8NoBOM` nie istnieje w 5.1 — pisz przez `[System.IO.File]::WriteAllText(path, text, New-Object System.Text.UTF8Encoding($false))`.
- Plik `.ps1` bez BOM jest czytany jako CP1250, a bajty UTF‑8 dla „—”, „…”, „→” trafiają w znaki cytowania PowerShella i **psują parser**. Skrypty trzymaj ASCII-only albo zapisuj z BOM.

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
- [ ] dokumentacja zaktualizowana, jeśli zmienił się kontrakt,
- [ ] commit i `git push origin main` (reguła 12).
