//! Czujniki w user mode: GPU (AMD ADL, NVIDIA NVML), strefy termiczne ACPI
//! i bateria (WMI).
//!
//! To jedyne miejsce z FFI dla czujników. Warstwa ta nie interpretuje odczytów
//! poza oczywistą normalizacją jednostek (miliwat, dziesiąte kelwina) — nazwy,
//! progi i wykresy należą do modułu `hw.sensors`.
//!
//! Brak biblioteki sterownika nie jest błędem: karta bez ADL/NVML po prostu nie
//! ma czujników, a raport działa dalej.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

use sk_core::providers::{NS_WMI, SensorProbe, WmiProvider};
use sk_core::sensors::{DeviceKind, SensorKind, SensorReading};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::Memory::{GetProcessHeap, HEAP_ZERO_MEMORY, HeapAlloc};
use windows::core::w;

use crate::ffi::symbol;
use crate::wmi::WinWmi;

/// Temperatury z ADL są w stopniach Celsjusza; powyżej tej granicy uznalibyśmy
/// odczyt za uszkodzony (karta raportuje 54000 °C, gdy czujnik nie istnieje).
const MAX_TEMPERATURE_C: f64 = 200.0;

const ACPI_ZONE_QUERY: &str =
    "SELECT CurrentTemperature, InstanceName FROM MSAcpi_ThermalZoneTemperature";
const BATTERY_QUERY: &str = "SELECT Temperature, Voltage, InstanceName FROM BatteryStatus";

// ---------------------------------------------------------------------------
// AMD ADL
// ---------------------------------------------------------------------------

type AdlAllocFn = unsafe extern "system" fn(i32) -> *mut c_void;
type AdlCreateFn = unsafe extern "system" fn(AdlAllocFn, i32, *mut *mut c_void) -> i32;
type AdlDestroyFn = unsafe extern "system" fn(*mut c_void) -> i32;
type AdlCountFn = unsafe extern "system" fn(*mut c_void, *mut i32) -> i32;
type AdlInfoFn = unsafe extern "system" fn(*mut c_void, *mut AdlAdapterInfo, i32) -> i32;
type AdlActiveFn = unsafe extern "system" fn(*mut c_void, i32, *mut i32) -> i32;
type AdlPmlogFn = unsafe extern "system" fn(*mut c_void, i32, *mut AdlPmlogData) -> i32;
type AdlOdnTempFn = unsafe extern "system" fn(*mut c_void, i32, i32, *mut i32) -> i32;
type AdlFanFn = unsafe extern "system" fn(*mut c_void, i32, i32, *mut AdlFanSpeed) -> i32;
type AdlActivityFn = unsafe extern "system" fn(*mut c_void, i32, *mut AdlActivity) -> i32;
type AdlPowerFn = unsafe extern "system" fn(*mut c_void, i32, i32, *mut i32) -> i32;

/// `ADLAdapterInfo` z `adl_structures.h` (wersja 18.1, Windows x64): 1572 B.
#[repr(C)]
#[derive(Clone, Copy)]
struct AdlAdapterInfo {
    size: i32,
    adapter_index: i32,
    udid: [u8; 256],
    bus_number: i32,
    device_number: i32,
    function_number: i32,
    vendor_id: i32,
    adapter_name: [u8; 256],
    display_name: [u8; 256],
    present: i32,
    exist: i32,
    driver_path: [u8; 256],
    driver_path_ext: [u8; 256],
    pnp_string: [u8; 256],
    os_display_index: i32,
}

impl AdlAdapterInfo {
    fn zeroed() -> Self {
        // SAFETY: struktura `#[repr(C)]` złożona wyłącznie z liczb i tablic bajtów;
        // wszystkie zera są poprawną wartością.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AdlSensorValue {
    supported: i32,
    value: i32,
}

/// `ADLPMLogDataOutput`: nagłówek + 256 wpisów `{supported, value}`.
#[repr(C)]
struct AdlPmlogData {
    size: i32,
    sensors: [AdlSensorValue; 256],
}

impl AdlPmlogData {
    fn new() -> Self {
        Self {
            size: size_of::<Self>() as i32,
            sensors: [AdlSensorValue {
                supported: 0,
                value: 0,
            }; 256],
        }
    }

