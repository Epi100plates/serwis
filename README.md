# SerwisKit

Narzędzie serwisowe dla Windows 10/11 x64: diagnostyka sprzętu (jak AIDA64), testy dysków i SMART (jak HD Tune / CrystalDiskInfo) oraz migracja profili przeglądarek między komputerami. Jeden plik `.exe`, praca offline, moduły dokładane przez kontrybutorów.

Projekt open source. Status: **Faza 1 ukończona, Faza 2 w toku** — działa GUI + CLI + jawny rejestr modułów, 11 modułów (telemetria AIDA‑lite, wykresy live, SMART dysków NVMe i ATA/SATA) i raport JSON/HTML. Kolejne fazy: `docs/PLAN.md` §8.

![Okno SerwisKit — telemetria na żywo](docs/screenshot.png)

Stan dysków (SMART) w układzie jak CrystalDiskInfo: stan i temperatura każdego nośnika w przeglądzie, baner oceny, wskaźniki z logu zdrowia NVMe lub z atrybutów ATA i pełna tabela atrybutów z oceną każdego z nich:

![Stan dysków (SMART) — przegląd, baner stanu i wskaźniki](docs/screenshot-smart.png)

Tabela atrybutów ATA z kolumną „Ocena” — zrzut z makiety (`MockStorage`), bo kontroler SATA tej maszyny nie przekazuje SMART:

![Tabela atrybutów ATA z oceną — dane syntetyczne](docs/screenshot-smart-ata-synthetic.png)

Raport HTML wygenerowany tym samym zbieraniem danych co GUI (zrzut: `docs/report.png`).

## Dlaczego

W serwisie komputerowym ta sama czynność wymaga dziś 3–5 programów komercyjnych: AIDA64 do wyceny i raportu, HD Tune do testu dysku, CrystalDiskInfo do SMART, osobne narzędzia do przenoszenia profili i haseł. SerwisKit robi to w jednym oknie, offline, z raportem, który można wydrukować i dołączyć do protokołu.

## Co już działa (Faza 1 + SMART z Fazy 2)

| Moduł | Zawartość |
|---|---|
| `hw.cpu` | model, producent, socket, rdzenie/wątki, taktowanie nominalne i bieżące (liczniki wydajności), cache L2/L3, architektura, wirtualizacja, SLAT |
| `hw.memory` | moduły (slot, pojemność, taktowanie, producent, part number, typ, napięcie), zajętość slotów, maksimum płyty, korekcja błędów |
| `hw.board` | producent, model, wersja, numer seryjny, etykieta SMBIOS |
| `hw.bios` | wersja i data BIOS‑u, SMBIOS, BIOS systemu, kontroler EC, tryb firmware (UEFI/BIOS) |
| `hw.gpu` | adaptery, sterownik i data, tryb pracy, **realny VRAM** z rejestru (WMI obcina do 4 GiB) |
| `hw.monitor` | model, producent, rok produkcji, przekątna, **tryb natywny z EDID**, maks. odświeżanie, wejście cyfrowe/analogowe |
| `hw.battery` | pojemność projektowa i rzeczywista, zużycie ogniwa, napięcie, moc ładowania, stan (na desktopie: czytelna degradacja) |
| `hw.disk_inventory` | wszystkie dyski: model, interfejs, rozmiar, sektor, firmware, numer seryjny, partycje, stan |
| `storage.smart` | stan dysków w układzie CrystalDiskInfo: przegląd (stan, temperatura, magistrala, typ, zużycie, godziny pracy, zapisane dane), baner oceny, wskaźniki NVMe (zapas bloków, % zużycia, odczyt/zapis, komendy hosta, cykle, błędy medium) lub ATA (temperatura, zużycie, przealokowane/oczekujące/niepoprawne sektory, CRC), **pełna tabela atrybutów ATA z tłumaczonymi nazwami i oceną każdego atrybutu** |
| `system.info` | wersja systemu, model komputera, RAM, uptime, czas lokalny, typ komputera, hypervisor, uprawnienia |
| `system.live` | wykresy obciążenia CPU, taktowania i zużycia RAM — próbka co sekundę, historia 10 minut |

