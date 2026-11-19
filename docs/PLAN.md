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

### 4.7 Start bez okna konsoli (decyzja 2026‑11‑19)

Okno cmd, które otwierało się obok GUI, nie pochodziło z kodu — Windows tworzy je dla **każdego** exe z podsystemem konsoli. Dopóki `serwiskit.exe` był takim exe, podwójne kliknięcie dawało dwa okna.

Warianty:

- A: zostawić podsystem konsoli i ukrywać okno, gdy proces jest jedynym właścicielem konsoli (`GetConsoleProcessList` + `ShowWindow`). Odrzucone: okno zdąży mrugnąć, a ukrycie konsoli odziedziczonej po `cmd` ukryłoby terminal użytkownika.
- B: dwa exe (GUI + `serwiskit-cli.exe`). Odrzucone: dwa artefakty przy obietnicy „jeden plik `.exe`” i duplikacja argumentów.
- **C (wybrany):** podsystem GUI tylko w wydaniu — `#![cfg_attr(all(windows, not(debug_assertions), not(test)), windows_subsystem = "windows")]` w `sk-app/src/main.rs`.

Konsekwencje, które kod obsługuje:

1. **Build deweloperski zostaje przy konsoli** — `cargo run`, `--help` i komunikaty paniki są widoczne, a `cargo test` nie zmienia podsystemu binarnego celu testowego.
2. **Tryb CLI sam dba o wyjście** (`sk_win::ensure_console`): najpierw sprawdza, czy `stdout`/`stderr` już mają uchwyty (przekierowanie `> log.txt` — nic nie zmienia), potem dołącza do konsoli wywołującego, a gdy to się nie uda (proces podniesiony przez UAC nie ma konsoli rodzica) tworzy własną przez `AllocConsole`.
3. **Błąd, którego nie ma gdzie wypisać, trafia do okienka** (`sk_win::show_error` → `MessageBoxW`) — dotyczy ścieżki GUI w wydaniu. Kod GUI nie może używać `println!`/`eprintln!`: bez konsoli makra panikują na błędzie zapisu, dlatego `main` wybiera okienko, gdy konsoli nie ma.
4. **Powłoki nie czekają na proces GUI tak jak na konsolowy.** W teście na tej maszynie PowerShell wrócił z promptem natychmiast (`Start-Process -Wait` czeka poprawnie); `cmd.exe` zachowuje się tak samo i wymaga `start /wait`. Raport i tak powstaje — asynchroniczny jest tylko prompt.

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

### Faza 1 — Telemetria (AIDA‑lite) (3–5 dni) — ✅ wykonane (2026‑11‑18)

Moduły: `hw.cpu`, `hw.memory`, `hw.board`, `hw.bios`, `hw.gpu`, `hw.monitor`, `hw.battery`, `hw.disk_inventory`, `system.info` (rozszerzony), `system.live` (wykresy CPU/RAM).

Zrealizowane:

