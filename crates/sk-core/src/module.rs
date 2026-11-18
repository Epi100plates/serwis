use serde::{Deserialize, Serialize};

use crate::i18n::Locale;
use crate::providers::{Clock, HostEnv, LiveSource, MachineInfo, StorageProbe, WmiProvider};
use crate::view::View;

/// Jakie uprawnienia może potrzebować moduł. UI pokazuje to jako badge,
/// a wersja release i tak działa jako administrator (manifest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Zwykły użytkownik — działa wszędzie.
    Userland,
    /// Wymaga administratora (np. uchwyt do `\\.\PhysicalDriveN`).
    Elevated,
    /// Administrator + jawne potwierdzenie: operacja niszczy dane.
    Destructive,
}

impl Capability {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Userland => "common.capability.userland",
            Self::Elevated => "common.capability.elevated",
            Self::Destructive => "common.capability.destructive",
        }
    }
}

/// Grupa w nawigacji. Kolejność na liście wynika z tej deklaracji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Hardware,
    Storage,
    Migration,
    System,
    Tools,
}

impl Group {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Hardware => "common.group.hardware",
            Self::Storage => "common.group.storage",
            Self::Migration => "common.group.migration",
            Self::System => "common.group.system",
            Self::Tools => "common.group.tools",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Hardware => 0,
            Self::Storage => 1,
            Self::Migration => 2,
            Self::System => 3,
            Self::Tools => 4,
        }
    }
}

/// Statyczne metadane modułu. `id` jest publicznym kontraktem (klucz w raportach),
/// więc nie zmienia się bez decyzji architektonicznej.
#[derive(Debug, Clone, Copy)]
pub struct ModuleMeta {
    pub id: &'static str,
    pub name_key: &'static str,
    pub desc_key: &'static str,
    pub group: Group,
    pub capability: Capability,
    pub experimental: bool,
}

/// Wynik zbierania: surowe dane (raport JSON/HTML) i widok (UI).
#[derive(Debug, Clone)]
pub struct ModuleOutput {
    pub data: serde_json::Value,
    pub view: View,
}

/// Wszystko, co moduł dostaje ze świata. Wyłącznie traity — dzięki temu
/// moduł testuje się na makietach, bez sprzętu i bez administratora.
pub struct CollectCtx<'a> {
    pub wmi: &'a dyn WmiProvider,
    pub clock: &'a dyn Clock,
    pub host: &'a dyn HostEnv,
    /// Dane spoza WMI: firmware, VRAM, EDID (implementacja w `sk-win`).
    pub machine: &'a dyn MachineInfo,
    /// Dyski fizyczne i surowe SMART (`sk-win`; wymaga administratora).
    pub storage: &'a dyn StorageProbe,
    /// Historia próbek telemetrii (pusta, jeśli nikt jeszcze nie próbkował).
    pub live: &'a dyn LiveSource,
    pub locale: Locale,
}

pub trait Module: Send + Sync + 'static {
    fn meta(&self) -> ModuleMeta;

    fn collect(&self, ctx: &CollectCtx<'_>) -> crate::error::Result<ModuleOutput>;
}
