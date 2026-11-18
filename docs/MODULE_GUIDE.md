# Jak dodać nowy moduł do SerwisKit

Dokument dla kontrybutorów i agentów AI. Opisuje **rzeczywisty** kontrakt z `crates/sk-core` (stan: Faza 2 w toku, 2026‑11‑18 — SMART dysków gotowy). Jeśli implementacja wymusi zmianę kontraktu — zmieniamy ten dokument i kod w tym samym PR.

Zasada nadrzędna: **moduł to jeden plik i nie dotyka kodu UI.** Jeśli musisz zmienić renderer, żeby dodać moduł, to znaczy, że moduł jest źle zaprojektowany albo brakuje typu bloku w kontrakcie (procedura w §9).

---

## 1. Mapa repozytorium

```
crates/
  sk-core/                 kontrakt: Module, View, Capability, błędy, i18n, rejestr, telemetria, makiety
    locales/pl/*.json      tłumaczenia (jeden plik na rodzinę modułów)
    locales/en/*.json
  sk-win/                  WMI + cache, zegar, środowisko procesu, rejestr, EDID, dyski/SMART — JEDYNE miejsce z `unsafe`
  sk-modules/              MODUŁY: jeden plik = jeden moduł
    src/edid.rs           parser EDID (czysty Rust, testy na fixtures)
    src/smart.rs          parsery SMART NVMe i ATA oraz ocena stanu (czysty Rust)
    src/util.rs           wspólne dekodowanie WMI i formatowanie wartości
    src/modules/mod.rs     lista modułów i rejestracja (dwie linie na moduł)
    src/modules/hw_*.rs    moduły sprzętowe
    src/modules/storage_*.rs moduły dyskowe (SMART, w kolejce benchmark i surface)
    tests/contract.rs      automatyczne testy każdego modułu
    tests/golden.rs        wzorce kształtu widoków (`tests/golden/*.json`)
    tests/hardware.rs      testy sprzętowe `#[ignore]` (prawdziwe WMI, dyski)
  sk-app/                  binarka `serwiskit`: egui + clap + renderer widoków + raport JSON/HTML
    src/html.rs            raport HTML z modelu `View` (inline SVG dla wykresów)
    src/live.rs            próbkowanie telemetrii dla `system.live`
    src/report.rs          jedno zbieranie → JSON i HTML
docs/
  PLAN.md                  plan i decyzje architektoniczne
  MODULE_GUIDE.md          ten dokument
  fixtures/wmi/*.json      prawdziwe odpowiedzi WMI (po anonimizacji) do testów
  fixtures/edid/*.hex      bloki EDID monitorów testowych
tools/
  collect-wmi-fixtures.ps1 zbieranie fixtures z bieżącej maszyny
AGENTS.md                  zasady dla agentów AI (czytaj pierwsze)
```

Planowane crate'y kolejnych faz: `sk-storage` (benchmarki, surface test), `sk-migrate` (pakiety `.skpkg`). Raport HTML **nie** jest osobnym crate'em — powstaje w `sk-app` z tego samego modelu widoku co GUI. Nie twórz nowych crate'ów w zadaniu modułowym.

**Zakresy zapisu (write scope) przy pracy równoległej botów:**

| Plik | Kto pisze | Uwaga |
|---|---|---|
| `crates/sk-modules/src/modules/<twoj_modul>.rs` | **tylko Ty** | nowy plik, zero konfliktów |
| `crates/sk-modules/src/modules/mod.rs` | Ty (2 linie) | jedyne miejsce konfliktu — dopisuj alfabetycznie, przy rebase zachowaj obie pary linii |
| `crates/sk-core/locales/{pl,en}/<rodzina>.json` | Ty (2 pliki) | jeśli rodzina istnieje — dopisz klucze; jeśli nie — nowy plik |
| `crates/sk-core/src/**`, `crates/sk-app/src/**`, `crates/sk-win/src/**` | nikt bez zgody lidera | zmiana kontraktu = decyzja architektoniczna |
| `Cargo.toml`, `docs/PLAN.md` | lider | nie edytuj w zadaniu modułowym |

---

## 2. Kontrakt modułu (stan faktyczny)

```rust
// crates/sk-core/src/module.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Userland,     // wystarczą uprawnienia zwykłego użytkownika
    Elevated,     // wymaga administratora
    Destructive,  // administrator + jawne potwierdzenie (operacja niszczy dane)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group { Hardware, Storage, Migration, System, Tools }

#[derive(Debug, Clone, Copy)]
pub struct ModuleMeta {
    pub id: &'static str,          // "hw.cpu" — unikalne, kropkowane, stabilne (klucz w raportach)
    pub name_key: &'static str,    // "module.hw_cpu.name" — klucz i18n, nie gotowy tekst
    pub desc_key: &'static str,
    pub group: Group,
    pub capability: Capability,
    pub experimental: bool,        // true => UI pokazuje ostrzeżenie
}

pub struct ModuleOutput {
    pub data: serde_json::Value,   // surowe dane do raportu JSON/HTML (stabilny schemat)
    pub view: View,                // deklaratywny widok do wyrenderowania
}

pub trait Module: Send + Sync + 'static {
    fn meta(&self) -> ModuleMeta;
    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput, ModuleError>;
}
```

Kontekst zbierania — wszystko, co łączy moduł ze światem, jest wstrzykiwane, więc moduł testuje się bez sprzętu:

```rust
pub struct CollectCtx<'a> {
    pub wmi: &'a dyn WmiProvider,      // query(namespace, wql) -> Vec<serde_json::Value>
    pub clock: &'a dyn Clock,          // now(), uptime_secs()
    pub host: &'a dyn HostEnv,         // is_elevated()
    pub machine: &'a dyn MachineInfo,  // firmware(), adapter_memory(), display_edid()
    pub storage: &'a dyn StorageProbe, // describe(index), smart(index) — dyski fizyczne (Faza 2)
    pub live: &'a dyn LiveSource,      // samples() — historia telemetrii (Faza 1)
    pub locale: Locale,
}
```

`MachineInfo` istnieje dlatego, że WMI **nie** daje trzech rzeczy: trybu firmware (`GetFirmwareType`), realnego VRAM (`AdapterRAM` jest 32‑bitowe i obcina do 4 GiB) i EDID monitora (tylko rejestr). Wszystkie metody mają domyślne implementacje, więc makieta nie musi ich znać — brak danych jest poprawnym stanem.

`StorageProbe` (Faza 2) daje to, czego WMI nie daje o dyskach: magistralę i rodzaj nośnika z deskryptora urządzenia oraz **surowe** bloki SMART. Implementacja realna (`sk-win/src/storage.rs`) wymaga administratora; makieta (`MockStorage`) podstawia gotowe bajty.

Kolejne fazy rozszerzą kontekst (Faza 3: `dpapi: &dyn Dpapi` do haseł przeglądarek). Rozszerzenie wykonuje lider, nie autor modułu.

Widok (to renderuje `sk-app`; ten sam model trafia do raportu HTML, moduł nigdy nie importuje `egui`):

```rust
pub struct View { pub sections: Vec<Section> }

pub struct Section {
    pub title_key: String,
    pub subtitle: Option<String>,
    pub blocks: Vec<Block>,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    KeyValues { rows: Vec<KeyValue> },          // etykieta (klucz i18n) + wartość (dana)
    Table { table: Table },                     // nagłówki (klucze i18n) + wiersze (dane)
    Note { level: NoteLevel, text_key: String },// Info | Warn | Error — degradacja, brak danych
    Badge { text_key: String },                 // np. "Administrator", "Niedostępne przez USB"
    Status { label_key, value_key, level },     // duży stan sekcji: „Stan dysku: Dobry”
    Chart { chart: Chart },                     // wykres liniowy (Faza 1: system.live)
}

pub struct KeyValue {
    pub label_key: String,
    pub value: String,                 // dana
    pub value_key: Option<String>,     // alternatywa: klucz i18n (np. Tak/Nie)
}

pub struct Table {
    pub headers: Vec<String>,          // klucze i18n
    pub rows: Vec<Vec<String>>,        // dane
    pub value_keys: Vec<Vec<Option<String>>>,    // klucze i18n dla pojedynczych komórek
    pub cell_levels: Vec<Vec<Option<StatusLevel>>>, // kolory pojedynczych komórek
}

pub struct Chart {
    pub title_key: String,             // etykieta wykresu = klucz i18n
    pub x_label: String,               // jednostka osi, np. "s"
    pub y_label: String,               // jednostka osi, np. "%"
    pub series: Vec<ChartSeries>,      // label = dana ("CPU"), points = [x, y]
}
```

Reguła rozróżniania tekstu i danych: **kluczem i18n jest wszystko, co jest zdaniem lub etykietą** (nazwy sekcji, nagłówki tabel, etykiety wierszy, komunikaty, wartości logiczne). **Daną jest to, co przyszło z WMI lub z obliczeń** (nazwa procesora, numer seryjny, „3200 MHz”, „2010‑W46”). Nie wpisuj do wartości słów typu „tydzień” — wybierz format neutralny językowo (ISO 8601) albo użyj `value_key`.

Dla stanów w **tabeli** użyj `Table::with_value_key(row, column, "common.value.digital")` — komórka jest wtedy tłumaczona tak samo jak `KeyValue::state`. Przykład: kolumna „Wejście” w `hw.monitor` (cyfrowe/analogowe) i kolumna stanu w `hw.disk_inventory`.

Dla **wyróżnienia kolorem** użyj `Table::with_cell_level(row, column, StatusLevel::Warn)` (`Ok | Warn | Bad | Unknown`) — niezależnie od `value_key`, bo dana może być jednocześnie liczbą i ostrzeżeniem („86 %”). Duży nagłówek stanu to `Block::status("…label", "…wartość", StatusLevel::Ok)` — renderuje się jako kolorowy baner w GUI i wyróżniony akapit w HTML. Kolory są wspólne dla obu rendererów, żeby „Dobry” znaczyło to samo w aplikacji i w raporcie.

Zapytania o klasy używane w więcej niż jednym module trzymamy w `sk_core::queries` (np. `WIN32_VIDEO_CONTROLLER`, `PROCESSOR_PERFORMANCE`). Cache WMI jest kluczowany po `(przestrzeń nazw, zapytanie)`, więc dwie różne listy pól tej samej klasy to dwa zapytania w rundzie — jedna stała = jedno zapytanie.

Rejestracja: **jawna lista** w `crates/sk-modules/src/modules/mod.rs`:

```rust
mod system_info;                                    // 1. drzewo modułów
registry.add::<system_info::SystemInfo>();          // 2. rejestracja w funkcji `register`
```

Świadomie nie używamy `inventory`/auto-rejestracji przez linker: na Windows z MSVC statyki z nieodwoływanych crate'ów bywają odrzucane i aplikacja pokazuje pustą listę modułów bez błędu kompilacji. Jawna lista jest deterministyczna i widoczna w jednym miejscu. Koszt: dwie linie na moduł.

---

## 3. Recepta: nowy moduł w 10 krokach

Przykład: `tools.timestamp` (czas lokalny i uptime — prosty, ale pełny przepływ).

**Krok 1.** Utwórz plik `crates/sk-modules/src/modules/tools_timestamp.rs`.

**Krok 2.** Zaimplementuj moduł:

```rust
use chrono::{DateTime, Local};
use sk_core::prelude::*;

#[derive(Debug, Default)]
pub struct Timestamp;

impl Module for Timestamp {
    fn meta(&self) -> ModuleMeta {
        ModuleMeta {
            id: "tools.timestamp",
            name_key: "module.tools_timestamp.name",
            desc_key: "module.tools_timestamp.desc",
            group: Group::Tools,
            capability: Capability::Userland,
            experimental: false,
        }
    }

    fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
        let local_time =
            DateTime::<Local>::from(ctx.clock.now()).format("%Y-%m-%d %H:%M:%S").to_string();
        let uptime_secs = ctx.clock.uptime_secs();

        let view = View::new(vec![
            Section::new("module.tools_timestamp.section").with_blocks(vec![
                Block::key_values(vec![
                    KeyValue::new("module.tools_timestamp.now", local_time.clone()),
                    KeyValue::new(
                        "module.tools_timestamp.uptime",
                        format!("{} min", uptime_secs / 60),
                    ),
                ]),
            ]),
        ]);

        let data = serde_json::json!({
            "schema_version": 1,
            "local_time": local_time,
            "uptime_secs": uptime_secs,
        });

        Ok(ModuleOutput { data, view })
    }
}
```

**Krok 3.** Dopisz dwie linie w `crates/sk-modules/src/modules/mod.rs` (alfabetycznie):

```rust
mod tools_timestamp;
// ... w funkcji register:
registry.add::<tools_timestamp::Timestamp>();
```

**Krok 4.** Dodaj klucze i18n. Nowa rodzina → nowe pliki `crates/sk-core/locales/pl/tools.json` i `crates/sk-core/locales/en/tools.json`:

```json
{
  "module.tools_timestamp.name": "Znacznik czasu",
  "module.tools_timestamp.desc": "Aktualny czas lokalny i czas działania systemu.",
  "module.tools_timestamp.section": "Czas",
  "module.tools_timestamp.now": "Teraz",
  "module.tools_timestamp.uptime": "Czas działania"
}
```

**Krok 5.** Test jednostkowy w tym samym pliku:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sk_core::testing::Harness;

    #[test]
    fn exposes_time_rows() {
        let harness = Harness::new();
        let output = Timestamp.collect(&harness.ctx()).expect("zbieranie danych");
        assert_eq!(output.view.sections.len(), 1);
        assert!(output.view.uses_key("module.tools_timestamp.now"));
        assert!(output.data["uptime_secs"].is_u64());
    }
}
```

**Krok 6.** Jeśli moduł czyta WMI: zarejestruj odpowiedzi w makiecie i sprawdź mapowanie pól:

```rust
use sk_core::testing::{Harness, MockWmi};

let wmi = MockWmi::new()
    .with_rows("Win32_Processor", fixture_rows!("win32_processor.json"));
let harness = Harness::new().with_wmi(wmi);
```

`fixture_rows!` to makro z `sk-modules/src/lib.rs` — wczytuje `docs/fixtures/wmi/<plik>` przez `include_str!` (ścieżka względem `CARGO_MANIFEST_DIR`, więc działa z każdego pliku modułu).

Fixtures mają odwzorowywać **prawdziwą** odpowiedź WMI: zbierasz je skryptem `tools/collect-wmi-fixtures.ps1`, który sam usuwa artefakty PowerShell/CIM i anonimizuje seriale, UUID, nazwę komputera i właściciela. Gdy klasa na Twojej maszynie nie istnieje (np. `Win32_Battery` na desktopie), dopisz **syntetyczny** fixture z sufiksem `_synthetic.json` i opisz go w `docs/fixtures/wmi/README.md` — nie udawaj, że pochodzi z tej maszyny.

**Krok 7.** Weryfikacja lokalna:

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo test -p sk-modules --test hardware -- --ignored   # prawdziwe WMI tej maszyny
cargo run -p sk-app                                     # moduł widoczny na liście, dane realne
cargo run -p sk-app -- report --json out.json --html out.html --module tools.timestamp
```

**Krok 8.** Jeśli moduł wymaga administratora — ustaw `Capability::Elevated` i napisz w `desc`, **co konkretnie** wymaga uprawnień.

**Krok 9.** Jeśli moduł ma długą operację (benchmark, kopiowanie profilu) — **nie rób tego w `collect()`**. Do Fazy 2 `TaskDescriptor` jeszcze nie istnieje; zgłoś potrzebę liderowi, zamiast blokować wątek UI. Wzorzec dla danych „w czasie” już istnieje: moduł `system.live` czyta `ctx.live.samples()`, a próbki zbiera wątek roboczy aplikacji (raz na sekundę, z throttle w UI).

**Krok 10.** Uzupełnij checklistę z §7 i zgłoś PR z szablonem z §11.

---

## 4. Zasady twarde (egzekwowane w review i CI)

1. **Zero `unsafe` poza `sk-win`.** Potrzebujesz nowego wywołania systemowego — zgłoś dodanie go do `sk-win`; nie dopisuj FFI u siebie.
2. **Zero sieci.** Żadnych klientów HTTP, telemetrii, „sprawdzania aktualizacji” z modułu.
3. **Zero `unwrap()`/`expect()` na danych zewnętrznych.** `ModuleError` z kontekstem: co, skąd, jaki błąd. Panika w module blokuje PR (test kontraktu wywołuje `collect()` na pustym dostawcy i sprawdza, że nie panikuje).
4. **Brak blokowania UI.** Moduł nie śpi i nie robi długiego I/O w `collect()`; powyżej ~200 ms zgłoś potrzebę zadania w tle.
5. **Wszystkie teksty przez klucze i18n** w `pl` i `en`. Testy sprawdzają komplet kluczy metadanych i widoku; brak klucza = czerwone CI.
6. **`id` jest wieczne.** Zmiana `id` to zmiana publicznego kontraktu raportów — wymaga decyzji lidera i migracji.
7. **Dane osobowe minimalnie.** Nie loguj ścieżek użytkownika ani sekretów; seriale/UUID tylko w raporcie.
8. **Degradacja zamiast wyjątku.** Brak baterii, SMART przez USB, brak admina → `Block::note(NoteLevel::Warn, "...")`, nie `Err` wywalający cały raport.
9. **Jedna odpowiedzialność.** `hw.cpu` nie zbiera danych o dyskach. Duży obszar = kilka modułów.
10. **Zmiana kontraktu** (`sk-core`) = jeden PR, jeden właściciel, aktualizacja tego dokumentu.

---

## 5. Długie operacje — stan obecny

`TaskDescriptor` (postęp, anulowanie, parametry) jest zaprojektowany, ale **niezaimplementowany** — dochodzi w Fazie 2 razem z benchmarkiem dysku. Do tego czasu:

- moduł nie wykonuje operacji > ~200 ms,
- jeśli funkcja wymaga dłuższego I/O, to jest to nowy moduł zgłoszony liderowi, a nie „obejście” w `collect()`.

Docelowy kształt (dla planowania):

```rust
pub struct TaskDescriptor {
    pub id: &'static str,
    pub label_key: &'static str,
    pub capability: Capability,        // Destructive => UI wymaga potwierdzenia
    pub params: &'static [ParamSpec],  // formularz generowany przez renderer
    pub run: fn(&TaskCtx<'_>) -> Result<ModuleOutput>,
}
```

### 5.1 Wykresy i dane w czasie

Jeśli moduł pokazuje coś, co zmienia się w czasie, **nie próbkuj w `collect()`**. Wzorzec z Fazy 1 (`system.live`):

```rust
fn collect(&self, ctx: &CollectCtx<'_>) -> Result<ModuleOutput> {
    let samples = ctx.live.samples();               // historia z wątku roboczego
    if samples.is_empty() {
        return Ok(ModuleOutput { data: json!({}), view: /* Note: brak próbek */ });
    }
    let points: Vec<[f64; 2]> = samples
        .iter()
        .filter_map(|s| s.cpu_load_percent.map(|v| [s.elapsed_secs, v]))
        .collect();
    let chart = Chart::new("module.x.chart", "s", "%")
        .with_series(vec![ChartSeries::new("CPU", points)]);
    // Block::chart(chart) renderuje egui_plot w GUI, a raport HTML — inline SVG
}
```

Próbki zbiera wątek roboczy aplikacji (`sk-app/src/live.rs`) i tylko wtedy, gdy moduł jest otwarty w GUI. Dzięki temu `collect()` jest natychmiastowe, a moduł pozostaje bezstanowy i testowalny przez `MockLive`.

### 5.2 Surowe dane urządzeń — SMART

Zasada: **transport w `sk-win`, parsowanie w module**. `StorageProbe` zwraca bajty dokładnie w takiej postaci, w jakiej daje je system:

```rust
ctx.storage.describe(4)?          // PhysicalDrive { index, bus, kind, removable } — z deskryptora
ctx.storage.smart(4)?             // SmartData::Nvme { log_page } | SmartData::Ata { attributes, thresholds }
```

Parsery współdzielone leżą w `crates/sk-modules/src/smart.rs` (`parse_nvme_health`, `parse_ata_attributes`, `nvme_health`, `ata_health`) — nie kopiuj ich do modułu. Są czystymi funkcjami, więc każdy przypadek (zużycie 100 %, sektory oczekujące, przekroczony próg) testuje się na zbudowanym w teście buforze, bez dysku i bez administratora.

Reguły, których oczekuje review:

- **Brak uprawnień to nie błąd modułu.** `AccessDenied` → `Block::note(NoteLevel::Warn, "…note.elevation")` plus stan „nieznane” w tabeli; `collect()` zwraca `Ok`.
- **Awaria jednego dysku nie zabiera raportu.** Błąd `smart()` zapisujesz przy konkretnym dysku (dane JSON `error.kind`), a nie jako `Err` całego modułu.
- **Nie interpretuj surowych liczników bez wiedzy.** Znaczenie mają atrybuty z `SUSPICIOUS_ATTRIBUTE_IDS` (5, 10, 184, 187, 188, 196, 197, 198, 199); atrybuty 1 i 7 mają surowe wartości niezerowe także na zdrowym dysku.
- **Ocena stanu jest jawna** (`Health::Good/Warning/Bad`) i pokazana jako `value_key` (`module.storage_smart.health.*`) plus kolor (`StatusLevel`), nie jako kolor bez wyjaśnienia.
- **Układ sekcji dysku jest stały** (jak w CrystalDiskInfo): `Block::status` ze stanem → notki (ostrzeżenia NVMe, przekroczony próg, brak uprawnień) → wskaźniki (`nvme_rows` / `ata_rows`) → tożsamość nośnika → pełna tabela atrybutów ATA z kolumną „Ocena”. Wskaźniki ATA liczy `ata_rows` z atrybutów (5, 9, 12, 177/231/233, 194, 197, 198, 199, 241, 242), więc technik widzi je bez przewijania 30 wierszy tabeli.
- **Ocena pojedynczego atrybutu to ta sama reguła, co ocena dysku** (`smart::attribute_state`): przekroczony próg → `Bad`, niezerowy licznik z `SUSPICIOUS_ATTRIBUTE_IDS` → `Warning`, inaczej `Good`. Nie duplikuj tej logiki w module — inaczej kolumna „Ocena” rozjedzie się z banerem.
- **Liczniki ATA w LBA przeliczaj przez `BytesPerSector`** z `Win32_DiskDrive` (atrybuty 241/242), a nie przez stałe 512 B — dysk 4Kn inaczej pokaże czterokrotnie zaniżone dane.

---

## 6. Testy, których oczekuje review

| Test | Po co | Gdzie |
|---|---|---|
| jednostkowy na makiecie | logika bez sprzętu | w pliku modułu |
| kompletność kluczy i18n | brak „martwych” napisów | w pliku modułu (`Translator::load` + `has`) |
| kontrakt (automatyczny) | unikalne `id`, klucze metadanych, brak paniki | `crates/sk-modules/tests/contract.rs` — nic nie dopisujesz |
| golden widoku | stabilny kształt raportu (JSON) | `crates/sk-modules/tests/golden.rs` — dopisz swój moduł, jeśli jego widok jest kontraktem |
| sprzętowy `#[ignore]` | realne WMI na maszynie serwisowej | `crates/sk-modules/tests/hardware.rs` — dopisz swój `id` do listy modułów, jeśli chcesz go tam objąć |