- 10 modułów w `crates/sk-modules` (jeden plik = jeden moduł), 135 testów: jednostkowe, kontraktowe, golden widoków (`hw.cpu`, `hw.memory`) i 3 sprzętowe `#[ignore]`.
- **Cache WMI** w `sk-win` (`QueryCache`): jedno zapytanie na klasę na odświeżenie, `begin_round()` czyści go przed każdą rundą. Statystyki trafień/pudeł trafiają do raportu (`wmi_cache`); zmierzone na tej maszynie: 16 zapytań, 1 trafienie (`Win32_VideoController` dzielone przez `hw.gpu` i `hw.monitor`) na 10 modułów.
- **VRAM z rejestru**: `HardwareInformation.qwMemorySize` (potwierdzone: 12 868 124 672 B dla RX 6700 XT, gdy `AdapterRAM` raportuje 4 293 918 720 B). Gdy wpisu brak, moduł pokazuje „≥ 4 GB” i wyjaśnia obcięcie, zamiast podawać fałszywą liczbę.
- **EDID z rejestru**: własny parser (`crates/sk-modules/src/edid.rs`) daje rozdzielczość natywną, przekątną, rok produkcji i maksymalne odświeżanie (Samsung T19B300: 1366×768, 18,5″; Gigabyte GS27QCA: 2560×1440, 180 Hz).
- **Tryb firmware** (`GetFirmwareType`) i **pamięć adapterów** — trait `MachineInfo`, makieta `MockMachine`.
- **Wykresy live**: `Block::Chart`, `LiveSource` w kontekście, próbkowanie 1 s w wątku roboczym (25 próbek/25 s zmierzone w GUI), bufor pierścieniowy 600 próbek, `egui_plot` 0.38.
- **Raport JSON + HTML** z jednego przebiegu zbierania. HTML powstaje z tego samego modelu `View` co GUI (bez `askama`, bez JS), wykresy jako inline SVG, wszystkie wartości escapowane.
- **Fixtures WMI** z tej maszyny (`docs/fixtures/wmi/`, 17 klas) + `tools/collect-wmi-fixtures.ps1` z anonimizacją; fixtures EDID (`docs/fixtures/edid/`); bateria laptopa na syntetycznym fixture (potwierdzone, oznaczone w README).
- **Testy sprzętowe** `#[ignore]`: `cargo test -p sk-modules --test hardware -- --ignored` — 3 testy przechodzą na maszynie testowej (Ryzen 5 5600, RX 6700 XT, 4 dyski, 2 monitory).

**Odstępstwa od planu (świadome):**

| Plan | Rzeczywistość | Powód |
|---|---|---|
| raport HTML przez `askama` | renderer HTML nad modelem `View` w `sk-app/src/html.rs` | szablony per moduł łamałyby zasadę „moduł nie zna UI” i dublowały wiedzę; jeden renderer = JSON i HTML zawsze spójne, zero nowych zależności |
| bateria przez `IOCTL_BATTERY_QUERY_INFORMATION` | tylko klasy WMI (`BatteryStaticData`, `BatteryFullChargedCapacity`, `BatteryStatus`) | brak laptopa do weryfikacji IOCTL; kod nieprzetestowany na sprzęcie nie wchodzi. Liczba cykli i tak wymaga IOCTL — moduł mówi o tym wprost |
| rozdzielczość natywna przez `QueryDisplayConfig` | EDID z rejestru + parser w Rust | `QueryDisplayConfig` daje tryb bieżący, nie natywny; EDID jest jedynym wiarygodnym źródłem i dał się przetestować na fixtures |
| `system.live` jako moduł z własnym samplerem | sampler w wątku roboczym, moduł czyta `LiveSource` | moduł zostaje bezstanowy i testowalny bez zegara |
| nowy crate `sk-report` | raport w `sk-app` | przy jednym formacie wyjścia osobny crate to zbędna warstwa |
| tabela bez tłumaczonych komórek | `Table::value_keys` w kontrakcie | kolumna „Wejście” monitora to stan (cyfrowe/analogowe), a nie dana — bez tego mechanizmu proza trafiałaby do kodu (znalezione w recenzji) |

**Akceptacja — potwierdzona:** wszystkie moduły zwracają realne dane na tej maszynie (`report` → 10 modułów, 0 błędów), bateria przechodzi na fixture syntetycznym, raport HTML otwiera się w przeglądarce i zawiera komplet sekcji (zrzuty: `docs/screenshot.png`, `docs/report.png`), testy modułów na mockach zielone, testy `--ignored` przechodzą.

### Faza 2 — Dyski: SMART i benchmarki (4–7 dni) — 🚧 w toku (SMART gotowy 2026‑11‑18)

**Zrealizowane — `storage.smart`:**

