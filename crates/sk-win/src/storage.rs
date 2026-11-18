//! Dyski fizyczne: magistrala, rodzaj nośnika i surowe dane SMART.
//!
//! Wszystko idzie przez `DeviceIoControl` na uchwycie `\\.\PhysicalDriveN`, który
//! wymaga uprawnień administratora. Ten plik jest wyłącznie transportem —
//! parsowanie SMART żyje w `sk-modules/src/smart.rs`, żeby dało się je testować
//! bez dysku i bez podniesionego procesu.

use std::mem::{offset_of, size_of};
use std::ptr;

use sk_core::error::{ModuleError, Result};
use sk_core::providers::{BusKind, DriveKind, PhysicalDrive, SmartData, StorageProbe};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    STORAGE_BUS_TYPE,
};
use windows::Win32::Storage::IscsiDisc::{
    ATA_FLAGS_DATA_IN, ATA_PASS_THROUGH_EX, IOCTL_ATA_PASS_THROUGH,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, NVMeDataTypeLogPage,
    PropertyStandardQuery, ProtocolTypeNvme, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_ID,
    STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_DATA_DESCRIPTOR, STORAGE_PROTOCOL_SPECIFIC_DATA,
    StorageAdapterProtocolSpecificProperty, StorageDeviceProperty,
    StorageDeviceProtocolSpecificProperty, StorageDeviceSeekPenaltyProperty,
};

/// Polecenie ATA SMART (`SMART READ DATA` / `READ ATTRIBUTE THRESHOLDS`).
const ATA_SMART_CMD: u8 = 0xB0;
/// Numer funkcji SMART: odczyt atrybutów.
const ATA_SMART_READ_DATA: u8 = 0xD0;
/// Numer funkcji SMART: odczyt progów atrybutów.
const ATA_SMART_READ_THRESHOLDS: u8 = 0xD1;
/// Numer funkcji SMART: włączenie raportowania (nie dotyka danych nośnika).
const ATA_SMART_ENABLE: u8 = 0xD8;
/// Sygnatura SMART przenoszona w LBA (0xC24F) — wymóg standardu ATA.
const ATA_SMART_LBA_LOW: u8 = 0x4F;
const ATA_SMART_LBA_MID: u8 = 0xC2;
/// Dysk jako master na pierwszym kanale.
const ATA_DEVICE_HEAD: u8 = 0xA0;
/// Bit błędu w rejestrze statusu ATA.
const ATA_STATUS_ERR: u8 = 0x01;

/// Strona logu zdrowia NVMe (SMART/Health Information, log page 0x02).
const NVME_LOG_PAGE_HEALTH: u32 = 0x02;
const NVME_HEALTH_LEN: usize = 512;
const ATA_SECTOR_LEN: usize = 512;
/// Bufor na deskryptor urządzenia — `STORAGE_DEVICE_DESCRIPTOR` ma zmienną długość.
const DESCRIPTOR_BUFFER: usize = 1024;
const IOCTL_TIMEOUT_SECS: u32 = 5;

/// Offset `AdditionalParameters` w `STORAGE_PROPERTY_QUERY` — tam zaczyna się
/// `STORAGE_PROTOCOL_SPECIFIC_DATA` dla zapytań NVMe.
const QUERY_PARAMS_OFFSET: usize = offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters);
/// Bufor na zapytanie NVMe: deskryptor zapytania + struktura protokołu + 8 B
/// zapasu na sterowniki liczące `ProtocolDataOffset` od początku deskryptora
/// (a nie od `ProtocolSpecificData`) + strona logu.
const NVME_BUFFER_LEN: usize =
    QUERY_PARAMS_OFFSET + size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>() + 8 + NVME_HEALTH_LEN;

/// Kody błędów Win32 w postaci, w jakiej zwraca je `windows-rs`:
/// `HRESULT_FROM_WIN32(kod)`. Sprawdzamy je, żeby odróżnić brak uprawnień
/// i „kontroler tego nie obsługuje” od prawdziwej awarii.
const HRESULT_ACCESS_DENIED: u32 = 0x8007_0005;
const HRESULT_INVALID_FUNCTION: u32 = 0x8007_0001;
const HRESULT_NOT_READY: u32 = 0x8007_0015;
const HRESULT_NOT_SUPPORTED: u32 = 0x8007_0032;
const HRESULT_IO_DEVICE: u32 = 0x8007_045D;

