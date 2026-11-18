//! Makiety dostawców danych dla testów modułów (feature `testing`).
//!
//! Dzięki nim moduł testuje się bez WMI, bez sprzętu i bez uprawnień
//! administratora. Makieta zwraca dokładnie to, co poda test.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{ModuleError, Result};
use crate::i18n::Locale;
use crate::module::CollectCtx;
use crate::providers::{Clock, HostEnv, WmiProvider};

/// WMI zwracające przygotowane wiersze. Fragment zapytania dopasowuje się do
/// treści zapytania (np. `"Win32_OperatingSystem"`).
#[derive(Debug, Default)]
pub struct MockWmi {
    responses: Vec<(String, Vec<serde_json::Value>)>,
}

impl MockWmi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rejestruje odpowiedź dla zapytań zawierających `query_fragment`.
    pub fn with_rows(
        mut self,
        query_fragment: impl Into<String>,
        rows: Vec<serde_json::Value>,
    ) -> Self {
        self.responses.push((query_fragment.into(), rows));
        self
    }
}

impl WmiProvider for MockWmi {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>> {
        let haystack = format!("{namespace} {wql}");
        self.responses
            .iter()
            .find(|(fragment, _)| haystack.contains(fragment.as_str()))
            .map(|(_, rows)| rows.clone())
            .ok_or_else(|| {
                ModuleError::unavailable(format!("mock WMI: brak odpowiedzi dla `{wql}`"))
            })
    }
}

/// Zegar o stałym czasie — testy nie zależą od daty uruchomienia.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: SystemTime,
    uptime_secs: u64,
}

impl FixedClock {
    /// 1 stycznia 2024, 00:00:00 UTC.
    pub fn new() -> Self {
        Self {
            now: UNIX_EPOCH + Duration::from_secs(1_704_067_200),
            uptime_secs: 3_600,
        }
    }

    pub fn with_uptime(mut self, uptime_secs: u64) -> Self {
        self.uptime_secs = uptime_secs;
        self
    }
}

impl Default for FixedClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.now
    }

    fn uptime_secs(&self) -> u64 {
        self.uptime_secs
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockHost {
    elevated: bool,
}

impl MockHost {
    pub fn elevated() -> Self {
        Self { elevated: true }
    }

    pub fn userland() -> Self {
        Self { elevated: false }
    }
}

impl HostEnv for MockHost {
    fn is_elevated(&self) -> bool {
        self.elevated
    }
}

/// Zestaw makiet + wygodne `ctx()`.
#[derive(Debug)]
pub struct Harness {
    pub wmi: MockWmi,
    pub clock: FixedClock,
    pub host: MockHost,
    pub locale: Locale,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            wmi: MockWmi::new(),
            clock: FixedClock::new(),
            host: MockHost::default(),
            locale: Locale::Pl,
        }
    }

    pub fn with_wmi(mut self, wmi: MockWmi) -> Self {
        self.wmi = wmi;
        self
    }

    pub fn with_clock(mut self, clock: FixedClock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_host(mut self, host: MockHost) -> Self {
        self.host = host;
        self
    }

    pub fn with_locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    pub fn ctx(&self) -> CollectCtx<'_> {
        CollectCtx {
            wmi: &self.wmi,
            clock: &self.clock,
            host: &self.host,
            locale: self.locale,
        }
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_wmi_matches_by_fragment() {
        let wmi =
            MockWmi::new().with_rows("Win32_BIOS", vec![serde_json::json!({"SerialNumber": "X"})]);
        let rows = wmi
            .query("ROOT\\CIMV2", "SELECT * FROM Win32_BIOS")
            .expect("trafienie");
        assert_eq!(rows.len(), 1);
        assert!(wmi.query("ROOT\\CIMV2", "SELECT * FROM Win32_Fan").is_err());
    }

    #[test]
    fn harness_builds_ctx() {
        let harness = Harness::new().with_host(MockHost::elevated());
        let ctx = harness.ctx();
        assert!(ctx.host.is_elevated());
        assert_eq!(ctx.clock.uptime_secs(), 3_600);
    }
}