- `sk-win/src/storage.rs`: `WinStorage` (trait `StorageProbe`). Deskryptor urządzenia (`IOCTL_STORAGE_QUERY_PROPERTY`: magistrala, nośnik wymienny, `IncursSeekPenalty` → SSD/HDD), strona logu zdrowia NVMe (log page 0x02 przez `StorageDeviceProtocolSpecificProperty` z fallbackiem na poziom adaptera) i SMART ATA **dwiema ścieżkami**: `IOCTL_ATA_PASS_THROUGH` oraz klasyczny `SMART_RCV_DRIVE_DATA`. Wszystkie uchwyty zamykane w `Drop`, `unsafe` wyłącznie tutaj.
- `sk-modules/src/smart.rs`: parsery NVMe (offsety logu, kelwiny→°C, jednostki 1000×512 B) i ATA (30 atrybutów × 12 B, progi z osobnego sektora), ocena stanu (`Good`/`Warning`/`Bad`) z jawnymi regułami: przekroczony próg producenta = `Bad`; niezerowe liczniki `SUSPICIOUS_ATTRIBUTE_IDS` (5, 10, 184, 187, 188, 196, 197, 198, 199) = `Warning`; NVMe: bity critical warning (degradacja/read-only = `Bad`, reszta = `Warning`), % zużycia ≥ 80 %, błędy medium, zapas bloków (100 % zużycia to wyczerpany zapas projektowy, nie awaria — zgodnie ze specyfikacją NVMe).
- `storage.smart`: przegląd wszystkich dysków (model, stan, temperatura, magistrala, typ, zużycie, godziny pracy, zapisane dane) + sekcja na dysk w układzie CrystalDiskInfo: **baner stanu** (`Block::status`, kolor zależny od oceny), wskaźniki z logu zdrowia NVMe albo wyliczone z atrybutów ATA (temperatura, zużycie, godziny, cykle zasilania, przealokowane/oczekujące/niepoprawne sektory, błędy CRC, odczytane i zapisane dane), tożsamość nośnika i pełna tabela atrybutów ATA z **59 przetłumaczonymi nazwami**, kolumną „Ocena” (OK / Uwaga / Zły, kolorowana per wiersz) i nieznanymi atrybutami producenta jako „Atrybut producenta”. `Capability::Elevated`.
- Degradacja bez wyjątku: brak admina → stan „Nieznane” + notka o uprawnieniach; mostek USB/kontroler RAID → notka informacyjna; czytnik bez nośnika → pomijany bez IOCTL; awaria jednego dysku nie psuje raportu.
- Kontrakt widoku rozszerzony o `Block::Status` i `Table::cell_levels` (kolory komórek) — wspólne dla GUI i raportu HTML; tabele szersze niż okno przewijają się poziomo zamiast ucinać kolumny.
- Testy: 43 nowe przy pierwszym przyroście SMART + 15 przy przebudowie widoku (parsery na budowanych buforach, moduł na `MockStorage`, 2 nowe testy makiet w `sk-core`, 7 testów warstwy Windows z testem układu ABI, golden `storage_smart_view.json`, 2 testy sprzętowe `#[ignore]`). Razem **190 testów zielonych, 6 `#[ignore]`**.

**Dowód z tej maszyny (release z manifestem, `serwiskit report --json/--html`):** 11 modułów, 0 błędów, cache WMI 2 trafienia/16 zapytań (współdzielone `Win32_DiskDrive`). Zrzut nowego widoku: `docs/screenshot-smart.png`.

| Dysk | Wynik |
|---|---|
| WD PC SN530 512 GB (NVMe) | stan **Dobry**, 54 °C, zużycie 3 %, 4946 h, 23,84 TB zapisane / 23,21 TB odczytane, 2183 cykle zasilania, 176 niebezpiecznych wyłączeń, 0 błędów medium, zapas bloków 100 % (próg 10 %) |
| CT1000MX500SSD1 (SATA) | kontroler odrzuca SMART obiema ścieżkami (`ABRT` / `ERROR_IO_DEVICE`); moduł pokazuje powód zamiast udawać dane |
| Microsoft Storage Space Device | magistrala RAID, brak SMART — notka „kontroler nie przekazuje” |
| Czytnik USB bez nośnika | pominięty bez wywołania IOCTL |

**Odstępstwa i decyzje (świadome):**