    fn value(&self, index: usize) -> Option<i32> {
        let sensor = self.sensors.get(index)?;
        (sensor.supported != 0).then_some(sensor.value)
    }
}

#[repr(C)]
struct AdlFanSpeed {
    size: i32,
    speed_type: i32,
    fan_speed: i32,
    flags: i32,
}

#[repr(C)]
struct AdlActivity {
    size: i32,
    engine_clock: i32,
    memory_clock: i32,
    vddc: i32,
    activity_percent: i32,
    current_performance_level: i32,
    current_bus_speed: i32,
    current_bus_lanes: i32,
    maximum_bus_lanes: i32,
    reserved: i32,
}

/// Wskaźniki PMLog, z których korzystamy (numeracja `ADLPMLogSensors`).
const PMLOG_GFX_CLOCK: usize = 1;
const PMLOG_MEM_CLOCK: usize = 2;
const PMLOG_TEMP_EDGE: usize = 8;
const PMLOG_TEMP_MEM: usize = 9;
const PMLOG_FAN_RPM: usize = 14;
const PMLOG_FAN_PERCENT: usize = 15;
const PMLOG_GFX_ACTIVITY: usize = 19;
const PMLOG_GFX_VOLTAGE: usize = 21;
const PMLOG_ASIC_POWER: usize = 23;
const PMLOG_TEMP_HOTSPOT: usize = 27;
const PMLOG_TEMP_GFX: usize = 28;

/// `ADL_ODN_TEMPERATURE_*` z `ADLODNTemperatureType` (nazwy z dokumentacji AMD).
const ODN_TEMPERATURE_EDGE: i32 = 1;
const ODN_TEMPERATURE_MEM: i32 = 2;
const ODN_TEMPERATURE_HOTSPOT: i32 = 7;

struct AdlAdapter {
    index: i32,
    name: String,
    bus: i32,
    device: i32,
    function: i32,
}

/// Załadowana `atiadlxx.dll` z kontekstem ADL. `None` = brak sterownika AMD.
struct Adl {
    module: HMODULE,
    context: *mut c_void,
    destroy: AdlDestroyFn,
    pmlog: AdlPmlogFn,
    odn_temperature: Option<AdlOdnTempFn>,
    fan_speed: Option<AdlFanFn>,
    activity: Option<AdlActivityFn>,
    power: Option<AdlPowerFn>,
    adapters: Vec<AdlAdapter>,
}

// Kontekst ADL jest używany wyłącznie z jednego wątku roboczego aplikacji.
// `WinSensors` nie jest współdzielony między wątkami, więc surowe wskaźniki
// nie opuszczają tego modułu.
impl Adl {
    fn load() -> Option<Self> {
        // SAFETY: `LoadLibraryW` z literałem zakończonym NUL; wynik jest zwalniany
        // w `Drop`, a brak biblioteki obsługujemy jako `None`.
        let module = unsafe { LoadLibraryW(w!("atiadlxx.dll")) }.ok()?;
        let create: AdlCreateFn = match symbol(module, "ADL2_Main_Control_Create") {
            Some(create) => create,
            None => {
                // SAFETY: uchwyt pochodzi z `LoadLibraryW` i nie jest już potrzebny.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };
        let destroy: AdlDestroyFn = match symbol(module, "ADL2_Main_Control_Destroy") {
            Some(destroy) => destroy,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };
        let pmlog: AdlPmlogFn = match symbol(module, "ADL2_New_QueryPMLogData_Get") {
            Some(pmlog) => pmlog,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };

        let mut context: *mut c_void = ptr::null_mut();
        // SAFETY: callback alokuje pamięć procesu (HeapAlloc), zgodnie z kontraktem
        // ADL; kontekst jest niszczony w `Drop`.
        let status = unsafe { create(adl_alloc, 1, &mut context) };
        if status != 0 || context.is_null() {
            // SAFETY: biblioteka załadowana, kontekst nie powstał.
            unsafe {
                let _ = FreeLibrary(module);
            }
            return None;
        }

        let adapters = enumerate_adapters(module, context);
        Some(Self {
            module,
            context,
            destroy,
            pmlog,
            odn_temperature: symbol(module, "ADL2_OverdriveN_Temperature_Get"),
            fan_speed: symbol(module, "ADL2_Overdrive5_FanSpeed_Get"),
            activity: symbol(module, "ADL2_Overdrive5_CurrentActivity_Get"),
            power: symbol(module, "ADL2_Overdrive6_CurrentPower_Get"),
            adapters,
        })
    }

    fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        for adapter in &self.adapters {
            let device = adapter.name.clone();
            let mut add = |kind: SensorKind, value: f64| {
                readings.push(SensorReading::new(
                    device.clone(),
                    DeviceKind::Gpu,
                    kind,
                    value,
                ));
            };

            if !self.read_pmlog(adapter.index, &mut add) {
                self.read_overdrive(adapter.index, &mut add);
            }
        }
        readings
    }

    /// Nowoczesne karty (RDNA i nowsze) raportują wszystko przez PMLog; starsze
    /// zwracają błąd i wtedy schodzimy do OverdriveN/Overdrive5.
    fn read_pmlog(&self, adapter: i32, add: &mut impl FnMut(SensorKind, f64)) -> bool {
        let mut data = AdlPmlogData::new();
        // SAFETY: wskaźnik i rozmiar struktury są zgodne z ADL; kontekst żyje
        // przez cały czas życia `Adl`.
        let status = unsafe { (self.pmlog)(self.context, adapter, &mut data) };
        if status != 0 {
            return false;
        }

        let mut found_temperature = false;
        if let Some(value) = data.value(PMLOG_TEMP_EDGE).map(f64::from) {
            if valid_temperature(value) {
                add(SensorKind::Temperature, value);
                found_temperature = true;
            }
        } else if let Some(value) = data.value(PMLOG_TEMP_GFX).map(f64::from)
            && valid_temperature(value)
        {
            add(SensorKind::Temperature, value);
            found_temperature = true;
        }
        if let Some(value) = data.value(PMLOG_TEMP_HOTSPOT).map(f64::from)
            && valid_temperature(value)
        {
            add(SensorKind::Hotspot, value);
        }
        if let Some(value) = data.value(PMLOG_TEMP_MEM).map(f64::from)
            && valid_temperature(value)
        {
            add(SensorKind::MemoryTemp, value);
        }

        if let Some(rpm) = data.value(PMLOG_FAN_RPM)
            && (0..=20_000).contains(&rpm)
        {
            add(SensorKind::FanRpm, f64::from(rpm));
        }
        if let Some(percent) = data.value(PMLOG_FAN_PERCENT)
            && (0..=100).contains(&percent)
        {
            add(SensorKind::FanPercent, f64::from(percent));
        }
        if let Some(power) = data.value(PMLOG_ASIC_POWER)
            && (0..=2_000).contains(&power)
        {
            add(SensorKind::Power, f64::from(power));
        }
        if let Some(mhz) = data.value(PMLOG_GFX_CLOCK)
            && (0..=10_000).contains(&mhz)
        {
            add(SensorKind::CoreClock, f64::from(mhz));
        }
        if let Some(mhz) = data.value(PMLOG_MEM_CLOCK)
            && (0..=20_000).contains(&mhz)
        {
            add(SensorKind::MemoryClock, f64::from(mhz));
        }
        if let Some(percent) = data.value(PMLOG_GFX_ACTIVITY)
            && (0..=100).contains(&percent)
        {
            add(SensorKind::Load, f64::from(percent));
        }
        if let Some(millivolts) = data.value(PMLOG_GFX_VOLTAGE)
            && (0..=2_000).contains(&millivolts)
        {
            add(SensorKind::Voltage, f64::from(millivolts) / 1000.0);
        }

        // PMLog odpowiada, ale karta może nie mieć czujnika temperatury w tej
        // tabeli — wtedy warto spróbować starszego API.
        found_temperature
    }

    fn read_overdrive(&self, adapter: i32, add: &mut impl FnMut(SensorKind, f64)) {
        if let Some(temperature) = self.odn_temperature {
            for (kind, raw_kind) in [
                (SensorKind::Temperature, ODN_TEMPERATURE_EDGE),
                (SensorKind::MemoryTemp, ODN_TEMPERATURE_MEM),
                (SensorKind::Hotspot, ODN_TEMPERATURE_HOTSPOT),
            ] {
                let mut value = 0i32;
                // SAFETY: cztery argumenty zgodne z `ADL2_OverdriveN_Temperature_Get`.
                let status = unsafe { temperature(self.context, adapter, raw_kind, &mut value) };
                if status == 0 {
                    let celsius = overdrive_temperature(value);
                    if valid_temperature(celsius) {
                        add(kind, celsius);
                    }
                }
            }
        }

        if let Some(fan_speed) = self.fan_speed {
            for (kind, speed_type) in [(SensorKind::FanRpm, 2), (SensorKind::FanPercent, 1)] {
                let mut value = AdlFanSpeed {
                    size: size_of::<AdlFanSpeed>() as i32,
                    speed_type,
                    fan_speed: 0,
                    flags: 0,
                };
                // SAFETY: struktura i rozmiar zgodne z ADL; kontroler termiczny 0.
                let status = unsafe { fan_speed(self.context, adapter, 0, &mut value) };
                let limit = if speed_type == 2 { 20_000 } else { 100 };
                if status == 0 && (0..=limit).contains(&value.fan_speed) {
                    add(kind, f64::from(value.fan_speed));
                }
            }
        }

        if let Some(activity) = self.activity {
            let mut value = AdlActivity {
                size: size_of::<AdlActivity>() as i32,
                engine_clock: 0,
                memory_clock: 0,
                vddc: 0,
                activity_percent: 0,
                current_performance_level: 0,
                current_bus_speed: 0,
                current_bus_lanes: 0,
                maximum_bus_lanes: 0,
                reserved: 0,
            };
            // SAFETY: struktura i rozmiar zgodne z ADL.
            let status = unsafe { activity(self.context, adapter, &mut value) };
            if status == 0 {
                if (0..=10_000).contains(&value.engine_clock) && value.engine_clock > 0 {
                    add(SensorKind::CoreClock, f64::from(value.engine_clock));
                }
                if (0..=20_000).contains(&value.memory_clock) && value.memory_clock > 0 {
                    add(SensorKind::MemoryClock, f64::from(value.memory_clock));
                }
                if (0..=100).contains(&value.activity_percent) {
                    add(SensorKind::Load, f64::from(value.activity_percent));
                }
            }
        }

        if let Some(power) = self.power {
            let mut value = 0i32;
            // SAFETY: typ mocy 0 = całkowita moc GPU, zgodnie z ADL.
            let status = unsafe { power(self.context, adapter, 0, &mut value) };
            // ADL zwraca moc w formacie 8.8 W.
            let watts = f64::from(value >> 8);
            if status == 0 && (0.0..=2_000.0).contains(&watts) {
                add(SensorKind::Power, watts);
            }
        }
    }
}

impl Drop for Adl {
    fn drop(&mut self) {
        // SAFETY: kontekst i uchwyt biblioteki pochodzą z tej samej instancji.
        unsafe {
            let _ = (self.destroy)(self.context);
            let _ = FreeLibrary(self.module);
        }
    }
}

extern "system" fn adl_alloc(size: i32) -> *mut c_void {
    if size <= 0 {
        return ptr::null_mut();
    }
    // SAFETY: alokacja w stercie procesu, zgodnie z `ADL_MAIN_MALLOC_CALLBACK`.
    unsafe {
        match GetProcessHeap() {
            Ok(heap) => HeapAlloc(heap, HEAP_ZERO_MEMORY, size as usize),
            Err(_) => ptr::null_mut(),
        }
    }
}

fn enumerate_adapters(module: HMODULE, context: *mut c_void) -> Vec<AdlAdapter> {
    let count: AdlCountFn = match symbol(module, "ADL2_Adapter_NumberOfAdapters_Get") {
        Some(count) => count,
        None => return Vec::new(),
    };
    let info: AdlInfoFn = match symbol(module, "ADL2_Adapter_AdapterInfo_Get") {
        Some(info) => info,
        None => return Vec::new(),
    };
    let active: Option<AdlActiveFn> = symbol(module, "ADL2_Adapter_Active_Get");

    let mut total = 0i32;
    // SAFETY: wskaźnik do lokalnej zmiennej; kontekst żyje.
    if unsafe { count(context, &mut total) } != 0 || total <= 0 {
        return Vec::new();
    }
    let total = total.min(64) as usize;
    let mut entries = vec![AdlAdapterInfo::zeroed(); total];
    let bytes = (total * size_of::<AdlAdapterInfo>()) as i32;
    // SAFETY: bufor ma dokładnie `bytes` bajtów i jest odpowiednio wyrównany.
    if unsafe { info(context, entries.as_mut_ptr(), bytes) } != 0 {
        return Vec::new();
    }

    let mut adapters = Vec::new();
    for entry in entries {
        if entry.exist == 0 || entry.present == 0 {
            continue;
        }
        if let Some(active) = active {
            let mut is_active = 0i32;
            // SAFETY: indeks pochodzi z ADL.
            if unsafe { active(context, entry.adapter_index, &mut is_active) } != 0
                || is_active == 0
            {
                continue;
            }
        }
        // Jedna karta ma kilka wpisów ADL (po jednym na wyjście). Zostawiamy
        // pierwszy dla danego adresu PCI, inaczej czujniki policzyłyby się dwa razy.
        if adapters.iter().any(|other: &AdlAdapter| {
            other.bus == entry.bus_number
                && other.device == entry.device_number
                && other.function == entry.function_number
        }) {
            continue;
        }
        adapters.push(AdlAdapter {
            index: entry.adapter_index,
            name: c_string(&entry.adapter_name),
            bus: entry.bus_number,
            device: entry.device_number,
            function: entry.function_number,
        });
    }
    adapters
}

/// Nazwa z bufora ADL: UTF-8 do pierwszego zera. ADL zwraca ASCII, ale nie
/// zakładamy tego — niepoprawne bajty zamieniamy, zamiast panikować.
fn c_string(buffer: &[u8]) -> String {
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).trim().to_string()
}

/// Odczyty temperatury OverdriveN są w milistopniach, ale starsze sterowniki
/// potrafią zwrócić stopnie. Rozstrzygamy to zakresem, nie wersją sterownika.
fn overdrive_temperature(raw: i32) -> f64 {
    let value = f64::from(raw);
    if value.abs() > 1000.0 {
        value / 1000.0
    } else {
        value
    }
}

fn valid_temperature(celsius: f64) -> bool {
    celsius.is_finite() && celsius > 0.0 && celsius < MAX_TEMPERATURE_C
}

// ---------------------------------------------------------------------------
// NVIDIA NVML
// ---------------------------------------------------------------------------

type NvmlInitFn = unsafe extern "system" fn() -> i32;
type NvmlShutdownFn = unsafe extern "system" fn() -> i32;
type NvmlCountFn = unsafe extern "system" fn(*mut u32) -> i32;
type NvmlHandleFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;
type NvmlNameFn = unsafe extern "system" fn(*mut c_void, *mut i8, u32) -> i32;
type NvmlTemperatureFn = unsafe extern "system" fn(*mut c_void, i32, *mut u32) -> i32;
type NvmlFanFn = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
type NvmlPowerFn = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
type NvmlClockFn = unsafe extern "system" fn(*mut c_void, i32, *mut u32) -> i32;
type NvmlUtilizationFn = unsafe extern "system" fn(*mut c_void, *mut NvmlUtilization) -> i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

/// `nvmlTemperatureSensors_t::NVML_TEMPERATURE_GPU`.
const NVML_TEMPERATURE_GPU: i32 = 0;
/// `nvmlClockType_t`: GRAPHICS = 0, MEM = 2.
const NVML_CLOCK_GRAPHICS: i32 = 0;
const NVML_CLOCK_MEM: i32 = 2;

struct NvmlDevice {
    handle: *mut c_void,
    name: String,
}

struct Nvml {
    module: HMODULE,
    shutdown: NvmlShutdownFn,
    temperature: Option<NvmlTemperatureFn>,
    fan: Option<NvmlFanFn>,
    power: Option<NvmlPowerFn>,
    clock: Option<NvmlClockFn>,
    utilization: Option<NvmlUtilizationFn>,
    devices: Vec<NvmlDevice>,
}

impl Nvml {
    fn load() -> Option<Self> {
        // NVIDIA instaluje `nvml.dll` w System32 razem ze sterownikiem.
        // SAFETY: literał zakończony NUL; biblioteka zwalniana w `Drop`.
        let module = unsafe { LoadLibraryW(w!("nvml.dll")) }.ok()?;
        let init: NvmlInitFn =
            match symbol(module, "nvmlInit_v2").or_else(|| symbol(module, "nvmlInit")) {
                Some(init) => init,
                None => {
                    // SAFETY: uchwyt z `LoadLibraryW`.
                    unsafe {
                        let _ = FreeLibrary(module);
                    }
                    return None;
                }
            };
        let shutdown: NvmlShutdownFn = match symbol(module, "nvmlShutdown") {
            Some(shutdown) => shutdown,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };
        let count: NvmlCountFn = match symbol(module, "nvmlDeviceGetCount_v2")
            .or_else(|| symbol(module, "nvmlDeviceGetCount"))
        {
            Some(count) => count,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };
        let handle_of: NvmlHandleFn = match symbol(module, "nvmlDeviceGetHandleByIndex_v2")
            .or_else(|| symbol(module, "nvmlDeviceGetHandleByIndex"))
        {
            Some(handle) => handle,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };
        let name: NvmlNameFn = match symbol(module, "nvmlDeviceGetName") {
            Some(name) => name,
            None => {
                // SAFETY: jak wyżej.
                unsafe {
                    let _ = FreeLibrary(module);
                }
                return None;
            }
        };

        // SAFETY: brak argumentów, zwraca status NVML.
        if unsafe { init() } != 0 {
            // SAFETY: biblioteka załadowana, NVML nie zainicjalizowany.
            unsafe {
                let _ = FreeLibrary(module);
            }
            return None;
        }

        let mut devices = Vec::new();
        let mut total = 0u32;
        // SAFETY: wskaźnik do lokalnej zmiennej.
        if unsafe { count(&mut total) } == 0 {
            for index in 0..total.min(16) {
                let mut device: *mut c_void = ptr::null_mut();
                // SAFETY: indeks z zakresu zwróconego przez NVML.
                if unsafe { handle_of(index, &mut device) } != 0 || device.is_null() {
                    continue;
                }
                let mut buffer = [0i8; 96];
                // SAFETY: bufor ma 96 B, tyle podajemy NVML.
                let status = unsafe { name(device, buffer.as_mut_ptr(), buffer.len() as u32) };
                if status != 0 {
                    continue;
                }
                let bytes: Vec<u8> = buffer.iter().map(|byte| *byte as u8).collect();
                devices.push(NvmlDevice {
                    handle: device,
                    name: c_string(&bytes),
                });
            }
        }

        Some(Self {
            module,
            shutdown,
            temperature: symbol(module, "nvmlDeviceGetTemperature"),
            fan: symbol(module, "nvmlDeviceGetFanSpeed"),
            power: symbol(module, "nvmlDeviceGetPowerUsage"),
            clock: symbol(module, "nvmlDeviceGetClockInfo"),
            utilization: symbol(module, "nvmlDeviceGetUtilizationRates"),
            devices,
        })
    }

    fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        for device in &self.devices {
            let name = device.name.clone();
            let mut add = |kind: SensorKind, value: f64| {
                readings.push(SensorReading::new(
                    name.clone(),
                    DeviceKind::Gpu,
                    kind,
                    value,
                ));
            };

            if let Some(temperature) = self.temperature {
                let mut value = 0u32;
                // SAFETY: uchwyt NVML i typ czujnika GPU.
                if unsafe { temperature(device.handle, NVML_TEMPERATURE_GPU, &mut value) } == 0 {
                    let celsius = f64::from(value);
                    if valid_temperature(celsius) {
                        add(SensorKind::Temperature, celsius);
                    }
                }
            }
            if let Some(fan) = self.fan {
                let mut value = 0u32;
                // SAFETY: uchwyt NVML.
                if unsafe { fan(device.handle, &mut value) } == 0 && value <= 100 {
                    add(SensorKind::FanPercent, f64::from(value));
                }
            }
            if let Some(power) = self.power {
                let mut milliwatts = 0u32;
                // SAFETY: uchwyt NVML; NVML zwraca moc w mW.
                if unsafe { power(device.handle, &mut milliwatts) } == 0 && milliwatts <= 2_000_000
                {
                    add(SensorKind::Power, f64::from(milliwatts) / 1000.0);
                }
            }
            if let Some(clock) = self.clock {
                for (kind, clock_type, limit) in [
                    (SensorKind::CoreClock, NVML_CLOCK_GRAPHICS, 10_000u32),
                    (SensorKind::MemoryClock, NVML_CLOCK_MEM, 20_000u32),
                ] {
                    let mut mhz = 0u32;
                    // SAFETY: uchwyt NVML i typ taktowania.
                    if unsafe { clock(device.handle, clock_type, &mut mhz) } == 0 && mhz <= limit {
                        add(kind, f64::from(mhz));
                    }
                }
            }
            if let Some(utilization) = self.utilization {
                let mut value = NvmlUtilization::default();
                // SAFETY: uchwyt NVML; struktura o dwóch polach `u32`.
                if unsafe { utilization(device.handle, &mut value) } == 0 && value.gpu <= 100 {
                    add(SensorKind::Load, f64::from(value.gpu));
                }
            }
        }
        readings
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: NVML został zainicjalizowany w `load`, uchwyt z tej instancji.
        unsafe {
            let _ = (self.shutdown)();
            let _ = FreeLibrary(self.module);
        }
    }
}

