# Moduły PawnIO wkompilowane w `sk-win`

Pliki `.bin` to **podpisane** moduły interpretera Pawn ze sterownika
[PawnIO](https://pawnio.eu/) (autor: namazso). Pochodzą z wydania
`namazso/PawnIO.Modules` **0.2.12** (`release_0_2_12.zip`) i są wgrywane do
sterownika przez `PawnIOLib.dll` w `sk-win/src/pawnio.rs`.

| Plik | Funkcje, których używa `sk-win` |
|---|---|
| `AMDFamily17.bin` | `ioctl_read_smn`, `ioctl_read_msr` — temperatura Tctl/CCD, SVI2, moc pakietu AMD |
| `LpcIO.bin` | `ioctl_select_slot`, `ioctl_find_bars`, `ioctl_pio_inb/outb`, `ioctl_superio_inb/inw/outb` — Super I/O (Nuvoton NCT67xx) |
| `IntelMSR.bin` | `ioctl_read_msr` — temperatura pakietu i napięcie Intel |

Dlaczego wkompilowane: instalator PawnIO nie zawiera modułów, a podpisany
sterownik wykonuje wyłącznie moduły podpisane przez jego autora. Własny moduł
musiałby przejść przegląd i podpisanie u autora, więc korzystamy z gotowych.

## Licencja

Moduły i `COPYING` pochodzą z `PawnIO.Modules` i są objęte **GNU LGPL 2.1**
(plik `COPYING` w tym katalogu). Projekt SerwisKit jest GPL‑3.0‑only, więc
LGPL‑2.1 jest kompatybilna; moduły pozostają osobnymi plikami danych, nie są
linkowane z kodem Rusta.

Aktualizacja modułów: pobierz nowe wydanie `PawnIO.Modules`, podmień pliki
`.bin` i `COPYING`, uruchom `cargo test -p sk-win --lib -- --ignored hardware`
na maszynie z PawnIO i zaktualizuj `AGENTS.md` (sekcja o Fazie 5).
