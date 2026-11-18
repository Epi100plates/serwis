//! Minimalny odczyt rejestru Windows — tylko to, czego WMI nie daje
//! (pamięć dedykowana GPU, EDID monitora).
//!
//! Brak wartości nie jest błędem: na maszynie klienta może nie być wpisu,
//! a moduł ma wtedy pokazać degradację, nie wywalić raportu.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_VALUE_TYPE, RRF_RT_REG_BINARY, RRF_RT_REG_QWORD,
    RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::core::{PCWSTR, PWSTR};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Odczyt wartości typu `REG_QWORD` (np. `HardwareInformation.qwMemorySize`).
pub fn read_qword(path: &str, name: &str) -> Option<u64> {
    let subkey = wide(path);
    let value_name = wide(name);
    let mut data = 0u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    let mut kind = REG_VALUE_TYPE::default();

    // SAFETY: oba łańcuchy (`subkey`, `value_name`) są zakończone zerem i żyją
    // do końca wywołania; `data`/`size` opisują prawdziwy bufor wyjściowy.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_QWORD,
            Some(&mut kind),
            Some(std::ptr::from_mut(&mut data).cast()),
            Some(&mut size),
        )
    };

    (status == ERROR_SUCCESS).then_some(data)
}

/// Odczyt wartości typu `REG_SZ` (np. `DriverDesc`).
pub fn read_string(path: &str, name: &str) -> Option<String> {
    let subkey = wide(path);
    let value_name = wide(name);
    let mut size = 0u32;

    // SAFETY: wywołanie tylko z rozmiarem — brak bufora wyjściowego.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }

    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: bufor ma rozmiar podany w `size` (w bajtach) i żyje do końca wywołania.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end]).ok()
}

/// Odczyt wartości typu `REG_BINARY` (np. blok EDID monitora).
pub fn read_binary(path: &str, name: &str) -> Option<Vec<u8>> {
    let subkey = wide(path);
    let value_name = wide(name);
    let mut size = 0u32;

    // SAFETY: wywołanie tylko z rozmiarem — brak bufora wyjściowego.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }

    let mut buffer = vec![0u8; size as usize];
    // SAFETY: bufor ma rozmiar `size` bajtów i żyje do końca wywołania.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            PCWSTR::from_raw(value_name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    buffer.truncate(size as usize);
    Some(buffer)
}

/// Nazwy podkluczy danego klucza (bez rekurencji).
pub fn subkeys(path: &str) -> Vec<String> {
    let subkey = wide(path);
    let mut key = HKEY::default();

    // SAFETY: `subkey` jest zakończonym zerem łańcuchem, a `key` dostaje uchwyt
    // do zamknięcia w tym samym zakresie.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }

    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let mut buffer = [0u16; 256];
        let mut length = buffer.len() as u32;
        // SAFETY: `buffer`/`length` opisują prawdziwy bufor wyjściowy;
        // system zapisuje w nim nazwę podklucza.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR::from_raw(buffer.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        if let Ok(name) = String::from_utf16(&buffer[..length as usize]) {
            names.push(name);
        }
        index += 1;
    }

    // SAFETY: `key` pochodzi z udanego RegOpenKeyExW i nie jest używany dalej.
    unsafe {
        let _ = RegCloseKey(key);
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_appends_nul() {
        let buffer = wide("A");
        assert_eq!(buffer, vec![b'A' as u16, 0]);
    }

    #[test]
    fn missing_value_returns_none_instead_of_panicking() {
        // Klucz, którego pewnie nie ma — sprawdzamy tylko brak paniki i None.
        assert!(read_qword("SOFTWARE\\SerwisKit\\NieMa", "Brak").is_none());
        assert!(read_string("SOFTWARE\\SerwisKit\\NieMa", "Brak").is_none());
        assert!(read_binary("SOFTWARE\\SerwisKit\\NieMa", "Brak").is_none());
        assert!(subkeys("SOFTWARE\\SerwisKit\\NieMa").is_empty());
    }
}
