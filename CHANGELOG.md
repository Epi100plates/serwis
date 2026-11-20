# Zmiany w SerwisKit

Wersjonowanie SemVer. Wydanie publikuje workflow [`.github/workflows/release.yml`](.github/workflows/release.yml): buduje `serwiskit.exe` na czystym `windows-latest`, pakuje ZIP, liczy sumy SHA‑256 i tworzy GitHub Release. Uruchamiają go dwa zdarzenia: push tagu `v*` oraz push na `main` — w drugim przypadku tylko wtedy, gdy dla wersji z `Cargo.toml` brakuje wydania albo nie ma ono załączników (tak odtwarza się wydanie bez ruszania tagu). Wydanie powstaje jako szkic i workflow publikuje je jawnie. Ten plik jest treścią wydania, więc wpis musi być aktualny **przed** pushnięciem tagu.

## 0.1.0 — 2026‑11‑20

Pierwsze publiczne wydanie: Faza 1 ukończona, Faza 2 w toku. Jeden plik `serwiskit.exe`, bez instalatora, bez Node/WebView2, zero sieci.

### Dodane

- GUI (egui 0.37, wgpu) i tryb CLI `report --json/--html`, jawny rejestr 14 modułów, raport JSON (`schema_version` 1) oraz HTML (samodzielny plik, inline SVG, bez JavaScriptu).
- Moduły: `hw.cpu`, `hw.memory`, `hw.board`, `hw.bios`, `hw.gpu`, `hw.monitor`, `hw.battery`, `hw.disk_inventory`, `hw.sensors`, `storage.smart`, `storage.bench`, `storage.sectors`, `system.info`, `system.live`.
- SMART NVMe i ATA/SATA w układzie CrystalDiskInfo: przegląd nośników, baner oceny, wskaźniki z logu zdrowia NVMe albo z atrybutów ATA, pełna tabela atrybutów z oceną każdego z nich.
- Benchmark powierzchni: mapa prędkości na całą powierzchnię w wybranym limicie czasu (15/30/60 s lub bez limitu), losowy odczyt 4K (IOPS, czas dostępu), zapis wykresu do PNG.
- Skan nieczytelnych sektorów: tryb szybki (próbki po całej powierzchni) i dokładny (jednostka 4 MiB), mapa 64×64, lista złych zakresów.
- Czujniki: GPU (AMD ADL PMLog, NVIDIA NVML), temperatury dysków z SMART, strefy ACPI, bateria, a temperatury rdzeni CPU, napięcia, moc pakietu i Super I/O przez podpisany sterownik PawnIO.

### Wymagania

- Windows 10/11 x64. Wydanie ma manifest `requireAdministrator` (SMART i testy dysków czytają `\\.\PhysicalDriveN`).
- Czujniki CPU i płyty wymagają zainstalowanego [PawnIO](https://pawnio.eu/) — aplikacja go nie pobiera ani nie instaluje, tylko wykrywa; bez niego pokazuje notkę, a raport działa dalej.

### Weryfikacja

- Bramki zielone: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` (329 testów, 11 `#[ignore]` sprzętowych), `cargo build --release`.
- Release startuje bez okna konsoli (podsystem GUI). Tryb `report` dołącza do konsoli wywołującego; powłoka nie czeka na proces GUI, więc w skrypcie użyj `Start-Process -Wait` albo `start /wait`.
- Czujniki CPU, płyty i GPU potwierdzone na Ryzen 5 5600 + RX 6700 XT (Tctl 53–62 °C, pakiet 39–54 W, wentylatory 1080/1028 RPM, PWM 30 %/45 %).

### Ograniczenia

- SMART ATA **niepotwierdzony sprzętowo** — kontroler w maszynie testowej odrzuca oba polecenia (ścieżka ma testy na buforach zgodnych ze standardem). Ścieżka Intela przez PawnIO nieprzetestowana (brak maszyny).
- Brak podpisu Authenticode — Smart App Control na czystym Windows 11 może zablokować plik.
- Poza zakresem wydania: test zapisu (`Destructive`), wspólny `TaskDescriptor`, migracja profili przeglądarek i plików.