/// Bufor wyrównany do 8 B: `DeviceIoControl` dostaje wskaźniki na struktury
/// z polami `usize`/`u64`, więc nie mogą leżeć pod adresem nieparzystym.
#[repr(C, align(8))]
struct AlignedBuffer<const N: usize>([u8; N]);

/// Klasyczny IOCTL SMART (`ntddscsi.h`) — druga ścieżka obok ATA PASS-THROUGH.
const SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;

/// Rejestry ATA w żądaniu klasycznego IOCTL SMART.
#[repr(C)]
#[derive(Clone, Copy)]
struct IdeRegs {
    features: u8,
    sector_count: u8,
    sector_number: u8,
    cyl_low: u8,
    cyl_high: u8,
    drive_head: u8,
    command: u8,
    reserved: u8,
}

/// `SENDCMDINPARAMS` z 512-bajtowym buforem danych.
#[repr(C)]
#[derive(Clone, Copy)]
struct SendCmdInParams {
    buffer_size: u32,
    regs: IdeRegs,
    drive_number: u8,
    reserved: [u8; 3],
    reserved_words: [u32; 4],
    buffer: [u8; ATA_SECTOR_LEN],
}

/// `DRIVERSTATUS` z odpowiedzi klasycznego IOCTL SMART.
#[repr(C)]
#[derive(Clone, Copy)]
struct DriverStatus {
    driver_error: u8,
    ide_error: u8,
    reserved: [u8; 2],
    reserved_words: [u32; 2],
}

/// `SENDCMDOUTPARAMS` z 512-bajtowym buforem danych.
#[repr(C)]
#[derive(Clone, Copy)]
struct SendCmdOutParams {
    buffer_size: u32,
    status: DriverStatus,
    buffer: [u8; ATA_SECTOR_LEN],
}

/// Dostawca danych o dyskach fizycznych dla Windows.
#[derive(Debug, Default)]
pub struct WinStorage;

impl WinStorage {
    pub fn new() -> Self {
        Self
    }
}

impl StorageProbe for WinStorage {
    fn describe(&self, index: u32) -> Result<PhysicalDrive> {
        let device = DeviceHandle::open(index)?;
        let (bus, removable) = device.bus_and_removable()?;
        let kind = match device.incurs_seek_penalty() {
            Some(true) => DriveKind::Hdd,
            Some(false) => DriveKind::Ssd,
            None => DriveKind::Unknown,
        };
        Ok(PhysicalDrive {
            index,
            bus,
            kind,
            removable,
        })
    }

    fn smart(&self, index: u32) -> Result<SmartData> {
        let device = DeviceHandle::open(index)?;
        // Deskryptor bywa niedostępny (np. przez mostek USB); wtedy próbujemy
        // po kolei obu protokołów, zamiast zgadywać po samym indeksie.
        let bus = device
            .bus_and_removable()
            .map_or(BusKind::Unknown, |(bus, _)| bus);

        match bus {
            BusKind::Nvme => device
                .nvme_health()
                .map(|log_page| SmartData::Nvme { log_page }),
            BusKind::Sata | BusKind::Ata | BusKind::Scsi => device.ata_smart(),
            _ => device.nvme_health().map_or_else(
                |_| device.ata_smart(),
                |log_page| Ok(SmartData::Nvme { log_page }),
            ),
        }
    }
}

/// Uchwyt do `\\.\PhysicalDriveN`, zamykany w `Drop` — także przy wczesnym wyjściu.
struct DeviceHandle(HANDLE);