| Plan | Rzeczywistość | Powód |
|---|---|---|
| SMART ATA jednym IOCTL-em | dwie ścieżki: `IOCTL_ATA_PASS_THROUGH` + `SMART_RCV_DRIVE_DATA` | miniporty różnią się obsługą; druga ścieżka to ta, której używa CrystalDiskInfo. Obie odpowiedzi trafiają do raportu, gdy zawiodą |
| tylko odczyt SMART | po nieudanym odczycie jedna próba `SMART ENABLE` | część dysków ma SMART wyłączony w firmware (np. po secure erase); polecenie jest odwracalne i nie dotyka danych |
| fixtures SMART z prawdziwego dysku | syntetyczne bufory budowane w testach | odczyt wymaga administratora, więc fixture w CI byłby nieodtwarzalny; kształt danych pilnuje golden + testy parserów |
| ocena stanu „jak CrystalDiskInfo” | własne, jawne reguły w `smart.rs` | brak licencji na kopiowanie progów; reguły są opisane w kodzie i testach |
| wygląd „jak CrystalDiskInfo” | odtworzony układ (baner stanu, wskaźniki, tabela atrybutów z oceną), bez kodu i zasobów CDI | CrystalDiskInfo jest freeware, nie GPL — kopiujemy pomysł na układ, nie implementację |
| `TaskDescriptor` dla benchmarku | dedykowany wątek + `SurfaceStore` (`SurfaceSource` w `CollectCtx`) | `TaskDescriptor` nadal nie istnieje, a benchmark potrzebował jednego zadania z postępem i anulowaniem; pełny system zadań to osobna decyzja architektoniczna |
| benchmark zapisu w tej iteracji | tylko odczyt | zapis nadpisuje dane — wymaga `Destructive` i jawnego potwierdzenia (PLAN §7.2), więc nie jest częścią tego przyrostu |

**Zrealizowane — `storage.bench` (2026‑11‑19):**

- `sk-core/src/surface.rs`: typy pomiaru (`SurfaceParams`, `SurfaceSample`, `SurfaceScan`, `SurfaceAccess`, błędy odczytu), dobór bloku do zakresu (limit `MAX_SAMPLES` = 16k próbek), magazyn `SurfaceStore` + trait `SurfaceSource` w `CollectCtx`; `IdleSurface` dla trybu CLI.
- `sk-win/src/surface.rs`: `scan_surface` — sekwencyjny odczyt `\\.\PhysicalDriveN` z `FILE_FLAG_NO_BUFFERING` (bufor, offset i długość wyrównane do 4 KiB; dyski 512e i 4Kn), pomiar MB/s dziesiętnych, faza losowa 4K (czas dostępu min/średni/maks, IOPS QD1), błędy `ReadFile` jako dane (limit 128, stop po 8 pod rząd), anulowanie między blokami, rozmiar przez `GetFileSizeEx` z fallbackiem `IOCTL_DISK_GET_LENGTH_INFO`.
- `sk-modules/src/bench.rs` + `modules/storage_bench.rs`: statystyki min/średnia/maks, krzywa prędkości na osi pozycji w GB, ocena wyniku (Zakończony / Przerwany / Błędy odczytu / Niepełny / Limit czasu), tabela nieczytelnych bloków, widok bezczynny z listą dysków. `Capability::Elevated`, zero zapisu na dysk.
- **Próbkowanie całej powierzchni w limicie czasu (2026‑11‑20):** panel ma tylko dysk i szybkość testu (15 s / 30 s / 60 s / bez limitu), a `scan_surface` próbkuje cały nośnik: krok liczy `next_sample_offset` (pozostała odległość ÷ liczba kroków, które zmieszczą się w pozostałym czasie; czas kroku z pozycjonowaniem, EMA 0,7/0,3, margines 0,9). Ostatnia próbka trafia na koniec dysku razem z terminem, więc mapa jest pełna, a liczba próbek zależy od budżetu, nie od rozmiaru. Bez limitu — zwykły odczyt sekwencyjny. Postęp przy limicie liczy zegar (`SurfaceState::Running.elapsed_ms`), wiersz „Pokryty zakres” pokazuje pozycję ostatniej próbki.
- **Wykres w gotowym układzie:** oś X w GB (1 GB = 1024³ B) rozpięta na cały dysk przez `Chart::with_x_range`; renderer używa `default_x_bounds` + `x_axis_formatter`, więc podziałka pokazuje „100 GB”, „200 GB”…, a HTML skaluje SVG do `x_max`. Wykres rysuje się od pierwszej próbki, bez rozciągania skali.
- `sk-app`: panel testu (dysk, szybkość testu 15 s / 30 s / 60 s / bez limitu, Start/Anuluj, pasek postępu), dedykowany wątek `serwiskit-bench`, wspólny `SurfaceStore`, odświeżanie widoku 4×/s.
- Testy: 33 nowe jednostkowe (typy i magazyn, analiza mapy, moduł na `MockSurface`, wyrównanie/statystyki i odczyt z `FILE_FLAG_NO_BUFFERING` na pliku tymczasowym w `sk-win`, rozmiar dysku w panelu) + 21 dla limitu i próbkowania (podział budżetu, plan kroków `next_sample_offset`, rozrzut próbek na pliku tymczasowym, postęp z zegara, pokryty zakres, zgodność JSON ze starym formatem, etykiety szybkości, stały zakres osi X w modelu i w SVG), golden `storage_bench_view.json` (mapa w GB + `x_min`/`x_max`), 1 sprzętowy `#[ignore]`. Razem **242 testy zielone, 7 `#[ignore]`**; bramki `fmt`/`clippy`/`test`/`build --release` zielone.
- Dowód z GUI: `docs/screenshot-bench.png` — **realny pomiar** na CT1000MX500SSD1 (931,5 GB) uruchomiony z manifestu release (administrator): limit 60 s, 492 próbki, pokryty zakres 926,6 GB / 931,5 GB, odczytane 28,3 GB, ≈549 MB/s, wykres 0–931,5 GB z podziałką co 100 GB.

