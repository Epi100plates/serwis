//! Lista modułów w tej kompilacji.
//!
//! Dodanie modułu = dwie linie w tym pliku:
//! 1. `mod nazwa_pliku;`
//! 2. `registry.add::<nazwa_pliku::NazwaTypu>();` w funkcji `register`.
//!
//! Dopisuj alfabetycznie. Przy konflikcie scalania zachowaj obie pary linii.

use sk_core::registry::ModuleRegistry;

mod hw_battery;
mod hw_bios;
mod hw_board;
mod hw_cpu;
mod hw_disk_inventory;
mod hw_gpu;
mod hw_memory;
mod hw_monitor;
mod system_info;
mod system_live;

pub(crate) fn register(registry: &mut ModuleRegistry) {
    registry.add::<hw_battery::Battery>();
    registry.add::<hw_bios::BiosModule>();
    registry.add::<hw_board::Board>();
    registry.add::<hw_cpu::Cpu>();
    registry.add::<hw_disk_inventory::DiskInventory>();
    registry.add::<hw_gpu::Gpu>();
    registry.add::<hw_memory::Memory>();
    registry.add::<hw_monitor::Monitor>();
    registry.add::<system_info::SystemInfo>();
    registry.add::<system_live::Live>();
}
