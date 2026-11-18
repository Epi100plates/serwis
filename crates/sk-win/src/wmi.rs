use std::cell::RefCell;
use std::collections::HashMap;

use sk_core::error::{ModuleError, Result};
use sk_core::providers::WmiProvider;

/// Statystyki cache z bieżącej rundy zbierania.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    /// Zapytania obsłużone z cache (bez odpytywania WMI).
    pub hits: u64,
    /// Zapytania wykonane naprawdę.
    pub misses: u64,
}

impl CacheStats {
    pub fn total(&self) -> u64 {
        self.hits + self.misses
    }
}

/// Cache odpowiedzi WMI w obrębie jednej rundy zbierania.
///
/// Kilka modułów pyta o tę samą klasę (np. `Win32_Processor` w `hw.cpu`
/// i `system.live`). Bez cache to kilka identycznych zapytań WMI na odświeżenie,
/// a WMI jest wolne. `clear()` rozpoczyna nową rundę — dane nie mogą się
/// zestarzeć między odświeżeniami.
#[derive(Debug, Default)]
pub struct QueryCache {
    entries: HashMap<(String, String), Vec<serde_json::Value>>,
    stats: CacheStats,
}

impl QueryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Zwraca wiersze z cache (i liczy trafienie) albo `None` (i liczy pudło).
    pub fn lookup(&mut self, namespace: &str, wql: &str) -> Option<Vec<serde_json::Value>> {
        match self.entries.get(&(namespace.to_string(), wql.to_string())) {
            Some(rows) => {
                self.stats.hits += 1;
                Some(rows.clone())
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    pub fn store(&mut self, namespace: &str, wql: &str, rows: Vec<serde_json::Value>) {
        self.entries
            .insert((namespace.to_string(), wql.to_string()), rows);
    }

    /// Nowa runda: czyści wpisy i statystyki.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.stats = CacheStats::default();
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Dostawca WMI dla Windows.
///
/// `WMIConnection` nie jest `Send` (każdy wątek musi zainicjalizować COM
/// niezależnie), dlatego połączenia trzymamy w `thread_local`. W praktyce
/// wszystkie zapytania idą z jednego, długo żyjącego wątku roboczego aplikacji.
/// Cache również jest per-wątek — dzięki temu nie trzeba synchronizacji.
#[derive(Debug, Default)]
pub struct WinWmi;

thread_local! {
    static CONNECTIONS: RefCell<HashMap<String, wmi::WMIConnection>> =
        RefCell::new(HashMap::new());
    static CACHE: RefCell<QueryCache> = RefCell::new(QueryCache::new());
}

impl WinWmi {
    pub fn new() -> Self {
        Self
    }

    /// Rozpoczyna nową rundę zbierania: czyści cache zapytań.
    pub fn begin_round(&self) {
        CACHE.with(|cache| cache.borrow_mut().clear());
    }

    /// Statystyki cache z bieżącej rundy — używane w raporcie i testach.
    pub fn cache_stats(&self) -> CacheStats {
        CACHE.with(|cache| cache.borrow().stats())
    }
}

impl WmiProvider for WinWmi {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>> {
        if let Some(rows) = CACHE.with(|cache| cache.borrow_mut().lookup(namespace, wql)) {
            return Ok(rows);
        }

        let rows = CONNECTIONS.with(|cell| {
            let mut connections = cell.borrow_mut();

            if !connections.contains_key(namespace) {
                let connection = wmi::WMIConnection::with_namespace_path(namespace)
                    .map_err(|err| ModuleError::provider("WMI", format!("{namespace}: {err}")))?;
                connections.insert(namespace.to_string(), connection);
            }

            let connection = connections.get(namespace).ok_or_else(|| {
                ModuleError::internal(format!("brak połączenia WMI dla przestrzeni {namespace}"))
            })?;

            let rows: Vec<HashMap<String, wmi::Variant>> = connection
                .raw_query(wql)
                .map_err(|err| ModuleError::provider("WMI", format!("{wql}: {err}")))?;

            rows.into_iter()
                .map(|row| {
                    serde_json::to_value(row)
                        .map_err(|err| ModuleError::provider("WMI", format!("{wql}: {err}")))
                })
                .collect::<Result<Vec<_>>>()
        })?;

        CACHE.with(|cache| cache.borrow_mut().store(namespace, wql, rows.clone()));
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(value: u32) -> Vec<serde_json::Value> {
        vec![serde_json::json!({ "Value": value })]
    }

    #[test]
    fn first_query_is_a_miss_and_second_is_a_hit() {
        let mut cache = QueryCache::new();
        assert!(cache.lookup("ROOT\\CIMV2", "SELECT * FROM X").is_none());
        cache.store("ROOT\\CIMV2", "SELECT * FROM X", rows(1));

        let cached = cache
            .lookup("ROOT\\CIMV2", "SELECT * FROM X")
            .expect("trafienie");
        assert_eq!(cached[0]["Value"], 1);
        assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 1 });
        assert_eq!(cache.stats().total(), 2);
    }

    #[test]
    fn cache_is_keyed_by_namespace_and_query() {
        let mut cache = QueryCache::new();
        cache.store("ROOT\\CIMV2", "SELECT * FROM X", rows(1));
        assert!(cache.lookup("ROOT\\WMI", "SELECT * FROM X").is_none());
        assert!(cache.lookup("ROOT\\CIMV2", "SELECT * FROM Y").is_none());
    }

    #[test]
    fn clear_starts_a_new_round() {
        let mut cache = QueryCache::new();
        cache.store("ROOT\\CIMV2", "Q", rows(1));
        let _ = cache.lookup("ROOT\\CIMV2", "Q");
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.stats(), CacheStats::default());
        assert!(cache.lookup("ROOT\\CIMV2", "Q").is_none());
    }

    #[test]
    fn cached_rows_are_independent_copies() {
        let mut cache = QueryCache::new();
        cache.store("ROOT\\CIMV2", "Q", rows(1));
        let mut first = cache.lookup("ROOT\\CIMV2", "Q").expect("trafienie");
        first.clear();
        let second = cache.lookup("ROOT\\CIMV2", "Q").expect("trafienie");
        assert_eq!(second.len(), 1);
    }
}
