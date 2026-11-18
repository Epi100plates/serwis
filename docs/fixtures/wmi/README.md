# Fixtures WMI i EDID

Odpowiedzi WMI używane przez testy modułów. Dzięki nim testy nie zależą od sprzętu,
systemu ani uprawnień administratora.

## Skąd pochodzą

- Pliki `win32_*.json` i `wmi_*.json` to **prawdziwe odpowiedzi z maszyny serwisowej**
  (Windows 11 26300, Ryzen 5 5600, RX 6700 XT, WD SN530 + 3× SATA SSD, 2 monitory),
  zebrane skryptem:

  ```powershell
  powershell -ExecutionPolicy Bypass -File tools\collect-wmi-fixtures.ps1
  ```

- Pliki `*_synthetic.json` są **napisane ręcznie** i oznaczone w nazwie. Dotyczą sprzętu,
  którego nie było pod ręką (bateria laptopa). Nie udają danych z tej maszyny — mają
  realistyczne wartości i prawdziwe nazwy klas/właściwości, żeby module testowały
  obliczenia (zużycie ogniwa, procent naładowania).

## Anonimizacja

Skrypt zastępuje dane identyfikujące: `SerialNumber`, `UUID`, `ProcessorId`,
`Signature`, `SystemName`, `CSName`, `RegisteredUser`, `PrimaryOwnerName`, nazwę
komputera, `DNSHostName`, nazwę użytkownika oraz `SerialNumberID` monitora.
Usuwa też artefakty PowerShella/CIM (`CimClass`, `PSComputerName`, zagnieżdżone
obiekty CIM), których klient WMI w Rust nie zwraca.

Fixture można wrzucić do repo tylko wtedy, gdy `Select-String -Path docs\fixtures\wmi\*.json -Pattern 'DESKTOP-|<imię>|CimClass'`
nic nie znajduje.

## Ograniczenie: kształt danych

PowerShell serializuje `DateTime` jako ISO 8601, a klient `wmi` w Rust zostawia
`CIM_DATETIME` jako łańcuch WMI (`20230516020000.000000+120`). Dlatego moduły parsują
daty **tolerancyjnie** (`util::date_only` obsługuje oba formaty), a poprawność na
prawdziwych danych sprawdzają testy sprzętowe:

```powershell
cargo test -p sk-modules --test hardware -- --ignored
```

## EDID

`docs/fixtures/edid/*.hex` to bloki EDID monitorów odczytane z
`HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\...\Device Parameters\EDID` i zapisane
jako hex (czytelny diff w gicie). Testy `crates/sk-modules/src/edid.rs` sprawdzają
na nich producenta, model, rozmiar, tryb natywny i limity odświeżania.