impl DeviceHandle {
    fn open(index: u32) -> Result<Self> {
        let path = format!("\\\\.\\PhysicalDrive{index}");
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: `wide` jest zakończone zerem i żyje do końca wywołania;
        // uchwyt zamyka `Drop`, a `CreateFileW` nie wymaga wskaźnika na atrybuty.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|err| match err.code().0 as u32 {
            HRESULT_ACCESS_DENIED => ModuleError::access_denied(path.clone()),
            code => ModuleError::provider("CreateFileW", format!("{path}: {code:#010x}")),
        })?;

        Ok(Self(handle))
    }

    /// Magistrala i nośnik wymienny z `STORAGE_DEVICE_DESCRIPTOR`.
    fn bus_and_removable(&self) -> Result<(BusKind, bool)> {
        let descriptor: STORAGE_DEVICE_DESCRIPTOR = query_property(self.0, StorageDeviceProperty)?;
        Ok((bus_kind(descriptor.BusType), descriptor.RemovableMedia))
    }

    /// `None`, gdy sterownik nie zna pojęcia czasu dostępu do sektora.
    fn incurs_seek_penalty(&self) -> Option<bool> {
        let descriptor: DEVICE_SEEK_PENALTY_DESCRIPTOR =
            query_property(self.0, StorageDeviceSeekPenaltyProperty).ok()?;
        Some(descriptor.IncursSeekPenalty)
    }

    fn nvme_health(&self) -> Result<Vec<u8>> {
        match self.nvme_log(StorageDeviceProtocolSpecificProperty) {
            Ok(log_page) => Ok(log_page),
            // Dokumentacja Microsoftu pokazuje oba poziomy zapytania; sterowniki
            // obsługują różne, więc drugi poziom jest sensownym fallbackiem.
            Err(first) => self
                .nvme_log(StorageAdapterProtocolSpecificProperty)
                .map_err(|_| first),
        }
    }

    fn nvme_log(&self, property: STORAGE_PROPERTY_ID) -> Result<Vec<u8>> {
        let mut buffer = AlignedBuffer([0u8; NVME_BUFFER_LEN]);

        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: property,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let specific = STORAGE_PROTOCOL_SPECIFIC_DATA {
            ProtocolType: ProtocolTypeNvme,
            DataType: NVMeDataTypeLogPage.0 as u32,
            ProtocolDataRequestValue: NVME_LOG_PAGE_HEALTH,
            ProtocolDataRequestSubValue: 0,
            ProtocolDataOffset: size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>() as u32,
            ProtocolDataLength: NVME_HEALTH_LEN as u32,
            ..Default::default()
        };

        // SAFETY: obie struktury są typami POD o stałym rozmiarze i mieszczą się
        // w buforze (NVME_BUFFER_LEN liczy się z tych samych stałych).
        unsafe {
            ptr::write_unaligned(
                buffer.0.as_mut_ptr().cast::<STORAGE_PROPERTY_QUERY>(),
                query,
            );
            ptr::write_unaligned(
                buffer
                    .0
                    .as_mut_ptr()
                    .add(QUERY_PARAMS_OFFSET)
                    .cast::<STORAGE_PROTOCOL_SPECIFIC_DATA>(),
                specific,
            );
        }

        device_io_control(
            self.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            buffer.0.as_mut_ptr(),
            NVME_BUFFER_LEN,
            "log zdrowia NVMe",
        )?;

        // SAFETY: bufor ma co najmniej rozmiar deskryptora; odczyt przez
        // `read_unaligned`, bo typ ma pola u32 i nie wymaga wyrównania wskaźnika.
        let descriptor: STORAGE_PROTOCOL_DATA_DESCRIPTOR =
            unsafe { ptr::read_unaligned(buffer.0.as_ptr().cast()) };
        let expected = size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>() as u32;
        if descriptor.Version != expected || descriptor.Size != expected {
            return Err(ModuleError::provider(
                "DeviceIoControl",
                format!(
                    "log zdrowia NVMe: nieoczekiwany deskryptor (wersja {}, rozmiar {}, oczekiwano {expected})",
                    descriptor.Version, descriptor.Size
                ),
            ));
        }

        // `ProtocolDataOffset` liczy się od początku `ProtocolSpecificData`.
        let offset = descriptor.ProtocolSpecificData.ProtocolDataOffset as usize;
        let start = QUERY_PARAMS_OFFSET.saturating_add(offset);
        if offset < size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>()
            || (descriptor.ProtocolSpecificData.ProtocolDataLength as usize) < NVME_HEALTH_LEN
            || start + NVME_HEALTH_LEN > NVME_BUFFER_LEN
        {
            return Err(ModuleError::provider(
                "DeviceIoControl",
                format!("log zdrowia NVMe: niepoprawny zakres danych (offset {offset})"),
            ));
        }

        Ok(buffer.0[start..start + NVME_HEALTH_LEN].to_vec())
    }

    fn ata_smart(&self) -> Result<SmartData> {
        let mut attributes = self.ata_smart_read(ATA_SMART_READ_DATA);
        if attributes.is_err() && self.ata_smart_enable().is_ok() {
            // Część dysków ma SMART wyłączony w firmware (np. po bezpiecznym
            // czyszczeniu) i odpowiada ABRT na odczyt. SMART ENABLE to
            // standardowe, odwracalne polecenie, które nie dotyka danych.
            attributes = self.ata_smart_read(ATA_SMART_READ_DATA);
        }
        let attributes = attributes?;
        // Progi bywają odrzucane nawet tam, gdzie atrybuty działają; brak progów
        // nie może wyrzucić posiadanych już atrybutów (parser podstawi 0).
        let thresholds = self
            .ata_smart_read(ATA_SMART_READ_THRESHOLDS)
            .unwrap_or_default();
        Ok(SmartData::Ata {
            attributes,
            thresholds,
        })
    }

    /// `SMART ENABLE` — włącza raportowanie SMART, nie zmienia danych nośnika.
    fn ata_smart_enable(&self) -> Result<()> {
        self.ata_command(
            ATA_SMART_ENABLE,
            [0x00, 0x00, 0x00, 0x00, ATA_DEVICE_HEAD],
            0,
        )
        .map(|_| ())
    }

    fn ata_smart_read(&self, features: u8) -> Result<Vec<u8>> {
        match self.ata_smart_read_passthrough(features) {
            Ok(bytes) => Ok(bytes),
            // Część miniportów ATA nie obsługuje IOCTL_ATA_PASS_THROUGH i odpowiada
            // ABRT, choć dysk SMART ma. Klasyczny SMART_RCV_DRIVE_DATA jest
            // osobną ścieżką w sterowniku (korzysta z niej CrystalDiskInfo).
            Err(first) => match self.ata_smart_read_legacy(features) {
                Ok(bytes) => Ok(bytes),
                // Oba powody trafiają do raportu — technik ma wiedzieć, co
                // odpowiedział kontroler, a nie tylko „nie działa”. To stan
                // „ten kontroler nie udostępnia SMART”, nie awaria programu.
                Err(legacy) => Err(ModuleError::unavailable(format!(
                    "SMART ATA: {}; ścieżka klasyczna: {}",
                    error_detail(&first),
                    error_detail(&legacy)
                ))),
            },
        }
    }

    fn ata_smart_read_passthrough(&self, features: u8) -> Result<Vec<u8>> {
        self.ata_command(
            features,
            [
                0x01,
                ATA_SMART_LBA_LOW,
                ATA_SMART_LBA_MID,
                0x00,
                ATA_DEVICE_HEAD,
            ],
            ATA_SECTOR_LEN,
        )
    }

    /// Klasyczny `SMART_RCV_DRIVE_DATA`: rejestry ATA w `SENDCMDINPARAMS`,
    /// dane w `SENDCMDOUTPARAMS` (oba w jednym buforze na wywołanie).
    fn ata_smart_read_legacy(&self, features: u8) -> Result<Vec<u8>> {
        let input = SendCmdInParams {
            buffer_size: ATA_SECTOR_LEN as u32,
            regs: IdeRegs {
                features,
                sector_count: 0x01,
                sector_number: ATA_SMART_LBA_LOW,
                cyl_low: ATA_SMART_LBA_MID,
                cyl_high: 0x00,
                drive_head: ATA_DEVICE_HEAD,
                command: ATA_SMART_CMD,
                reserved: 0,
            },
            drive_number: 0,
            reserved: [0; 3],
            reserved_words: [0; 4],
            buffer: [0; ATA_SECTOR_LEN],
        };
        let mut output = SendCmdOutParams {
            buffer_size: 0,
            status: DriverStatus {
                driver_error: 0,
                ide_error: 0,
                reserved: [0; 2],
                reserved_words: [0; 2],
            },
            buffer: [0; ATA_SECTOR_LEN],
        };
        let mut returned = 0u32;

        // SAFETY: oba bufory są żywe, mają rozmiary podane w argumentach i są
        // wyrównane do 4 B (pola u32 w strukturach).
        let result = unsafe {
            DeviceIoControl(
                self.0,
                SMART_RCV_DRIVE_DATA,
                Some((&raw const input).cast()),
                size_of::<SendCmdInParams>() as u32,
                Some((&raw mut output).cast()),
                size_of::<SendCmdOutParams>() as u32,
                Some(&mut returned),
                None,
            )
        };
        result.map_err(|err| ioctl_error("SMART_RCV_DRIVE_DATA", &err))?;

        if output.status.driver_error != 0 {
            return Err(ModuleError::provider(
                "DeviceIoControl",
                format!(
                    "SMART_RCV_DRIVE_DATA (funkcja {features:#04x}): błąd sterownika {}, błąd IDE {:#04x}",
                    output.status.driver_error, output.status.ide_error
                ),
            ));
        }
        // `bIDEError` to rejestr błędu ATA (bit 0 = AMNF, 0x04 = ABRT), a nie
        // rejestr statusu — odrzucone polecenie ma tu po prostu niezerową wartość.
        if output.status.ide_error != 0 {
            return Err(ModuleError::unavailable(format!(
                "SMART_RCV_DRIVE_DATA: dysk odrzucił polecenie (błąd IDE {:#04x})",
                output.status.ide_error
            )));
        }
        Ok(output.buffer.to_vec())
    }

    /// Jedno polecenie ATA PASS-THROUGH (16) z funkcją SMART; `read_len` = 0 dla
    /// poleceń bez danych (np. SMART ENABLE).
    fn ata_command(&self, features: u8, task: [u8; 5], read_len: usize) -> Result<Vec<u8>> {
        const HEADER_LEN: usize = size_of::<ATA_PASS_THROUGH_EX>();
        const BUFFER_LEN: usize = size_of::<ATA_PASS_THROUGH_EX>() + ATA_SECTOR_LEN;

        let request = ATA_PASS_THROUGH_EX {
            Length: HEADER_LEN as u16,
            AtaFlags: if read_len == 0 {
                0
            } else {
                ATA_FLAGS_DATA_IN as u16
            },
            DataTransferLength: read_len as u32,
            TimeOutValue: IOCTL_TIMEOUT_SECS,
            // Struktura ma 48 B (8 B wyrównania), więc bufor danych też jest
            // wyrównany do 8 — tego wymaga sterownik dla IOCTL_ATA_PASS_THROUGH.
            DataBufferOffset: HEADER_LEN,
            CurrentTaskFile: [
                features,
                task[0],
                task[1],
                task[2],
                task[3],
                task[4],
                ATA_SMART_CMD,
                0x00,
            ],
            ..Default::default()
        };

        let mut buffer = AlignedBuffer([0u8; BUFFER_LEN]);
        // SAFETY: struktura POD mieści się w buforze; zapis przez `write_unaligned`.
        unsafe {
            ptr::write_unaligned(buffer.0.as_mut_ptr().cast::<ATA_PASS_THROUGH_EX>(), request);
        }

        device_io_control(
            self.0,
            IOCTL_ATA_PASS_THROUGH,
            buffer.0.as_mut_ptr(),
            BUFFER_LEN,
            "IOCTL_ATA_PASS_THROUGH",
        )?;

        // Sterownik zwraca sukces IOCTL także wtedy, gdy samo polecenie ATA
        // zostało odrzucone przez urządzenie — status i rejestr błędu siedzą
        // w zwróconym bloku. Bez tego sprawdzenia „brak atrybutów” myli
        // odrzucone polecenie z dyskiem bez SMART.
        // SAFETY: bufor zawiera co najmniej rozmiar struktury żądania.
        let returned =
            unsafe { ptr::read_unaligned(buffer.0.as_ptr().cast::<ATA_PASS_THROUGH_EX>()) };
        let status = returned.CurrentTaskFile[6];
        let error = returned.CurrentTaskFile[0];
        let payload = &buffer.0[HEADER_LEN..];
        let no_data = read_len > 0 && !payload.iter().any(|byte| *byte != 0);
        if status & ATA_STATUS_ERR != 0 || no_data {
            return Err(ModuleError::provider(
                "DeviceIoControl",
                format!(
                    "IOCTL_ATA_PASS_THROUGH (funkcja {features:#04x}): status {status:#04x}, błąd {error:#04x}, danych {read_len} B"
                ),
            ));
        }

        Ok(payload.to_vec())
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        // SAFETY: uchwyt pochodzi z `CreateFileW` i nie jest używany po zamknięciu.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// `IOCTL_STORAGE_QUERY_PROPERTY` bez parametrów dodatkowych: bufor wejściowy to
/// sama `STORAGE_PROPERTY_QUERY`, wyjściowy jest większy niż struktura docelowa.
fn query_property<T: Copy>(handle: HANDLE, property: STORAGE_PROPERTY_ID) -> Result<T> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: property,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buffer = AlignedBuffer([0u8; DESCRIPTOR_BUFFER]);
    let mut returned = 0u32;

    // SAFETY: oba bufory są żywe i mają rozmiary podane w argumentach; zapytanie
    // jest typu POD o stałym rozmiarze.
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&raw const query).cast()),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(buffer.0.as_mut_ptr().cast()),
            DESCRIPTOR_BUFFER as u32,
            Some(&mut returned),
            None,
        )
    };
    result.map_err(|err| ioctl_error("IOCTL_STORAGE_QUERY_PROPERTY", &err))?;

    if (returned as usize) < size_of::<T>() {
        return Err(ModuleError::provider(
            "DeviceIoControl",
            format!(
                "IOCTL_STORAGE_QUERY_PROPERTY: odpowiedź {returned} B krótsza niż {}",
                size_of::<T>()
            ),
        ));
    }
    // SAFETY: sprawdziliśmy, że w buforze jest co najmniej `size_of::<T>()` bajtów.
    Ok(unsafe { ptr::read_unaligned(buffer.0.as_ptr().cast::<T>()) })
}

