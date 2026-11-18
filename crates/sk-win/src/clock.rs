use std::time::SystemTime;

use sk_core::providers::Clock;

/// Zegar systemowy. Czas działania bierzemy z `GetTickCount64` (milisekundy od
/// startu systemu) — działa bez uprawnień administratora i nie zależy od WMI.
#[derive(Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn uptime_secs(&self) -> u64 {
        // SAFETY: GetTickCount64 nie przyjmuje argumentów i nie może zawieść;
        // zwraca liczbę milisekund od uruchomienia systemu.
        unsafe { windows::Win32::System::SystemInformation::GetTickCount64() / 1_000 }
    }
}
