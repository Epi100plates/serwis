# Jak dodać nowy moduł do SerwisKit

Dokument dla kontrybutorów i agentów AI. Opisuje **rzeczywisty** kontrakt z `crates/sk-core` (stan: Faza 0, 2026‑11‑18). Jeśli implementacja wymusi zmianę kontraktu — zmieniamy ten dokument i kod w tym samym PR.

Zasada nadrzędna: **moduł to jeden plik i nie dotyka kodu UI.** Jeśli musisz zmienić renderer, żeby dodać moduł, to znaczy, że moduł jest źle zaprojektowany albo brakuje typu bloku w kontrakcie (procedura w §9).

---

## 1. Mapa repozytorium

```
crates/
  sk-core/                 kontrakt: Module, View, Capability, błędy, i18n, rejestr, makiety testowe
    locales/pl/*.json      tłumaczenia (jeden plik na rodzinę modułów)
    locales/en/*.json
  sk-win/                  WMI, zegar, środowisko procesu — JEDYNE miejsce z `unsafe`
  sk-modules/              MODUŁY: jeden plik = jeden moduł
    src/modules/mod.rs     lista modułów i rejestracja (dwie linie na moduł)
    src/modules/system_info.rs
  sk-app/                  binarka `serwiskit`: egui + clap + generyczny renderer widoków
docs/
  PLAN.md                  plan i decyzje architektoniczne
  MODULE_GUIDE.md          ten dokument
AGENTS.md                  zasady dla agentów AI (czytaj pierwsze)
```

Planowane crate'y kolejnych faz: `sk-storage` (benchmarki, surface test), `sk-migrate` (pakiety `.skpkg`), `sk-report` (HTML). Nie twórz ich w zadaniu modułowym.

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
    pub wmi: &'a dyn WmiProvider,  // query(namespace, wql) -> Vec<serde_json::Value>
    pub clock: &'a dyn Clock,      // now(), uptime_secs()
    pub host: &'a dyn HostEnv,     // is_elevated()
    pub locale: Locale,
}
```

Kolejne fazy rozszerzą kontekst (Faza 2: `devices: &dyn RawDevice` do surowych sektorów, Faza 3: `dpapi: &dyn Dpapi`). Rozszerzenie wykonuje lider, nie autor modułu.

Widok (to renderuje `sk-app`, moduł nigdy nie importuje `egui`):

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
    Table { table: Table },                     // nagłówki (klucze i18n) + wiersze
    Note { level: NoteLevel, text_key: String },// Info | Warn | Error — degradacja, brak danych
    Badge { text_key: String },                 // np. "Administrator", "Niedostępne przez USB"
}
```

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
    .with_rows("Win32_Processor", vec![serde_json::json!({ "Name": "AMD Ryzen 5 5600" })]);
let harness = Harness::new().with_wmi(wmi);
```

Dane w makiecie mają odwzorowywać **prawdziwą** odpowiedź WMI (skopiuj wartości z maszyny, usuń seriale/UUID/nazwy użytkownika). Od Fazy 1 fixtures trzymamy w `docs/fixtures/wmi/`.

**Krok 7.** Weryfikacja lokalna:

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -p sk-app                                   # moduł widoczny na liście, dane realne
cargo run -p sk-app -- report --json out.json --module tools.timestamp
```

**Krok 8.** Jeśli moduł wymaga administratora — ustaw `Capability::Elevated` i napisz w `desc`, **co konkretnie** wymaga uprawnień.

**Krok 9.** Jeśli moduł ma długą operację (benchmark, kopiowanie profilu) — **nie rób tego w `collect()`**. Do Fazy 2 `TaskDescriptor` jeszcze nie istnieje; zgłoś potrzebę liderowi, zamiast blokować wątek UI.

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

---

## 6. Testy, których oczekuje review

| Test | Po co | Gdzie |
|---|---|---|
| jednostkowy na makiecie | logika bez sprzętu | w pliku modułu |
| kompletność kluczy i18n | brak „martwych” napisów | w pliku modułu (`Translator::load` + `has`) |
| kontrakt (automatyczny) | unikalne `id`, klucze metadanych, brak paniki | `crates/sk-modules/tests/contract.rs` — nic nie dopisujesz |
| sprzętowy `#[ignore]` | realne WMI na maszynie serwisowej | obok, `cargo test -- --ignored` |

Makieta: `sk_core::testing::{Harness, MockWmi, FixedClock, MockHost}` — dostępna przez `dev-dependencies` z feature `testing`.

---

## 7. Definition of Done — checklista modułu

- [ ] `meta()` kompletne: unikalne `id`, klucze `name`/`desc`, właściwy `group`, uczciwe `capability`.
- [ ] `collect()` nie panikuje, nie blokuje > 200 ms, nie robi sieci.
- [ ] Braki sprzętowe/uprawnieniowe → `Block::Note`, nie błąd całego modułu.
- [ ] Klucze i18n w `pl` **i** `en`.
- [ ] Test jednostkowy na makiecie (+ test kompletności kluczy widoku).
- [ ] Test sprzętowy `#[ignore]` wykonany ręcznie (jeśli dotyczy sprzętu) — wynik w opisie PR.
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` zielone.
- [ ] Zero `unsafe`, zero `unwrap()` na danych zewnętrznych, zero sekretów w logach.
- [ ] Dwie linie w `modules/mod.rs` w kolejności alfabetycznej.

---

## 8. Anty‑wzorce (odrzucane w review)

| Anty‑wzorzec | Dlaczego |
|---|---|
| Gotowe stringi w `Block` zamiast kluczy i18n | brak EN, brak testu kompletności |
| `unwrap()` na wyniku WMI | panika na nietypowym sprzęcie klienta |
| Wołanie `egui` z modułu | moduł przestaje być testowalny bez okna |
| Jeden moduł „wszystko o sprzęcie” | nie da się przypisać właściciela, konflikt przy pracy równoległej |
| Ciche pomijanie błędów (`let _ = ...`) | technik dostaje niepełny raport i nie wie dlaczego |
| Nowe `unsafe` w module | jedna brama FFI, łatwiejszy audyt |
| `sleep`/retry w `collect()` | blokuje UI; od tego będą zadania w tle |
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
