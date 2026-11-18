# SerwisKit — plan działania

Dokument roboczy (stan: 2026‑11‑18). Wersja: 1.0. Nazwa kodowa: **SerwisKit** (zmienialna, patrz §11).

---

## 1. Cel i kontekst

Narzędzie dla serwisu komputerowego, open source, Windows 10/11 x64, w Rust, z GUI, które w jednym miejscu daje to, co dziś wymaga 3–5 komercyjnych programów:

| Dziś (komercyjne) | Zadanie | Moduł w SerwisKit |
|---|---|---|
| AIDA64 Extreme | pełna diagnostyka sprzętu (CPU, GPU, RAM, płyta, BIOS, monitor, bateria, dyski) | `hw.*` |
| HD Tune Pro | test prędkości, surface test, odczyt całej powierzchni, SMART | `storage.*` |
| CrystalDiskInfo | SMART i stan dysków w czytelnej formie | `storage.smart` |
| narzędzia do migracji profili | przenoszenie profili/zakładek/haseł między PC | `migrate.browsers` |

Główny scenariusz: **stary PC → nowy PC**. Technik uruchamia narzędzie na starym komputerze, robi pełny raport sprzętu (dowód stanu + wycena), eksportuje dane użytkownika do zaszyfrowanego pakietu, na nowym komputerze importuje dane i robi raport kontrolny.

Dwa tryby użycia tej samej logiki:

1. **GUI** (`serwiskit.exe`) — codzienna praca technika.
2. **CLI** (`serwiskit.exe report --json raport.json`) — automatyzacja, CI, testy bez GUI. Ten sam kod, inna powłoka.

---

## 2. Zakres: co wchodzi, co nie, i dlaczego

### Wchodzi do wersji 1.0

- Inwentarz sprzętu i systemu: CPU, pamięć RAM, płyta główna, BIOS/UEFI, karta graficzna, monitory (z EDID), bateria (laptopy), dyski, system operacyjny, uprawnienia, czas działania.
- Wykresy „live” dla obciążenia CPU/RAM/dysku i temperatury NVMe.
- Dyski: inwentarz, SMART (NVMe + ATA/SATA), stan zdrowia, przebieg, błędy.
- Benchmarki: sekwencyjny odczyt całej powierzchni (mapa prędkości), losowy odczyt 4K, czas dostępu; zapis tylko na jawnie wybranym dysku nie‑systemowym, po potwierdzeniu.
- Surface test: odczyt każdego bloku z retry i raportem błędnych LBA (bez zapisu w v1).
- Migracja przeglądarek: Chrome, Edge, Brave, Firefox (wykrywanie profili, zakładki, historia, ustawienia, hasła w granicach możliwości — §6).
- Pakiet przenoszenia `.skpkg` — zaszyfrowany hasłem, z manifestem i sumami kontrolnymi.
- Raport: JSON + HTML (do wydruku/PDF przez przeglądarkę), eksport wyników benchmarków.
- Modułowy system rozszerzeń + instrukcja dla kontrybutorów (`docs/MODULE_GUIDE.md`, `AGENTS.md`).

### Świadomie poza wersją 1.0

| Funkcja AIDA64/HWiNFO | Dlaczego nie w v1 |
|---|---|
| Temperatura rdzeni CPU, napięcia, obroty wentylatorów, odczyt EC/SMBus | Wymaga kodu w ring 0 (MSR/SMBus/EC). WMI tego nie wystawia i nie wystawi. **Decyzja z 2026‑11‑18: wchodzi w Fazie 5 jako helper .NET z LibreHardwareMonitorLib + PawnIO** (podpisany driver, nie WinRing0 — ten jest na liście blokowanych, CVE‑2020‑14979). W wersji 1.0 raport mówi wprost: „temperatura rdzeni wymaga modułu sensorów”. |
| Obejście App‑Bound Encryption Chrome (v20) przez `IElevator`/wstrzykiwanie DLL | To sygnatura infostealera — AV/EDR to oznaczy, a zaufanie do narzędzia serwisowego znika. Hasła z Chrome: tylko legalne ścieżki (§6.3). |
| Cloud, konta, telemetria, wysyłka danych | Narzędzie pracuje offline. Zero ruchu sieciowego z danymi klienta. |
| Linux/macOS | Poza zakresem; architektura (trait + provider) nie zamyka drzwi, ale nie robimy tego teraz. |
| Test zapisu na całej powierzchni domyślnie | Niszczy dane. Dostępny, ale wyłączony i za potwierdzeniem (§7.2). |

---

## 3. Dowody ze środowiska (co sprawdziłem, a nie założyłem)

Maszyna deweloperska: Windows 11 Enterprise 26300, Ryzen 5 5600, AMD RX 6700 XT, NVMe WD SN530 512 GB + 3× SATA SSD, Chrome/Edge/Brave/Firefox z realnymi profilami. Rust 1.99.1 (MSVC), Node 24.15, pnpm, WebView2 157.