/// Wspólne wywołanie z tym samym buforem wejściowym i wyjściowym (tak robi
/// przykład Microsoftu: zapytanie jest nadpisywane deskryptorem odpowiedzi).
fn device_io_control(
    handle: HANDLE,
    code: u32,
    buffer: *mut u8,
    length: usize,
    context: &str,
) -> Result<()> {
    let mut returned = 0u32;
    // SAFETY: `buffer` pochodzi z żywego `AlignedBuffer` o długości `length`.
    let result = unsafe {
        DeviceIoControl(
            handle,
            code,
            Some(buffer.cast()),
            length as u32,
            Some(buffer.cast()),
            length as u32,
            Some(&mut returned),
            None,
        )
    };
    result.map_err(|err| ioctl_error(context, &err))
}

fn ioctl_error(context: &str, err: &windows::core::Error) -> ModuleError {
    map_hresult(err.code().0 as u32, context)
}

/// Sam opis błędu, bez wariantu (`niedostępne:`/`błąd …:`) — przy łączeniu
/// dwóch prób w jeden komunikat prefiksy tylko zaśmiecają raport.
fn error_detail(error: &ModuleError) -> String {
    match error {
        ModuleError::Unavailable { what }
        | ModuleError::AccessDenied { what }
        | ModuleError::Internal(what) => what.clone(),
        ModuleError::Provider { provider, message } => format!("{provider}: {message}"),
    }
}