// ---------------------------------------------------------------------------
// Publiczny dostawca
// ---------------------------------------------------------------------------

/// Wszystkie czujniki dostępne na tej maszynie.
///
/// Kolejność źródeł jest stała, więc raport jest powtarzalny. Błąd jednego
/// źródła nie zabiera pozostałych: brak ADL/NVML/PawnIO to po prostu brak
/// odczytów, a nie błąd modułu.
pub struct WinSensors {
    wmi: WinWmi,
    adl: Option<Adl>,
    nvml: Option<Nvml>,
    ring0: Ring0,
}

/// Czujniki wymagające ring 0 (PawnIO). Oba pola są opcjonalne: sterownik może
/// nie być zainstalowany albo płyta nie mieć Super I/O.
#[derive(Default)]
struct Ring0 {
    cpu: Option<crate::cpu::CpuSensors>,
    board: Option<crate::superio::SuperIo>,
}

impl Ring0 {
    fn new() -> Self {
        Self {
            cpu: crate::cpu::CpuSensors::probe(),
            board: crate::superio::SuperIo::probe(),
        }
    }

    fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        if let Some(cpu) = &self.cpu {
            readings.extend(cpu.readings());
        }
        if let Some(board) = &self.board {
            readings.extend(board.readings());
        }
        readings
    }
}

