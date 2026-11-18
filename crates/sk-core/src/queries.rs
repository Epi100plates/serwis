//! Kanoniczne zapytania WMI współdzielone przez moduły i próbkowanie telemetrii.
//!
//! Cache w `sk-win` jest kluczowany po `(przestrzeń nazw, zapytanie)`, więc dwie
//! różne listy pól dla tej samej klasy to dwa zapytania w jednej rundzie. Dlatego
//! zapytania o klasy używane w kilku miejscach żyją tutaj, a nie w modułach.

/// System operacyjny: wersja, architektura i pamięć (potrzebna też telemetrii).
pub const WIN32_OPERATING_SYSTEM: &str = "SELECT Caption, Version, BuildNumber, \
     OSArchitecture, FreePhysicalMemory, TotalVisibleMemorySize FROM Win32_OperatingSystem";

/// Model komputera, RAM i informacje o platformie.
pub const WIN32_COMPUTER_SYSTEM: &str = "SELECT Manufacturer, Model, TotalPhysicalMemory, \
     HypervisorPresent, PCSystemType, NumberOfLogicalProcessors FROM Win32_ComputerSystem";

/// Adaptery graficzne: sterownik, VRAM (obcinany przez WMI) i bieżący tryb.
/// Współdzielone przez `hw.gpu` i `hw.monitor`.
pub const WIN32_VIDEO_CONTROLLER: &str = "SELECT Name, AdapterCompatibility, VideoProcessor, \
     DriverVersion, DriverDate, AdapterRAM, CurrentHorizontalResolution, \
     CurrentVerticalResolution, CurrentRefreshRate, CurrentBitsPerPixel \
     FROM Win32_VideoController";

/// Bieżące taktowanie procesora — współdzielone przez `hw.cpu` i telemetrię.
pub const PROCESSOR_PERFORMANCE: &str =
    "SELECT Active, InstanceName, frequency, percentage FROM ProcessorPerformance";

/// Dyski fizyczne: tożsamość, interfejs i geometria. Współdzielone przez
/// `hw.disk_inventory` i `storage.smart` — identyczna lista pól to jedno
/// zapytanie w rundzie.
pub const WIN32_DISK_DRIVE: &str = "SELECT Index, Model, InterfaceType, MediaType, Size, \
     BytesPerSector, FirmwareRevision, SerialNumber, Partitions, Status, PNPDeviceID \
     FROM Win32_DiskDrive";
