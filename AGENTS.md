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
- Telemetria: próbki zbiera wątek roboczy (`sk-app/src/live.rs`) raz na sekundę **tylko gdy `system.live` jest otwarte**; bufor 600 próbek. `collect()` modułu nie próbkuje.
- Zapytania o klasy używane w kilku miejscach żyją w `sk_core::queries` (`WIN32_OPERATING_SYSTEM`, `WIN32_VIDEO_CONTROLLER`, `PROCESSOR_PERFORMANCE`, `WIN32_COMPUTER_SYSTEM`). Cache jest kluczowany po `(namespace, WQL)`, więc inna lista pól = drugie zapytanie w rundzie. Statystyki widać w raporcie (`wmi_cache`) — na tej maszynie 16 zapytań i 1 trafienie na 10 modułów.
- Stany w komórkach tabeli: `Table::with_value_key(row, column, "common.value.digital")`. Nie wpisuj do komórki słów typu „Digital”/„None”.
- Golden: `crates/sk-modules/tests/golden/*.json` pilnują kształtu widoków `hw.cpu`, `hw.memory` i `storage.smart`; odświeżanie tylko świadome: `$env:UPDATE_GOLDEN = "1"; cargo test -p sk-modules --test golden`.
- Sprawdzanie GUI: `PrintWindow` **nie** przechwytuje treści renderowanej przez wgpu (dostajesz biały obraz). Użyj `CopyFromScreen` po `SetForegroundWindow` albo `egui_kittest` w testach.

**Faza 2 — SMART (stan: 2026‑11‑18, zweryfikowane kompilacją i działaniem):**

- Dyski fizyczne: `CreateFileW(\\.\PhysicalDriveN, GENERIC_READ|GENERIC_WRITE, FILE_SHARE_READ|WRITE, OPEN_EXISTING)`. Bez podniesionego procesu → `ERROR_ACCESS_DENIED` (`0x80070005`, w `windows-rs` jako `err.code().0`). Zamknięcie: `CloseHandle` w `Drop`.
- `GENERIC_READ`/`GENERIC_WRITE` są w `Win32::Foundation`, nie w `Storage::FileSystem`; `BusType*` i `STORAGE_BUS_TYPE` w `Win32::Storage::FileSystem`; `ATA_PASS_THROUGH_EX` i `IOCTL_ATA_PASS_THROUGH` w `Win32::Storage::IscsiDisc`; deskryptory i NVMe — `Win32::System::Ioctl`. Wymagane features: `Win32_Storage_FileSystem`, `Win32_Storage_IscsiDisc`, `Win32_System_IO`, `Win32_System_Ioctl`.
- Deskryptor: `IOCTL_STORAGE_QUERY_PROPERTY` + `StorageDeviceProperty` (BusType, RemovableMedia); `StorageDeviceSeekPenaltyProperty` → `IncursSeekPenalty` (false = SSD). BusType: NVMe 17, SATA 11, ATA 3, USB 7, RAID 8, Spaces 16.
- NVMe health log: jeden bufor `8 (offset AdditionalParameters) + 40 (STORAGE_PROTOCOL_SPECIFIC_DATA) + 512`; `StorageDeviceProtocolSpecificProperty` z fallbackiem na `StorageAdapterProtocolSpecificProperty`; `DataType = NVMeDataTypeLogPage (2)`, `ProtocolDataRequestValue = 0x02`, `ProtocolDataOffset = 40`, `ProtocolDataLength = 512`. Deskryptor odpowiedzi: `Version` i `Size` muszą być równe `size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>()` (48), dane pod `8 + ProtocolDataOffset`.
- Offsety strony zdrowia NVMe: 0 critical warning, 1–2 temperatura (K), 3 spare, 4 próg spare, 5 % zużycia, 48–63 data units written (×1000×512 B), 128–143 godziny, 144–159 unsafe shutdowns, 160–175 media errors, 176–191 wpisy logu, 192–195/196–199 minuty powyżej progu, 200–215 czujniki.
- ATA SMART: `IOCTL_ATA_PASS_THROUGH` + `ATA_PASS_THROUGH_EX` (48 B) i 512 B danych w **jednym** buforze wyrównanym do 8; `DataBufferOffset = 48`, `AtaFlags = ATA_FLAGS_DATA_IN`, `CurrentTaskFile = [funkcja, 0x01, 0x4F, 0xC2, 0x00, 0xA0, 0xB0, 0x00]`, gdzie funkcja 0xD0 = atrybuty, 0xD1 = progi.
- Atrybuty ATA: bajt 0 = rewizja, od bajtu 2 trzydzieści wpisów po 12 B (`id`, `flags` 2 B, `value`, `worst`, `raw` 6 B). Progi: ten sam układ, próg pod offsetem +1. Przekroczony próg (value ≤ threshold) = stan przedawaryjny; niezerowe 5/187/196/197/198/199 = ostrzeżenie; 1 i 7 bywają niezerowe na zdrowym dysku.
- Trait `StorageProbe` w `CollectCtx` (`describe`, `smart`), makiety `MockStorage` (`with_nvme`, `with_ata`, `with_denied`, `with_unavailable`). Parsery w `sk-modules/src/smart.rs`, transport w `sk-win/src/storage.rs`. Moduł `storage.smart` ma `Capability::Elevated`.
- `Win32_DiskDrive` jest czytane przez `queries::WIN32_DISK_DRIVE` w `hw.disk_inventory` i `storage.smart` — jedna stała = jedno zapytanie w rundzie.
- Program uruchomiony jako administrator (`requireAdministrator` z manifestu release) widzi dyski; `cargo test` bez podniesienia nie. Testy sprzętowe dysków muszą akceptować oba warianty (asercja na `elevated` w danych), a nie zakładać dane.

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
- [ ] dokumentacja zaktualizowana, jeśli zmienił się kontrakt.