impl Default for WinSensors {
    fn default() -> Self {
        Self::new()
    }
}

impl WinSensors {
    pub fn new() -> Self {
        Self {
            wmi: WinWmi::new(),
            adl: Adl::load(),
            nvml: Nvml::load(),
            ring0: Ring0::new(),
        }
    }

    /// Czy odczyty GPU są w ogóle możliwe (sterownik udostępnia ADL albo NVML).
    pub fn gpu_backend(&self) -> Option<&'static str> {
        if self.adl.is_some() {
            Some("adl")
        } else if self.nvml.is_some() {
            Some("nvml")
        } else {
            None
        }
    }

    /// Stan źródeł dla notek modułu: czy PawnIO jest zainstalowany i czy
    /// czujniki CPU oraz Super I/O odpowiadają.
    pub fn probe(&self) -> SensorProbe {
        SensorProbe {
            ring0: crate::pawnio::is_installed(),
            cpu: self.ring0.cpu.is_some(),
            board: self.ring0.board.is_some(),
        }
    }

    pub fn readings(&self) -> Vec<SensorReading> {
        let mut readings = Vec::new();
        if let Some(adl) = &self.adl {
            readings.extend(adl.readings());
        }
        if let Some(nvml) = &self.nvml {
            readings.extend(nvml.readings());
        }
        readings.extend(self.acpi_zones());
        readings.extend(self.battery());
        readings.extend(self.ring0.readings());
        readings
    }

    fn acpi_zones(&self) -> Vec<SensorReading> {
        match self.wmi.query(NS_WMI, ACPI_ZONE_QUERY) {
            Ok(rows) => parse_acpi_zones(&rows),
            Err(error) => {
                tracing::debug!(%error, "brak stref termicznych ACPI");
                Vec::new()
            }
        }
    }

    fn battery(&self) -> Vec<SensorReading> {
        match self.wmi.query(NS_WMI, BATTERY_QUERY) {
            Ok(rows) => parse_battery(&rows),
            Err(error) => {
                tracing::debug!(%error, "brak danych baterii");
                Vec::new()
            }
        }
    }
}