Raport: JSON (maszynowy, `schema_version`) i HTML (samodzielny plik, inline SVG, bez JavaScriptu).

SMART wymaga uprawnień administratora (manifest release je zapewnia). Gdy kontroler nie przekazuje SMART (mostek USB, RAID, niektóre porty SATA), moduł pokazuje powód i stan „Nieznane” — nie zmyśla danych.

## Zakres (wersja 1.0)

- **Dyski:** SMART NVMe i ATA/SATA (gotowe), sekwencyjny odczyt całej powierzchni z mapą prędkości, losowy odczyt 4K, czas dostępu, surface test (odczyt), test zapisu tylko za jawnym potwierdzeniem.
- **Migracja:** wykrywanie profili Chrome/Edge/Brave/Firefox, zakładki, historia, ustawienia, hasła w granicach możliwości systemu, zaszyfrowany pakiet `.skpkg`.
- **Raport:** JSON i HTML (gotowe w Fazie 1).

Czego nie ma i dlaczego: temperatury rdzeni CPU, napięcia i obroty wentylatorów wymagają sterownika jądra — świadomie poza wersją 1.0 (`docs/PLAN.md` §2). Rozdzielczość natywna monitora pochodzi z EDID; HDR i CTA‑861 są poza Fazą 1.

## Stos

Rust 1.99 (MSVC) · egui 0.37 / eframe 0.37 / egui_plot 0.38 (GUI, jeden exe, bez Node i WebView2) · `windows` 0.62 · `wmi` 0.18 · `serde` · `clap` · `chrono` · `include_dir` (wkompilowane tłumaczenia). Kolejne fazy dołożą `rusqlite` i `age`.

Struktura: `sk-core` (kontrakt modułów i widoków), `sk-win` (jedyne miejsce z `unsafe`: WMI z cache, rejestr, EDID, firmware, dyski i SMART), `sk-modules` (moduły + parsery SMART), `sk-app` (binarka `serwiskit`, raport JSON/HTML).

Wersja release ma manifest `requireAdministrator` — SMART i testy dysków wymagają administratora. Build deweloperski manifestu nie zawiera, żeby `cargo run` nie wywoływał UAC.

## Budowanie

```powershell
cargo build --release
.\target\release\serwiskit.exe
```

Wymagania: Rust (toolchain MSVC) + Visual Studio Build Tools. Tryb headless:

```powershell
.\target\release\serwiskit.exe report --json raport.json
.\target\release\serwiskit.exe report --json raport.json --html raport.html
.\target\release\serwiskit.exe report --html raport.html --module hw.cpu --locale en
```

`--json` i `--html` można łączyć; dane zbierane są raz, więc oba pliki są spójne.

Bramki jakości (wszystkie muszą być zielone):

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo test -p sk-modules --test hardware -- --ignored   # prawdziwe WMI tej maszyny
```

## Dokumentacja

- [`docs/PLAN.md`](docs/PLAN.md) — plan działania, decyzje architektoniczne, zakres, ryzyka, fazy.
- [`docs/MODULE_GUIDE.md`](docs/MODULE_GUIDE.md) — jak dodać nowy moduł (kontrakt, recepta, checklista).
- [`docs/fixtures/wmi/README.md`](docs/fixtures/wmi/README.md) — skąd pochodzą fixtures i jak je odświeżyć.
- [`AGENTS.md`](AGENTS.md) — zasady dla kontrybutorów i agentów AI.

## Licencja

GPL-3.0-only (`LICENSE`). Kod pochodny musi pozostać na GPL-3.0. Zależności są GPL-kompatybilne (egui MIT/Apache-2.0, `age` MIT/Apache-2.0, `rusqlite` MIT).

## Bezpieczeństwo i etyka

Narzędzie działa lokalnie i offline, nie wysyła danych użytkownika. Migracja danych wymaga zgody właściciela komputera. Nie implementujemy obejść zabezpieczeń przeglądarek (App‑Bound Encryption) ani blokad plików — to techniki charakterystyczne dla infostealerów.