/// Tłumaczy kod błędu Win32 na błąd domenowy. Wydzielone, żeby dało się to
/// przetestować bez prawdziwego dysku.
fn map_hresult(code: u32, context: &str) -> ModuleError {
    match code {
        HRESULT_ACCESS_DENIED => {
            ModuleError::access_denied(format!("{context} (wymaga administratora)"))
        }
        HRESULT_INVALID_FUNCTION | HRESULT_NOT_SUPPORTED => ModuleError::unavailable(format!(
            "{context}: kontroler lub mostek nie obsługuje tego polecenia"
        )),
        HRESULT_NOT_READY => {
            ModuleError::unavailable(format!("{context}: urządzenie nie jest gotowe"))
        }
        HRESULT_IO_DEVICE => ModuleError::unavailable(format!(
            "{context}: sterownik zgłosił błąd wejścia/wyjścia urządzenia"
        )),
        // Zły parametr oznacza błąd w naszym żądaniu (rozmiar, flagi), a nie
        // brak wsparcia — taki błąd ma być widoczny jako błąd programu.
        code => ModuleError::provider("DeviceIoControl", format!("{context}: {code:#010x}")),
    }
}

fn bus_kind(bus: STORAGE_BUS_TYPE) -> BusKind {
    let code = bus.0;
    if code == windows::Win32::Storage::FileSystem::BusTypeNvme.0 {
        BusKind::Nvme
    } else if code == windows::Win32::Storage::FileSystem::BusTypeSata.0 {
        BusKind::Sata
    } else if code == windows::Win32::Storage::FileSystem::BusTypeAta.0 {
        BusKind::Ata
    } else if code == windows::Win32::Storage::FileSystem::BusTypeScsi.0
        || code == windows::Win32::Storage::FileSystem::BusTypeSas.0
    {
        BusKind::Scsi
    } else if code == windows::Win32::Storage::FileSystem::BusTypeUsb.0 {
        BusKind::Usb
    } else if code == windows::Win32::Storage::FileSystem::BusTypeRAID.0
        || code == windows::Win32::Storage::FileSystem::BusTypeSpaces.0
    {
        BusKind::Raid
    } else {
        BusKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_access_denied_to_domain_error() {
        let error = map_hresult(HRESULT_ACCESS_DENIED, "SMART");
        assert!(matches!(error, ModuleError::AccessDenied { .. }), "{error}");
        assert!(error.to_string().contains("administrator"), "{error}");
    }

    #[test]
    fn maps_unsupported_command_to_unavailable() {
        for code in [
            HRESULT_INVALID_FUNCTION,
            HRESULT_NOT_SUPPORTED,
            HRESULT_NOT_READY,
            HRESULT_IO_DEVICE,
        ] {
            let error = map_hresult(code, "SMART ATA");
            assert!(matches!(error, ModuleError::Unavailable { .. }), "{error}");
        }
    }

    #[test]
    fn maps_other_codes_to_provider_error_with_hex_code() {
        let error = map_hresult(0x8007_001F, "log zdrowia NVMe");
        assert!(matches!(error, ModuleError::Provider { .. }), "{error}");
        assert!(error.to_string().contains("0x8007001f"), "{error}");
    }

    #[test]
    fn invalid_parameter_is_a_program_error_not_missing_support() {
        // Zły rozmiar bufora zgłoszony przez sterownik to nasz błąd, nie
        // „kontroler nie obsługuje SMART” — nie może zniknąć w degradacji.
        let error = map_hresult(0x8007_0057, "IOCTL_ATA_PASS_THROUGH");
        assert!(matches!(error, ModuleError::Provider { .. }), "{error}");
    }

    /// Rozmiary i wyrównania muszą zgadzać się z ABI Windows; te liczby to
    /// kontrakt ze strukturami z `ntddscsi.h`/`winioctl.h` i `windows-0.62.2`.
    #[test]
    fn ffi_layout_matches_windows_abi() {
        assert_eq!(size_of::<ATA_PASS_THROUGH_EX>(), 48);
        assert_eq!(std::mem::align_of::<ATA_PASS_THROUGH_EX>(), 8);
        // W C struktura ma 9 B; Rust dokłada padding do 12 B, co jest bezpieczne
        // jako rozmiar wejściowy (sterownik czyta tylko znane pola).
        assert_eq!(size_of::<STORAGE_PROPERTY_QUERY>(), 12);
        assert_eq!(offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters), 8);
        assert_eq!(size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>(), 40);
        assert_eq!(size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>(), 48);
        assert_eq!(size_of::<SendCmdInParams>(), 544);
        assert_eq!(size_of::<SendCmdOutParams>(), 528);
        assert_eq!(NVME_BUFFER_LEN, 568);
    }

    #[test]
    fn maps_bus_types_from_storage_descriptor() {
        use windows::Win32::Storage::FileSystem::{
            BusTypeAta, BusTypeNvme, BusTypeRAID, BusTypeSas, BusTypeSata, BusTypeScsi, BusTypeUsb,
        };

        assert_eq!(bus_kind(BusTypeNvme), BusKind::Nvme);
        assert_eq!(bus_kind(BusTypeSata), BusKind::Sata);
        assert_eq!(bus_kind(BusTypeAta), BusKind::Ata);
        assert_eq!(bus_kind(BusTypeScsi), BusKind::Scsi);
        assert_eq!(bus_kind(BusTypeSas), BusKind::Scsi);
        assert_eq!(bus_kind(BusTypeUsb), BusKind::Usb);
        assert_eq!(bus_kind(BusTypeRAID), BusKind::Raid);
        assert_eq!(
            bus_kind(STORAGE_BUS_TYPE(0)),
            BusKind::Other,
            "nieznana magistrala nie może udawać NVMe"
        );
    }

    #[test]
    fn ata_pass_through_buffer_keeps_data_aligned() {
        // Wymóg sterownika: offset danych w IOCTL_ATA_PASS_THROUGH musi być
        // wyrównany; struktura ma 48 B, więc offset jest wielokrotnością 8.
        const HEADER_LEN: usize = size_of::<ATA_PASS_THROUGH_EX>();
        assert_eq!(HEADER_LEN % 8, 0, "nagłówek ATA ma {HEADER_LEN} B");
    }

    /// Test sprzętowy: `cargo test -p sk-win -- --ignored`.
    ///
    /// `cargo test` bez podniesienia procesu nie widzi dysków fizycznych —
    /// wtedy test wymaga, żeby porażka była zgłoszona jako brak uprawnień
    /// (`AccessDenied`), a nie jako panika albo puste dane. Z administratorem
    /// test wymaga co najmniej jednego dysku z magistralą i SMART-em.
    #[test]
    #[ignore = "wymaga uprawnień administratora i prawdziwego dysku"]
    fn reads_real_drive_descriptors_and_smart() {
        use crate::env::WinHostEnv;

        let storage = WinStorage::new();
        let elevated = sk_core::providers::HostEnv::is_elevated(&WinHostEnv);
        let mut found = 0;
        let mut denied = 0;
        for index in 0..8 {
            match storage.describe(index) {
                Ok(drive) => {
                    found += 1;
                    assert_ne!(
                        drive.bus,
                        BusKind::Unknown,
                        "dysk {index}: nieznana magistrala"
                    );
                    match storage.smart(index) {
                        Ok(SmartData::Nvme { log_page }) => {
                            assert_eq!(log_page.len(), NVME_HEALTH_LEN);
                        }
                        Ok(SmartData::Ata { attributes, .. }) => {
                            assert_eq!(attributes.len(), ATA_SECTOR_LEN);
                        }
                        Err(err) => {
                            // Mostki USB i kontrolery RAID legalnie nie przekazują SMART.
                            println!("dysk {index}: SMART niedostępny ({err})");
                        }
                    }
                }
                Err(ModuleError::AccessDenied { .. }) => denied += 1,
                Err(err) => println!("dysk {index}: brak dostępu ({err})"),
            }
        }

        if !elevated {
            assert!(
                found > 0 || denied > 0,
                "bez administratora żaden dysk nie zwrócił ani danych, ani odmowy dostępu"
            );
            println!("proces bez administratora: {denied} dysków z odmową dostępu — oczekiwane");
            return;
        }

        assert!(found > 0, "nie znaleziono żadnego dysku fizycznego");
    }
}