/// Wiersze `MSAcpi_ThermalZoneTemperature`: `CurrentTemperature` w dziesiątych
/// kelwina, `InstanceName` jako nazwa strefy.
fn parse_acpi_zones(rows: &[serde_json::Value]) -> Vec<SensorReading> {
    let mut readings = Vec::new();
    for row in rows {
        let Some(raw) = row
            .get("CurrentTemperature")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let Some(celsius) = kelvin_tenths_to_celsius(raw) else {
            continue;
        };
        let device = row
            .get("InstanceName")
            .and_then(serde_json::Value::as_str)
            .map(short_instance_name)
            .unwrap_or_default();
        readings.push(SensorReading::new(
            device,
            DeviceKind::Board,
            SensorKind::Temperature,
            celsius,
        ));
    }
    readings
}

/// Wiersze `BatteryStatus`: temperatura w dziesiątych kelwina, napięcie w mV.
fn parse_battery(rows: &[serde_json::Value]) -> Vec<SensorReading> {
    let mut readings = Vec::new();
    for row in rows {
        let device = row
            .get("InstanceName")
            .and_then(serde_json::Value::as_str)
            .map(short_instance_name)
            .unwrap_or_default();
        if let Some(raw) = row.get("Temperature").and_then(serde_json::Value::as_u64)
            && let Some(celsius) = kelvin_tenths_to_celsius(raw)
        {
            readings.push(SensorReading::new(
                device.clone(),
                DeviceKind::Battery,
                SensorKind::Temperature,
                celsius,
            ));
        }
        if let Some(millivolts) = row.get("Voltage").and_then(serde_json::Value::as_u64)
            && (1_000..=30_000).contains(&millivolts)
        {
            readings.push(SensorReading::new(
                device,
                DeviceKind::Battery,
                SensorKind::Voltage,
                millivolts as f64 / 1000.0,
            ));
        }
    }
    readings
}

