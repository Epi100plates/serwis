# SerwisKit

Narzędzie serwisowe dla Windows 10/11 x64 w Rust z GUI: diagnostyka sprzętu, testy dysków (SMART, prędkość, skan powierzchni) i — w kolejnych fazach — migracja profili przeglądarek. Zastępuje zestaw, który w serwisie wymaga dziś 3–5 programów komercyjnych (AIDA64, HD Tune, CrystalDiskInfo, narzędzia do profili).

Jeden plik `.exe`, bez instalatora, bez Node i WebView2, zero sieci. Moduły dokładają kontrybutorzy. Licencja **GPL‑3.0‑only**.

Status: **Faza 1 ukończona, Faza 2 w toku** — działa GUI, tryb CLI, jawny rejestr modułów (14) i raport JSON/HTML. W Fazie 2 zostały test zapisu (`Destructive`) i wspólny `TaskDescriptor`; dalej migracja przeglądarek i plików oraz dystrybucja (`docs/PLAN.md` §8).

## Co robi

| Moduł | Zawartość |
|---|---|
| `hw.cpu` | model, producent, socket, rdzenie/wątki, taktowanie nominalne i bieżące, cache L2/L3, architektura, wirtualizacja, SLAT |
| `hw.memory` | moduły (slot, pojemność, taktowanie, producent, part number, typ, napięcie), zajętość slotów, maksimum płyty, korekcja błędów |
| `hw.board` | producent, model, wersja, numer seryjny, etykieta SMBIOS |
| `hw.bios` | wersja i data BIOS‑u, SMBIOS, BIOS systemu, kontroler EC, tryb firmware (UEFI/BIOS) |
| `hw.gpu` | adaptery, sterownik i data, tryb pracy, realny VRAM z rejestru (WMI obcina do 4 GiB) |
| `hw.monitor` | model, producent, rok produkcji, przekątna, tryb natywny z EDID, maks. odświeżanie, wejście cyfrowe/analogowe |
| `hw.battery` | pojemność projektowa i rzeczywista, zużycie ogniwa, napięcie, moc ładowania, stan |
| `hw.disk_inventory` | dyski: model, interfejs, rozmiar, sektor, firmware, numer seryjny, partycje, stan |
| `hw.sensors` | temperatury (rdzenie CPU: Tctl/Tdie i CCD, rdzeń GPU, hotspot, pamięć, dyski, strefy ACPI, bateria, płyta), wentylatory w RPM i PWM, moc pakietu i karty, napięcia, taktowania i obciążenie GPU; min/max/średnia z próbek co sekundę i wykres temperatur |
| `storage.smart` | stan dysków w układzie CrystalDiskInfo: przegląd, baner oceny, wskaźniki NVMe albo ATA, pełna tabela atrybutów ATA z oceną każdego z nich |
| `storage.bench` | mapa prędkości odczytu (MB/s od pozycji w GB), min/średnia/maks, czas dostępu i IOPS losowego odczytu 4K; wybór dysku i czasu testu, próbkowanie całej powierzchni, zapis wykresu do PNG |
| `storage.sectors` | skan nieczytelnych sektorów jak Error Scan w HD Tune: mapa powierzchni, lista nieczytelnych zakresów, tryb szybki i dokładny |
| `system.info` | wersja systemu, model komputera, RAM, uptime, czas lokalny, typ komputera, hypervisor, uprawnienia |
| `system.live` | wykresy obciążenia CPU, taktowania i zużycia RAM — próbka co sekundę, historia 10 minut |

Raport: JSON (maszynowy, `schema_version`) i HTML (samodzielny plik, inline SVG, bez JavaScriptu). Dane zbierane są raz, więc oba pliki są spójne.

## Wymagania i uprawnienia

