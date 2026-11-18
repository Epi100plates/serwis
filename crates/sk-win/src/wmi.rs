use std::cell::RefCell;
use std::collections::HashMap;

use sk_core::error::{ModuleError, Result};
use sk_core::providers::WmiProvider;

/// Dostawca WMI dla Windows.
///
/// `WMIConnection` nie jest `Send` (każdy wątek musi zainicjalizować COM
/// niezależnie), dlatego połączenia trzymamy w `thread_local`. W praktyce
/// wszystkie zapytania idą z jednego, długo żyjącego wątku roboczego aplikacji.
#[derive(Debug, Default)]
pub struct WinWmi;

thread_local! {
    static CONNECTIONS: RefCell<HashMap<String, wmi::WMIConnection>> =
        RefCell::new(HashMap::new());
}

impl WinWmi {
    pub fn new() -> Self {
        Self
    }
}

impl WmiProvider for WinWmi {
    fn query(&self, namespace: &str, wql: &str) -> Result<Vec<serde_json::Value>> {
        CONNECTIONS.with(|cell| {
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
                .collect()
        })
    }
}