| Sprawdzenie | Wynik | Wniosek dla projektu |
|---|---|---|
| `cargo build --release` (hello world) | OK, 0,38 s, 130 KB exe | toolchain MSVC gotowy |
| Otwarcie `\\.\PhysicalDrive0/4` bez admina | `Odmowa dostępu` | SMART i benchmark surowy **wymagają podniesienia uprawnień** |
| `Get-StorageReliabilityCounter` bez admina | `Klient nie ma dostępu do zasobu CIM` | to samo |
| WMI: `Win32_Processor`, `Win32_PhysicalMemory`, `Win32_DiskDrive`, `Win32_BIOS`, `Win32_BaseBoard`, `Win32_VideoController` | zwracają dane | inwentarz działa bez admina |
| WMI: `MSAcpi_ThermalZoneTemperature`, `Win32_Fan`, `Win32_Battery` | brak instancji na tym desktopie | temperatury/wentylatory przez WMI to iluzja; baterię testujemy na fixture'ach |
| `root\wmi: ProcessorPerformance` | 12 instancji | live taktowanie CPU da się pokazać bez admina |
| `Win32_VideoController.AdapterRAM` dla RX 6700 XT (12 GB) | 4 293 918 720 B (≈4 GiB) | potwierdzone obcięcie uint32 — VRAM trzeba brać z DXGI/rejestru |
| `Win32_ProcessorPowerManagement` | `Invalid class` | klasa nie istnieje, nie używać |
| Profile Chrome/Edge/Brave/Firefox | obecne, z `Login Data`, `Bookmarks`, `History` | realny materiał do testów migracji |
| `profiles.ini` Firefoksa odczytany jako `DomyĹ›lny` | mojibake | pułapka kodowania: pliki profili czytać jako UTF‑8, nie CP1250 |

---

## 4. Decyzje architektoniczne

### 4.1 GUI: egui/eframe 0.37 + egui_plot 0.38 (rekomendacja)

| Kryterium | egui | Tauri 2.12 | Slint 1.19 |
|---|---|---|---|
| Języki w repo | **tylko Rust** | Rust + TS/JS | Rust + DSL `.slint` |
| Zależności runtime | **brak** (jeden exe) | WebView2 | brak |
| Zmierzony rozmiar exe | 7,73 MB | 2,53 MB (+ ~100 MB WebView2 systemowo) | nie zmierzone |
| Wykresy live | **egui_plot, 9,3 mln pobrań** | JS (uPlot/ECharts), przez `Channel` + throttling | brak dojrzałych (330 pobrań) |
| Łatwość dokładania modułów | **wysoka** (nowy panel = Rust) | średnia (Rust + frontend) | średnia (+ DSL) |
| Testy bez GUI | **egui_kittest** (2,19 mln pobrań) | tauri-driver + WebDriver | backend „preliminary” |
| Licencja | **MIT/Apache** | MIT/Apache | GPLv3 albo royalty‑free z atrybucją |
| Ryzyko | **0.x, breaking changes** (potwierdzone: `run_simple_native` → `run_ui_native`) | dwa języki, IPC, WebView2 na Win10 | licencja + własne wykresy |

Wybór: **egui**. Uzasadnienie: wymóg „Rust + GUI + moduły dokładane przez innych” najmocniej ciąży w stronę jednego języka, jednego artefaktu i najlepszego ekosystemu wykresów live. Cena: pinowanie wersji i migracje przy podnoszeniu (świadomie akceptowane; wersje pinowane dokładnie, `=` w Cargo.toml).

Konsekwencja projektowa: **moduły nie znają egui**. Zwracają deklaratywny model widoku (`View`), a jeden generyczny renderer w `sk-app` rysuje sekcje, tabele i wykresy. Dzięki temu nowy moduł nie dotyka kodu UI, a widok da się testować jako JSON (golden test) bez okna.

### 4.2 Uprawnienia: manifest `requireAdministrator` (decyzja 2026‑11‑18)

Warianty brane pod uwagę:

- A: proces bez uprawnień + przycisk „uruchom ponownie jako administrator” — najmilsze dla użytkownika, ale dwa tryby pracy i ukryta ścieżka eskalacji w UI.
- **B (wybrany):** manifest `requestedExecutionLevel level="requireAdministrator"` — jedno zachowanie, SMART i testy dysków działają od startu. Cena: UAC przy każdym uruchomieniu i proces z uprawnieniami administratora robi też rzeczy, które ich nie potrzebują.
- C: osobny proces‑agent z named pipe — najbezpieczniejszy, odłożony, nie odrzucony.

Konsekwencje, które kod musi obsłużyć:

1. **Pliki zapisywane przez proces elevated dostają ACL administratora.** Pakiety, raporty i logi zapisujemy przez plik tymczasowy + jawne ustawienie uprawnień (dziedziczenie po katalogu użytkownika), inaczej użytkownik nie otworzy własnego raportu. Test regresyjny w Fazie 1.
2. **DPAPI działa poprawnie** — token elevated ma ten sam SID użytkownika, więc migracja haseł nie potrzebuje trybu nieelevated.
3. **Build deweloperski nie zawiera manifestu** (`build.rs` wstawia go tylko dla profilu release), żeby `cargo run` i testy nie wywoływały UAC na maszynie deweloperskiej. Wersja release zawsze wymaga administratora.
4. Wszystkie operacje uprzywilejowane pozostają za traitami w `sk-win`, więc ewentualne przejście na wariant C nie wymaga zmian w modułach.

### 4.3 Moduły: crate `sk-modules`, jeden plik = jeden moduł, jawny rejestr

```
crates/sk-modules/src/
  modules/mod.rs        # lista `mod hw_cpu;` + `registry.add::<...>()` (2 linie na moduł)
  modules/hw_cpu.rs     # cały moduł: meta + collect + view + testy
  modules/hw_gpu.rs
  modules/storage_smart.rs
```

Alternatywy i powody odrzucenia: osobny crate per moduł (wymaga linii w `Cargo.toml` aplikacji — konflikt gorszy niż w `modules/mod.rs`, wolniejszy build); build.rs generujący listę modułów (zero konfliktów, ale psuje podpowiedzi rust-analyzera — zły koszt dla ludzi); **auto-rejestracja przez `inventory` — odrzucona po teście**: na Windows z linkerm MSVC statyki z nieodwoływanych crate'ów bywają odrzucane i aplikacja pokazuje pustą listę modułów bez żadnego błędu kompilacji. Jawna lista jest deterministyczna i widoczna w jednym pliku.

Rejestracja jest jawna (`ModuleRegistry::add::<T>()`), a test kontraktowy w CI sprawdza, że każdy zarejestrowany moduł jest poprawny (unikalne `id`, klucze i18n w obu językach, `collect()` nie panikuje na pustym dostawcy).

### 4.4 Testowalność bez sprzętu: trait providers

Stan po Fazie 0 (kontrakt rozszerzany fazowo):

```rust
pub trait WmiProvider { fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>>; }
pub trait Clock        { fn now(&self) -> SystemTime; fn uptime_secs(&self) -> u64; }
pub trait HostEnv      { fn is_elevated(&self) -> bool; }

// Faza 2: pub trait RawDevice { ... } — surowe sektory, geometria dysków
// Faza 3: pub trait Dpapi     { ... } — CryptProtectData/CryptUnprotectData
```

Implementacja realna w `sk-win`, implementacja `mock` (`sk_core::testing`, feature `testing`) z fixture'ów JSON. Moduł dostaje `&dyn` przez `CollectCtx`. Efekt: 90% logiki modułów testuje się na `windows-latest` bez dysku, bez admina i bez przeglądarki. Testy na prawdziwym sprzęcie są oznaczone `#[ignore]` i uruchamiane ręcznie (`cargo test -- --ignored`).

### 4.5 Warstwy i zależności

```
sk-app (binarka: egui UI, clap CLI, renderer widoków)
  ├── sk-report (HTML/JSON)
  ├── sk-migrate (pakiety .skpkg, adaptery przeglądarek)
  ├── sk-storage (benchmark, surface, SMART-parsery)
  ├── sk-modules (moduły: hw.*, storage.*, migrate.*)
  │     └── sk-core (trait Module, View, Capability, i18n, błędy)
  └── sk-win (WMI, DeviceIoControl, battery, display, DPAPI — JEDYNE miejsce z `unsafe`)
```

Zasada: `unsafe` tylko w `sk-win`; moduły nie wolno, żeby nie mnożyć powierzchni ataku. `sk-core` nie zna Windows — to czyste typy.

### 4.6 Model danych i przepływ

```
Moduł.collect(ctx) ──► ModuleOutput { data: serde_json::Value, view: View }
                                  │
             ┌────────────────────┴─────────────────────┐
             ▼                                          ▼
   Raport (JSON/HTML, archiwum)              Renderer egui (sekcje/tabele/wykresy)
```

Długie operacje (benchmark, surface, kopiowanie profilu) nie są `collect()`: to `TaskDescriptor` z `run(&TaskCtx)` w wątku roboczym, `Progress` przez kanał `std::sync::mpsc` + `ctx.request_repaint()`, anulowanie przez `AtomicBool`. UI nigdy nie blokuje się na I/O.

---

## 5. Funkcje → źródło danych → uprawnienia → faza

Legenda uprawnień: **U** = zwykły użytkownik, **A** = administrator, **D** = administrator + jawne potwierdzenie (operacja destrukcyjna).