- Windows 10/11 x64. Do zbudowania: Rust 1.99 (MSVC) + Visual Studio Build Tools.
- Wydanie ma manifest `requireAdministrator`, bo SMART i testy dysków czytają `\\.\PhysicalDriveN`. Build deweloperski manifestu nie ma, żeby `cargo run` nie wywoływał UAC.
- Czujniki CPU i płyty wymagają zainstalowanego [PawnIO](https://pawnio.eu/) — aplikacja go nie pobiera ani nie instaluje: wykrywa bibliotekę, a bez niej pokazuje notkę. Raport działa dalej.
- Gdy kontroler nie przekazuje SMART (mostek USB, RAID) albo proces nie ma uprawnień, moduł pokazuje powód i stan „Nieznane” — nie zmyśla danych.

## Uruchomienie

```powershell
cargo build --release
.\target\release\serwiskit.exe
```

Gotowy plik jest w [Releases](https://github.com/Epi100plates/serwis/releases) — portable ZIP z `serwiskit.exe`, `LICENSE` i sumą SHA‑256.

Wydanie startuje bez okna konsoli — podwójne kliknięcie otwiera tylko okno aplikacji. `cargo run` zostaje przy konsoli, żeby widzieć `--help` i komunikaty błędów.

Tryb CLI (bez GUI):

```powershell
.\target\release\serwiskit.exe report --json raport.json
.\target\release\serwiskit.exe report --json raport.json --html raport.html
.\target\release\serwiskit.exe report --html raport.html --module hw.cpu --locale en
```

`--json` i `--html` można łączyć. Tryb `report` dołącza do konsoli, z której go uruchomiono, więc wydruk „Raport zapisany…” trafia do tego samego okna (przekierowanie `> log.txt` działa). Powłoka nie czeka na proces GUI — w skrypcie użyj `Start-Process -Wait` (PowerShell) albo `start /wait` (cmd).

## Rozwój

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all                                        # 329 testów, bez sprzętu
cargo test -p sk-modules --test hardware -- --ignored    # prawdziwe WMI tej maszyny
cargo test -p sk-win --lib -- --ignored hardware         # SMART, rejestr, PawnIO, Super I/O
cargo build --release
```

Cztery pierwsze bramki muszą być zielone przed zgłoszeniem pracy; to samo robi CI (`.github/workflows/ci.yml`, `windows-latest`) po każdym pushu.

**Po każdej zmianie commit i `git push origin main`.** Praca, która została tylko lokalnie, nie istnieje — także dla dokumentacji i poprawek w stylu. Szczegóły: [`AGENTS.md`](AGENTS.md) (reguła 12).

Wydanie: push tagu `v*` uruchamia `.github/workflows/release.yml` — build na `windows-latest`, ZIP z `serwiskit.exe`, `LICENSE` i `README.md`, sumy SHA‑256 i GitHub Release z treści [`CHANGELOG.md`](CHANGELOG.md). Wersję podnosi się w `workspace.package.version` i w changelogu, potem tag i push.

Struktura: `sk-core` (kontrakt modułów i widoków, typy testów dyskowych), `sk-win` (jedyne miejsce z `unsafe`: WMI z cache, rejestr, EDID, dyski i SMART, odczyt powierzchni, PawnIO), `sk-modules` (moduły i parsery), `sk-app` (binarka `serwiskit`, raport JSON/HTML, GUI i wątki testów).

Stos: Rust 1.99 · egui/eframe 0.37 · egui_plot 0.38 · `windows` 0.62 · `wmi` 0.18 · `serde` · `clap` · `chrono` · `include_dir` · `png`.

## Dokumentacja

- [`docs/PLAN.md`](docs/PLAN.md) — plan, decyzje architektoniczne, zakres, ryzyka, fazy.
- [`docs/MODULE_GUIDE.md`](docs/MODULE_GUIDE.md) — jak dodać nowy moduł (kontrakt, recepta, checklista).
- [`docs/fixtures/wmi/README.md`](docs/fixtures/wmi/README.md) — skąd pochodzą fixtures i jak je odświeżyć.
- [`AGENTS.md`](AGENTS.md) — zasady dla kontrybutorów i agentów AI.

## Licencja i granice

GPL‑3.0-only ([`LICENSE`](LICENSE)). Kod pochodny musi pozostać na GPL‑3.0; zależności są zgodne (MIT, Apache‑2.0, LGPL‑2.1 dla modułów PawnIO).

Narzędzie działa lokalnie i offline, nie wysyła danych użytkownika. Migracja danych wymaga zgody właściciela komputera. Nie implementujemy obejść zabezpieczeń przeglądarek (App‑Bound Encryption) ani blokad plików — to techniki charakterystyczne dla infostealerów.