/// Bateria i strefy ACPI raportują temperaturę w dziesiątych stopnia kelwina.
fn kelvin_tenths_to_celsius(raw: u64) -> Option<f64> {
    let celsius = raw as f64 / 10.0 - 273.15;
    valid_temperature(celsius).then_some(celsius)
}

/// Ostatni człon nazwy instancji WMI: `ACPI\ThermalZone\TZ00` → `TZ00`.
fn short_instance_name(name: &str) -> String {
    name.rsplit('\\').next().unwrap_or(name).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kelvin_tenths_become_celsius() {
        // 3000 × 0,1 K = 300 K ≈ 26,9 °C (typowa bateria).
        let value = kelvin_tenths_to_celsius(3000).expect("temperatura");
        assert!((value - 26.85).abs() < 0.01, "{value}");
        assert_eq!(kelvin_tenths_to_celsius(0), None);
        assert_eq!(kelvin_tenths_to_celsius(1_000_000), None);
    }

    #[test]
    fn overdrive_temperature_accepts_both_units() {
        assert!((overdrive_temperature(57_000) - 57.0).abs() < f64::EPSILON);
        assert!((overdrive_temperature(57) - 57.0).abs() < f64::EPSILON);
    }

    #[test]
    fn short_instance_name_keeps_last_component() {
        assert_eq!(short_instance_name("ACPI\\ThermalZone\\TZ00"), "TZ00");
        assert_eq!(short_instance_name("ACPI\\PNP0C0A\\1_0"), "1_0");
        assert_eq!(short_instance_name("TZ01"), "TZ01");
    }

    #[test]
    fn adapter_info_has_expected_layout_size() {
        assert_eq!(size_of::<AdlAdapterInfo>(), 1572);
    }

    #[test]
    fn acpi_zone_rows_become_board_temperatures() {
        let rows = vec![
            serde_json::json!({
                "CurrentTemperature": 3000,
                "InstanceName": "ACPI\\ThermalZone\\TZ00"
            }),
            serde_json::json!({
                "CurrentTemperature": 3100,
                "InstanceName": "ACPI\\ThermalZone\\TZ01"
            }),
            // Zero = brak pomiaru, nie 0 K.
            serde_json::json!({ "CurrentTemperature": 0 }),
        ];
        let readings = parse_acpi_zones(&rows);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].device, "TZ00");
        assert_eq!(readings[0].device_kind, DeviceKind::Board);
        assert!((readings[0].value - 26.85).abs() < 0.01);
        assert!((readings[1].value - 36.85).abs() < 0.01);
    }

    #[test]
    fn battery_rows_carry_temperature_and_voltage() {
        let rows = vec![serde_json::json!({
            "Temperature": 2992,
            "Voltage": 11800,
            "InstanceName": "ACPI\\PNP0C0A\\1_0",
        })];
        let readings = parse_battery(&rows);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].kind, SensorKind::Temperature);
        assert!((readings[0].value - 26.05).abs() < 0.01);
        assert_eq!(readings[1].kind, SensorKind::Voltage);
        assert!((readings[1].value - 11.8).abs() < f64::EPSILON);
        assert_eq!(readings[1].device, "1_0");
    }

    #[test]
    fn battery_without_sensor_values_is_empty() {
        let readings = parse_battery(&[serde_json::json!({ "Voltage": 0 })]);
        assert!(readings.is_empty());
    }

    /// Wymaga sterownika AMD z ADL; na maszynie bez AMD test nic nie sprawdza.
    #[test]
    #[ignore = "sprzętowy: wymaga karty AMD z ADL"]
    fn amd_gpu_reports_plausible_sensors() {
        let _serial = crate::pawnio::PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sensors = WinSensors::new();
        let readings = sensors.readings();
        let temperatures: Vec<&SensorReading> = readings
            .iter()
            .filter(|reading| {
                reading.device_kind == DeviceKind::Gpu && reading.kind == SensorKind::Temperature
            })
            .collect();
        if temperatures.is_empty() {
            eprintln!("ADL nie zwrócił temperatur GPU — pominięto");
            return;
        }
        for reading in temperatures {
            assert!(
                reading.value > 10.0 && reading.value < 120.0,
                "temperatura GPU {} °C",
                reading.value
            );
        }
    }
}