| Funkcja | Źródło | Uprawn. | Faza |
|---|---|---|---|
| Model/producent CPU, rdzenie/wątki, taktowanie max, cache, socket, architektura, wirtualizacja | `Win32_Processor` (WMI) | U | 1 |
| Live: obciążenie CPU, taktowanie per rdzeń | `ProcessorPerformance` (root\wmi), `Win32_PerfFormattedData_*` | U | 1 |
| RAM: moduły, pojemność, taktowanie, producent, part number, sloty, max | `Win32_PhysicalMemory`, `Win32_PhysicalMemoryArray` | U | 1 |
| Płyta główna, BIOS/UEFI, wersja, data, SMBIOS, EC | `Win32_BaseBoard`, `Win32_BIOS` | U | 1 |
| GPU: nazwa, sterownik, data, VRAM (64‑bit), tryb | `Win32_VideoController` + DXGI/registry (poprawka VRAM) | U | 1 |
| Monitory: nazwa, producent, rok, rozdzielczość natywna, odświeżanie, HDR | `WmiMonitorID`, `QueryDisplayConfig`, `DisplayConfigGetDeviceInfo` | U | 1 |
| Bateria: pojemność projektowa/pełna, wear, cykle, napięcie, moc ładowania/rozładowania, temperatura | `IOCTL_BATTERY_QUERY_INFORMATION`/`_STATUS`, fallback `BatteryStaticData`/`BatteryFullChargedCapacity`/`BatteryStatus` | U | 1 |
| System: wersja, build, uptime, czas lokalny, uprawnienia, czy VM | registry + `GetTickCount64` + WMI `Win32_ComputerSystem` | U | 1 |
| Dyski: model, serial, firmware, rozmiar, sektor fizyczny, interfejs, temperatura NVMe | `Win32_DiskDrive`, `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, `IOCTL_STORAGE_QUERY_PROPERTY` | U (część A) | 1 |
| SMART NVMe: temperatura, zużycie, spare, godziny, odczyty/zapisy, błędy medium, unsafe shutdowns | `IOCTL_STORAGE_QUERY_PROPERTY` + `NVMeDataTypeLogPage` + `NVME_LOG_PAGE_HEALTH_INFO` | A | 2 |
| SMART ATA/SATA: atrybuty, progi, raw (m.in. 05, C5, C6, 09, 0C) | `IOCTL_ATA_PASS_THROUGH` / `SMART_RCV_DRIVE_DATA` | A | 2 |
| Benchmark: sekwencyjny odczyt całej powierzchni + mapa prędkości | `\\.\PhysicalDriveN`, `FILE_FLAG_NO_BUFFERING`, wyrównanie do sektora fizycznego | A | 2 |
| Benchmark: losowy odczyt 4K (IOPS), czas dostępu, QD1 | jw. | A | 2 |
| Benchmark zapisu | jw. (nadpisuje dane) | **D** | 2 |
| Surface test (odczyt + retry + lista złych LBA) | jw. | A | 2 |
| Wykrywanie przeglądarek i profili, procesy działające | rejestr/ścieżki + `EnumProcesses` | U | 3 |
| Zakładki, historia, ustawienia, favikony, autofill | `Bookmarks`/`History`/`Preferences`/`Web Data`, `places.sqlite` | U | 3 |
| Hasła Firefox | `logins.json` + `key4.db` 1:1 do nowego profilu | U | 3 |
| Hasła Chromium (v10) | DPAPI na źródle → re‑szyfrowanie DPAPI na celu | U | 3 (eksperymentalnie) |
| Hasła Chromium (v20/ABE) | **tylko** eksport CSV z przeglądarki / import w UI | U | 3 (instrukcja) |
| Rozszerzenia | ID + reinstalacja z Web Store; `Local Extension Settings` przy zgodnej wersji | U | 3 |
| Migracja plików użytkownika (foldery osobiste), lista programów | system plików, rejestr `Uninstall` | U | 4 |
| Temperatury rdzeni CPU, napięcia, wentylatory | driver (PawnIO/LHM lub własny KMDF) | A + driver | 5 (opcjonalnie) |

Uwaga o SMART przez USB: mostki USB‑SATA często nie przekazują SMART, a NVMe po USB nie ma health logu. Moduł ma to zgłaszać jako „niedostępne przez ten mostek”, nie jako błąd.

---

## 6. Migracja danych — zakres realny, nie życzeniowy

### 6.1 Zasada: eksport na źródle → pakiet → import na celu

Nie kopiujemy profili 1:1 „w ciemno”, bo:

- DPAPI jest związane z użytkownikiem i maszyną — klucz zaszyfrowany na starym PC **nie odszyfruje się** na nowym ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)).
- Chrome 127+ ma App‑Bound Encryption (`v20`): klucz wymaga usługi uprzywilejowanej i weryfikacji aplikacji; przy przenoszeniu profili ABE „nie działa poprawnie” ([Google Security Blog](https://security.googleblog.com/2024/07/improving-security-of-chrome-cookies-on.html)).
- Skopiowanie `Local State` na cel nadpisuje klucze celu i psuje profil.

### 6.2 Pakiet `.skpkg`

- Kontener: ZIP (deflate) strumieniowo **wewnątrz** szyfrowania `age` 0.12 (hasło, scrypt) — plik jawny nigdy nie ląduje na dysku.
- Środek: `manifest.json` (schema_version, czas, hash maszyny źródłowej, lista elementów, liczby rekordów, SHA‑256 każdego payloadu) + `payload/`.
- Sekrety w pamięci: `zeroize`; do logu nigdy nie trafia wartość hasła.
- Import: weryfikacja manifestu i sum, tworzenie **nowego** profilu (nigdy nadpisywanie istniejącego), raport zgodności (co weszło, co pominięto i dlaczego).

### 6.3 Hasła — trzy poziomy prawdy

| Przypadek | Co robimy | Ograniczenie |
|---|---|---|
| Firefox | kopiujemy `logins.json` + `key4.db` do nowego profilu | jeśli użytkownik ma Primary Password, trzeba je znać |
| Chrome/Edge/Brave, profil `v10` | odszyfrowanie DPAPI na źródle → w pakiecie → re‑szyfrowanie kluczem DPAPI celu i zapis do `Login Data` | wymaga zgodności formatu — **walidacja eksperymentalna w Fazie 3**; jeśli zawiedzie, degradujemy do CSV |
| Chrome/Edge/Brave, profil `v20` (ABE) | instrukcja + import CSV z wbudowanego menedżera haseł | nie obchodzimy ABE (§2) |

Cookies domyślnie **nie** migrujemy (to dane uwierzytelniające; migracja tylko za osobnym przełącznikiem i z ostrzeżeniem).

### 6.4 Wymagania bezpieczeństwa procesu

- Przeglądarka musi być zamknięta; jeśli działa — komunikat i przerwanie (nie obchodzimy blokad plików; SQLite z WAL kopiujemy razem z `-wal`/`-shm`).
- Ekran zgody: „dane należą do mnie / mam upoważnienie”, brak trybu cichego dumpu, brak CLI do masowego wyciągania sekretów.
- Zero sieci. Log audytowy lokalny (kto/kiedy/jakie kategorie/ile rekordów/hash pakietu), bez wartości sekretów.

---

## 7. Bezpieczeństwo operacji destrukcyjnych

### 7.1 Zasady

1. Domyślnie wszystko jest **read‑only**.
2. Operacja destrukcyjna wymaga: wyboru konkretnego dysku (model + serial widoczne), wpisania frazy potwierdzającej, oraz odmowy dla dysku systemowego (wykrycie przez `IOCTL_STORAGE_GET_DEVICE_NUMBER` + porównanie z wolumenem systemowym).
3. Limit domyślny (np. 1 GiB testu zapisu), pełny test tylko po dodatkowym potwierdzeniu.
4. Anulowanie w każdej chwili; test kończy się czysto (zwolnienie uchwytów, zamknięcie logu).
5. Każda operacja D trafia do logu audytowego z parametrami.

### 7.2 Czego nie robimy

- Nie wysyłamy `FSCTL_DISMOUNT_VOLUME` na wolumen systemowy.
- Nie piszemy po `\\.\PhysicalDriveN` bez jawnego wyboru i potwierdzenia.
- Nie obchodzimy blokad plików przeglądarek i nie wstrzykujemy DLL do procesów.

---

## 8. Plan fazowy

Każda faza kończy się działającym, przetestowanym przyrostem. Kolejność jest tak dobrana, żeby Faza 0 dała kontrakt modułów, zanim powstanie ich dużo.

### Faza 0 — Fundament — ✅ wykonane (2026‑11‑18)

Zrealizowane:

- Repozytorium: `git init`, `.gitignore`, `LICENSE` (GPL‑3.0, pełny tekst), `README.md`, `AGENTS.md`, `docs/MODULE_GUIDE.md`, `clippy.toml` (testy mogą używać `unwrap`/`expect`), `rust-toolchain.toml` (1.99.1), `rustfmt.toml`.
- Workspace Cargo: `crates/sk-core`, `crates/sk-win`, `crates/sk-modules`, `crates/sk-app` (binarka `serwiskit`), wersje pinowane dokładnie, profile release (`lto`, `opt-level="z"`, `strip`, `panic="abort"`).
- `sk-core`: `Module`/`ModuleMeta`/`Capability`/`Group`/`ModuleOutput`, `View`/`Section`/`Block`, `ModuleError`, `ModuleRegistry`, i18n (`pl`/`en` z `include_dir`, 37 kluczy), makiety `Harness`/`MockWmi`/`FixedClock`/`MockHost`.
- `sk-win`: `WinWmi` (WMI przez `wmi` 0.18, połączenia per‑wątek), `SystemClock` (`GetTickCount64`), `WinHostEnv` (token elevation) — jedyne miejsce z `unsafe`.
- `sk-modules`: `system.info` (OS, model, RAM, uptime, uprawnienia) z 7 testami i kompletem kluczy i18n.
- `sk-app`: egui 0.37 (API `App::ui` + `egui::Panel`), lista modułów z rejestru, generyczny renderer `View`, wątek roboczy + `mpsc`, przełącznik PL/EN, tryb CLI `report --json`, atomowy zapis raportu.
- Manifest: `/MANIFESTUAC:level='requireAdministrator'` tylko w release (`build.rs`); build debug bez UAC.
- CI: `.github/workflows/ci.yml` (`fmt --check`, `clippy -D warnings`, `test`, `build --release`, artefakt).
- Bramki lokalne: 35 testów zielonych, clippy czysty, release 8,04 MB z wkompilowanym `requireAdministrator`.

**Odstępstwa od planu (świadome):**

| Plan | Rzeczywistość | Powód |
|---|---|---|
| rejestracja modułów przez `inventory` | jawny `ModuleRegistry::add::<T>()` | linker MSVC gubił statyki z nieodwoływanych crate'ów — test kontraktu pokazał pusty rejestr |
| manifest z pliku `app.manifest` + `/MANIFESTINPUT` | `/MANIFESTUAC` w argumentach linkera | scalanie z domyślnym fragmentem linkera kończyło się `manifest authoring error` |
| test UI smoke od razu | odłożony do Fazy 1 | spike pokazał, że `Harness::run()` panikuje przy animowanych widgetach (trzeba `run_steps`) |

**Akceptacja — potwierdzona:** okno GUI pokazuje realne dane z tej maszyny (zrzut: `docs/screenshot.png`), `serwiskit report --json out.json` tworzy poprawny raport JSON (2 973 B, 1 moduł, 0 błędów), CI gotowe do uruchomienia po pushu.

### Faza 1 — Telemetria (AIDA‑lite) (3–5 dni)

Moduły: `hw.cpu`, `hw.memory`, `hw.board`, `hw.bios`, `hw.gpu`, `hw.monitor`, `hw.battery`, `hw.disk_inventory`, `system.info` (rozszerzony), `system.live` (wykresy CPU/RAM).

- Cache zapytań WMI (jedno zapytanie na klasę na odświeżenie, nie na moduł).
- Wykresy live: sampler 1 s w wątku roboczym, bufor pierścieniowy, `request_repaint` z throttle.
- Raport JSON + HTML (szablon `askama`), eksport z GUI.
- Fixtures WMI z tej maszyny + fixtures baterii z laptopa (do zebrania; do tego czasu mock syntetyczny).

**Akceptacja:** wszystkie moduły zwracają realne dane na maszynie testowej; bateria przechodzi na fixture; raport HTML otwiera się i zawiera komplet sekcji; testy modułów na mockach zielone; `--ignored` testy sprzętowe przechodzą na tej maszynie.

### Faza 2 — Dyski: SMART i benchmarki (4–7 dni)

- `storage.smart`: NVMe health log + ATA SMART, parsery atrybutów, ocena stanu, czytelne etykiety PL/EN.
- `storage.bench`: sekwencyjny odczyt całej powierzchni (mapa prędkości, koszyki 1 GiB), losowy 4K, czas dostępu; live wykres; eksport CSV/JSON.
- `storage.surface`: odczyt bloków z retry, timeout per I/O, lista błędnych LBA, wznowienie od offsetu.
- `storage.write_test`: zapis, `Capability::Destructive`, domyślnie wyłączony.
- Obsługa braku uprawnień: jasny komunikat + przycisk restartu jako admin.

**Akceptacja:** SMART NVMe i SATA czytelne na maszynie testowej (z adminem); benchmark odczytu daje sensowną mapę (>500 MB/s SSD SATA, >1 GB/s NVMe); surface test na 240 GB SSD kończy się raportem bez błędów; test zapisu odmawia na dysku systemowym; wszystko testowalne na mocku bez admina w CI.

### Faza 3 — Migracja przeglądarek (5–8 dni)

- Wykrywanie: Chrome/Edge/Brave/Vivaldi/Opera/Firefox (+ Firefox MSIX), profile, wersje, działające procesy.
- Eksport: zakładki, historia, ustawienia, favikony, autofill; Firefox hasła 1:1; Chromium v10 eksperymentalnie; CSV jako ścieżka awaryjna.
- Pakiet `.skpkg` (age + zip + manifest + sumy), import do nowego profilu, raport zgodności, log audytowy.
- Audyt: brak sieci, brak logowania sekretów, `zeroize`.

**Akceptacja:** eksport z Chrome → import na tej samej maszynie do nowego profilu → zakładki i historia widoczne po uruchomieniu; to samo dla Edge i Firefox; hasła Firefox działają; pakiet otwiera się tylko z hasłem; próba eksportu przy działającej przeglądarce kończy się czytelnym komunikatem.

### Faza 4 — Migracja plików i inwentarz programów (3–5 dni)

Foldery osobiste z mapowaniem ścieżek, lista programów (rejestr `Uninstall`), rozmiary i sumy kontrolne, wznawianie po przerwaniu, raport „co przenieść ręcznie”.

### Faza 5 — Sensory głębokie: helper LibreHardwareMonitor + PawnIO (zaplanowana)

Cel: temperatury rdzeni CPU, napięcia, obroty wentylatorów, pobór mocy CPU (RAPL) — to, czego WMI nie daje.

Wybrany wariant (decyzja 2026‑11‑18): **helper .NET** jako osobny proces, komunikujący się z `serwiskit` przez stdin/stdout (JSON), korzystający z `LibreHardwareMonitorLib` (MPL‑2.0) i podpisanego `PawnIO` zamiast WinRing0.

Zadania:
1. Proces `lhm-helper` (C#/.NET) publikujący drzewo sensorów jako JSON; wersjonowany kontrakt schematu.
2. Strona Rust: wykrywanie helpera, timeouty, restart, brak helpera = `Block::Note` w module `hw.sensors` (raport działa dalej).
3. Instrukcja instalacji PawnIO + ostrzeżenie: wymaga uprawnień administratora i podpisanego drivera.
4. Testy: brak helpera nie psuje raportu; odpowiedź niezgodna ze schematem jest odrzucana.

Uwaga licencyjna: LHM jest MPL‑2.0 (kompatybilne z GPL‑3.0), ale helper pozostaje osobnym procesem — nie linkujemy go statycznie.

Akceptacja: na maszynie testowej moduł pokazuje temperaturę CPU i obroty wentylatorów; bez helpera raport nadal jest kompletny poza tą sekcją.

### Faza 6 — Dystrybucja (2–3 dni)

Portable ZIP + instalator (Inno Setup lub NSIS), podpis Authenticode (bez podpisu Smart App Control blokuje nieznane aplikacje na czystym Win11), GitHub Releases, CHANGELOG, wersjonowanie SemVer, README z zrzutami ekranu.

---

## 9. Testy i bramki jakości

| Poziom | Co | Kiedy |
|---|---|---|
| Unit | parsery SMART, liczenie wear/health, agregacja benchmarku, model widoku, i18n | każdy commit, CI |
| Kontrakt modułów | unikalne `id`, klucze i18n w `pl`+`en`, `collect()` nie panikuje na mocku, widok serializuje się | każdy commit, CI |
| Golden | `View` i raport JSON/HTML dla fixture'ów (snapshot) | każdy commit, CI |
| Integracyjne (sprzęt) | realne WMI, SMART, benchmark, migracja — `#[ignore]` | ręcznie, na maszynie serwisowej |
| UI smoke | `egui_kittest`: okno startuje, lista modułów się rysuje, klik w moduł nie panikuje | CI |
| Bezpieczeństwo | test, że operacja D bez potwierdzenia jest odrzucana; test, że log nie zawiera sekretów | CI |

