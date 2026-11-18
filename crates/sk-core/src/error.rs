use std::fmt;

/// Błąd modułu lub dostawcy danych. Zawsze niesie kontekst: co się nie udało
/// i skąd pochodzi błąd — technik ma wiedzieć, czego brakuje w raporcie.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    /// Dane są niedostępne na tym sprzęcie (np. brak baterii, SMART przez USB).
    #[error("niedostępne: {what}")]
    Unavailable { what: String },

    /// Dostawca danych zwrócił błąd (WMI, DeviceIoControl, plik).
    #[error("błąd {provider}: {message}")]
    Provider {
        provider: &'static str,
        message: String,
    },

    /// Brak uprawnień do wykonania operacji.
    #[error("brak uprawnień: {what}")]
    AccessDenied { what: String },

    /// Błąd wewnętrzny (niespójny stan, nieoczekiwana odpowiedź).
    #[error("błąd wewnętrzny: {0}")]
    Internal(String),
}

impl ModuleError {
    pub fn unavailable(what: impl Into<String>) -> Self {
        Self::Unavailable { what: what.into() }
    }

    pub fn provider(provider: &'static str, message: impl fmt::Display) -> Self {
        Self::Provider {
            provider,
            message: message.to_string(),
        }
    }

    pub fn access_denied(what: impl Into<String>) -> Self {
        Self::AccessDenied { what: what.into() }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

pub type Result<T> = std::result::Result<T, ModuleError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_keeps_source_and_message() {
        let err = ModuleError::provider("WMI", "Invalid class");
        let text = err.to_string();
        assert!(text.contains("WMI"), "{text}");
        assert!(text.contains("Invalid class"), "{text}");
    }

    #[test]
    fn access_denied_names_resource() {
        let err = ModuleError::access_denied("\\\\.\\PhysicalDrive0");
        assert!(err.to_string().contains("PhysicalDrive0"));
    }
}