**Pozostałe w Fazie 2:** `storage.surface` (test powierzchni z retry i listą złych LBA zamiast mapy prędkości), `storage.write_test` (`Destructive`) i `TaskDescriptor` (wspólny mechanizm zadań z postępem/anulowaniem).

**Akceptacja (częściowa):** SMART NVMe potwierdzony na maszynie testowej (dane powyżej, admin z manifestu). SMART SATA **niepotwierdzony sprzętowo** — jedyny dysk SATA w tej maszynie jest za kontrolerem, który odrzuca oba polecenia; ścieżka ATA ma testy jednostkowe na buforach zgodnych ze standardem i czeka na weryfikację na sprzęcie, który przekazuje SMART. `storage.bench` ma potwierdzoną ścieżkę odmowy dostępu, realną listę dysków oraz **realny pomiar na CT1000MX500SSD1** (release z manifestu `requireAdministrator`): 60 s limitu, 492 próbki rozrzucone po 931,5 GB, pokryty zakres 926,6 GB, ≈549 MB/s, 200 odczytów losowych. Do powtórzenia na HDD i na dysku 2 TB (na tej maszynie ich nie ma) oraz przy anulowaniu w trakcie.

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
| Golden | `View` modułów dla fixture'ów (`tests/golden/*.json`, odświeżanie przez `UPDATE_GOLDEN=1`) | każdy commit, CI |
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
| Rozjazd dokumentacji i kodu | średni | MODULE_GUIDE aktualizowany w tym samym PR co kontrakt; test kontraktowy i golden pilnują reszty |
| Wiele instancji tego samego urządzenia (2 sockety CPU, 2 baterie) | niski | moduły pokazują pierwszą instancję (`decode_first`); monitory i dyski są listami. Rozbudowa, gdy pojawi się taki sprzęt do testów |
| `MaxCapacity`/`MaxCapacityEx` w SMBIOS bywają w bajtach zamiast KB (stare systemy) | niski | moduł preferuje `MaxCapacityEx` (u64); wartość nietypową widać w raporcie, ale nie jest korygowana heurystyką |

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