Bramki: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`, `cargo build --release`. Wszystko w CI na `windows-latest`.

---

## 10. Ryzyka i mitigacje

| Ryzyko | Wpływ | Mitigacja |
|---|---|---|
| egui 0.x łamie API przy podnoszeniu | średni | wersje pinowane dokładnie (`=`), aktualizacja jako osobne zadanie z pełnym testem |
| Brak admina = połowa funkcji nie działa | wysoki | czytelne oznaczenia `Capability`, przycisk restartu jako admin, testy na mockach |
| SMART nie działa przez USB/RAID | średni | wykrywanie i komunikat „niedostępne przez ten kontroler”, nie udawanie danych |
| WMI wolne i czasem kłamie (VRAM, status dysku) | średni | cache, źródła alternatywne (DXGI, IOCTL), jawne „źródło danych” w raporcie |
| Destrukcyjny test zapisu | krytyczny | wyłączony domyślnie, potwierdzenie frazą, blokada dysku systemowego, log audytowy |
| ABE blokuje hasła Chromium | średni | CSV/eksport przeglądarki, jasna komunikacja, brak obejść |
| Sterowniki jądra (Faza 5) | wysoki | poza 1.0; jeśli wejdzie — tylko podpisany, udokumentowany, opt‑in |
| Pliki zapisane przez proces elevated mają ACL administratora — użytkownik ich nie otworzy | średni | zapis przez plik tymczasowy + jawne ACL z katalogu użytkownika; test regresyjny w Fazie 1 |
| RODO przy danych klienta | średni | offline, minimalizacja, szyfrowanie pakietu, log audytowy, docs dla serwisu |
| Rozjazd dokumentacji i kodu | średni | MODULE_GUIDE aktualizowany w tym samym PR co kontrakt; test kontraktowy pilnuje reszty |

---

## 11. Decyzje zatwierdzone (2026‑11‑18)

| # | Decyzja | Wybór | Konsekwencja |
|---|---|---|---|
| 1 | GUI | **egui/eframe 0.37 + egui_plot 0.38** | jeden język, jeden `.exe`, brak Node i WebView2 |
| 2 | Uprawnienia | **manifest `requireAdministrator`** | UAC przy starcie; obsługa ACL dla zapisywanych plików (§4.2) |
| 3 | Hasła | **Firefox pełne, Chromium v10 eksperymentalnie, ABE przez eksport CSV** | walidacja v10 w Fazie 3 na realnym profilu |
| 4 | Licencja | **GPL‑3.0** (`LICENSE`, SPDX `GPL-3.0-only`) | kod pochodny musi być GPL; zależności: egui MIT/Apache, `age` MIT/Apache, `rusqlite` MIT — kompatybilne |
| 5 | Język UI | **polski + angielski** (klucze i18n w `pl`/`en`) | test kontraktowy pilnuje kompletności kluczy |
| 6 | Sensory głębokie | **helper .NET LibreHardwareMonitor + PawnIO w Fazie 5** | poza 1.0; wymaga .NET runtime, admina i podpisanego drivera |
| 7 | Nazwa | **SerwisKit** | pakiet `serwiskit`, binarka `serwiskit.exe` |

Uwaga do licencji: GPL‑3.0 nie blokuje użycia `age` (MIT/Apache‑2.0), `rusqlite` (MIT), `egui` (MIT/Apache‑2.0) ani dynamicznego wywoływania `nvml.dll`/`atiadlxx.dll` przez FFI. Gdyby projekt miał kiedyś zostać zamknięty, trzeba by przepisać własny kod — nie zmienić licencji.

---

## 12. Mierniki sukcesu wersji 1.0

- Jeden plik `.exe` (plus opcjonalny instalator), działa na czystym Win11 bez instalowania runtime'ów.
- Raport sprzętu kompletny na tyle, że technik nie musi uruchamiać AIDA64 do wyceny.
- SMART NVMe i SATA czytelny i zrozumiały bez słownika.
- Test prędkości daje mapę powierzchni w < 3 min dla 512 GB SSD (odczyt), z live wykresem.
- Migracja Chrome/Edge/Firefox: zakładki + historia + ustawienia przenoszone w < 2 min, z raportem zgodności.
- Nowy moduł dodaje się w ≤ 1 dzień według `MODULE_GUIDE.md`, z testami, bez dotykania UI.
- CI zielone na `windows-latest`, zero `unsafe` poza `sk-win`.

---

## 13. Źródła (kluczowe)

- [CryptProtectData — DPAPI jest per‑user/per‑machine](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)
- [Google: App‑Bound Encryption w Chrome 127+](https://security.googleblog.com/2024/07/improving-security-of-chrome-cookies-on.html)
- [Praca z dyskami NVMe (IOCTL_STORAGE_QUERY_PROPERTY, health log)](https://learn.microsoft.com/en-us/windows/win32/fileio/working-with-nvme-devices)
- [IOCTL_ATA_PASS_THROUGH](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddscsi/ni-ntddscsi-ioctl_ata_pass_through)
- [File buffering — wymogi NO_BUFFERING i wyrównania](https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering)
- [IOCTL_DISK_GET_DRIVE_GEOMETRY_EX](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_disk_get_drive_geometry_ex)
- [IOCTL_BATTERY_QUERY_STATUS / BATTERY_STATUS](https://learn.microsoft.com/en-us/windows/win32/power/ioctl-battery-query-status)
- [DISPLAYCONFIG_TARGET_DEVICE_NAME](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-displayconfig_target_device_name)
- [Polityka podpisywania driverów jądra](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/kernel-mode-code-signing-policy--windows-vista-and-later-)
- [CVE‑2020‑14979 (WinRing0)](https://nvd.nist.gov/vuln/detail/CVE-2020-14979), [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor), [PawnIO](https://pawnio.eu/)
- [SQLite WAL — spójność kopii](https://sqlite.org/wal.html), [Chromium `user_data_dir.md`](https://github.com/chromium/chromium/blob/main/docs/user_data_dir.md)
- [egui/eframe](https://crates.io/crates/eframe), [egui_plot](https://crates.io/crates/egui_plot), [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/)