Wzorce golden odświeżasz świadomie: `$env:UPDATE_GOLDEN = "1"; cargo test -p sk-modules --test golden`, potem commit z opisem, dlaczego widok się zmienił.

Makieta: `sk_core::testing::{Harness, MockWmi, MockMachine, MockLive, MockStorage, FixedClock, MockHost}` — dostępna przez `dev-dependencies` z feature `testing`. `MockWmi` dopasowuje odpowiedź po **najdłuższym** pasującym fragmencie zapytania, więc `Win32_PhysicalMemoryArray` nie zostanie pomylone z `Win32_PhysicalMemory`. `MockStorage` ma `with_nvme`, `with_ata`, `with_denied` (brak administratora) i `with_unavailable` (mostek USB/kontroler RAID).

---

## 7. Definition of Done — checklista modułu

- [ ] `meta()` kompletne: unikalne `id`, klucze `name`/`desc`, właściwy `group`, uczciwe `capability`.
- [ ] `collect()` nie panikuje, nie blokuje > 200 ms, nie robi sieci.
- [ ] Braki sprzętowe/uprawnieniowe → `Block::Note`, nie błąd całego modułu.
- [ ] Klucze i18n w `pl` **i** `en`; żadnej prozy w wartościach (`value_key` albo format neutralny).
- [ ] Test jednostkowy na makiecie (+ test kompletności kluczy widoku).
- [ ] Fixtures w `docs/fixtures/` (prawdziwe z maszyny albo jawnie `_synthetic`).
- [ ] Test sprzętowy `#[ignore]` wykonany ręcznie (`cargo test -p sk-modules --test hardware -- --ignored`) — wynik w opisie PR.
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` zielone.
- [ ] Zero `unsafe`, zero `unwrap()` na danych zewnętrznych, zero sekretów w logach.
- [ ] Dwie linie w `modules/mod.rs` w kolejności alfabetycznej.
- [ ] Nowe klucze dodane do `pl` i `en` w tym samym PR (test `poland_and_english_have_identical_key_sets` tego pilnuje).

---

## 8. Anty‑wzorce (odrzucane w review)

| Anty‑wzorzec | Dlaczego |
|---|---|
| Gotowe stringi w `Block` zamiast kluczy i18n | brak EN, brak testu kompletności |
| Proza w wartości komórki tabeli („2010, tydzień 46”) | nie da się przetłumaczyć; używaj formatu neutralnego (`2010‑W46`) albo `value_key` |
| `unwrap()` na wyniku WMI | panika na nietypowym sprzęcie klienta |
| Wołanie `egui` z modułu | moduł przestaje być testowalny bez okna |
| Jeden moduł „wszystko o sprzęcie” | nie da się przypisać właściciela, konflikt przy pracy równoległej |
| Ciche pomijanie błędów (`let _ = ...`) | technik dostaje niepełny raport i nie wie dlaczego |
| Nowe `unsafe` w module | jedna brama FFI, łatwiejszy audyt |
| `sleep`/retry w `collect()` | blokuje UI; od tego będą zadania w tle |
| Syntetyczny fixture udający prawdziwy | wprowadza w błąd przy analizie regresji — oznacz `_synthetic` |
| Kopiowanie danych z sieci w module | narusza zasadę offline |

---

## 9. Gdy naprawdę potrzebny jest nowy typ bloku widoku

1. Sprawdź, czy nie wystarczy `KeyValues` + `Note` + `Table` (w większości przypadków wystarcza).
2. Napisz w PR: jaki moduł, dlaczego istniejące bloki nie wystarczają, jak wygląda render.
3. Zmiana w `sk-core` (`Block`) + renderer w `sk-app` + test + aktualizacja tego dokumentu — **jeden PR, jeden właściciel** (lider).
4. Nowy `Block` musi serializować się do JSON (raporty) i mieć sensowny odpowiednik w HTML (Faza 1).

---

## 10. Praca równoległa wielu botów — kolejność scalania

1. Każdy bot bierze **jeden moduł** = jeden nowy plik. Nie dotyka plików innych modułów.
2. `modules/mod.rs` i pliki locale to jedyne wspólne pliki: dopisuj **alfabetycznie**, przy konflikcie zachowaj obie strony.
3. Bot nie zmienia `Cargo.toml`, `sk-core`, `sk-app` ani `sk-win` — jeśli uważa, że musi, zgłasza to liderowi.
4. Kolejność scalania: najpierw kontrakt (`sk-core`), potem moduły; moduł wymagający nowego bloku widoku czeka na merge kontraktu.
5. Po scaleniu lider uruchamia pełne bramki (`fmt`, `clippy`, `test`, `build`) — warunkiem ukończenia jest zielone CI po merge, nie na branchu bota.

---

## 11. Zgłaszanie nowego modułu w PR

```markdown
## Moduł: hw.cpu
- id: hw.cpu
- grupa: Hardware
- capability: Userland
- źródła danych: Win32_Processor, root\wmi ProcessorPerformance
- nowe unsafe w sk-win: nie
- nowe klucze i18n: pl/en (7 kluczy)
- testy: 4 unit (mock), 1 test kluczy, 1 #[ignore] (wykonany na Ryzen 5 5600)
- czego nie pokrywa: temperatura rdzeni (wymaga drivera, Faza 5)
- ryzyka: CurrentClockSpeed ze SMBIOS jest nominalne, nie live
```
