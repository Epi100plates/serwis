# Zbiera odpowiedzi WMI uzywane przez moduly `hw.*` do `docs/fixtures/wmi/*.json`.
#
# Po co: testy modulow nie moga zalezec od sprzetu ani od uprawnien administratora.
# Fixture to prawdziwa odpowiedz WMI z maszyny serwisowej, po anonimizacji.
#
# Uzycie:
#   powershell -ExecutionPolicy Bypass -File tools\collect-wmi-fixtures.ps1
#   powershell -ExecutionPolicy Bypass -File tools\collect-wmi-fixtures.ps1 -OutDir C:\tmp\wmi
#
# Zasady:
#   - plik = jedna klasa, zawartosc = tablica instancji (nawet gdy instancja jest jedna),
#   - usuwane sa artefakty PowerShell/CIM (`CimClass`, `PSComputerName`, zagniezdzone
#     obiekty CIM), bo klient WMI w Rust ich nie zwraca,
#   - seriale, UUID, nazwa komputera i wlasciciel sa zastepowane stala wartoscia
#     (`SN-REDACTED`, `PC-TESTOWY`, ...) - fixtures nie moga zawierac danych osobowych,
#   - brak klasy (np. `Win32_Battery` na desktopie) zapisuje pusta tablice
#     i wypisuje ostrzezenie.
#
# Uwaga: ten plik jest celowo **ASCII-only**. Windows PowerShell 5.1 czyta `.ps1`
# bez BOM jako ANSI (CP1250); polskie znaki i myslniki z UTF-8 lamia wtedy parser.
# Opisy komend zostaja po polsku, ale bez diakrytykow.

[CmdletBinding()]
param(
    [string] $OutDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 nie zna `-Encoding utf8NoBOM`, a BOM psuje
# `serde_json` w Rust - dlatego zapisujemy przez jawne UTF8Encoding($false).
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

if ([string]::IsNullOrEmpty($OutDir)) {
    $scriptDir = $PSScriptRoot
    if ([string]::IsNullOrEmpty($scriptDir)) {
        $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
    }
    $OutDir = Join-Path (Split-Path -Parent $scriptDir) 'docs\fixtures\wmi'
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Content
    )
    [System.IO.File]::WriteAllText($Path, $Content + [Environment]::NewLine, $utf8NoBom)
}

$targets = @(
    @{ File = 'win32_processor.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_Processor' }
    @{ File = 'win32_physicalmemory.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_PhysicalMemory' }
    @{ File = 'win32_physicalmemoryarray.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_PhysicalMemoryArray' }
    @{ File = 'win32_baseboard.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_BaseBoard' }
    @{ File = 'win32_bios.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_BIOS' }
    @{ File = 'win32_videocontroller.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_VideoController' }
    @{ File = 'win32_diskdrive.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_DiskDrive' }
    @{ File = 'win32_computersystem.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_ComputerSystem' }
    @{ File = 'win32_operatingsystem.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_OperatingSystem' }
    @{ File = 'win32_perfformatteddata_perfos_processor.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_PerfFormattedData_PerfOS_Processor' }
    @{ File = 'win32_battery.json'; Namespace = 'ROOT\CIMV2'; Class = 'Win32_Battery' }
    @{ File = 'wmi_wmimonitorid.json'; Namespace = 'ROOT\WMI'; Class = 'WmiMonitorID' }
    @{ File = 'wmi_wmimonitorbasicdisplayparams.json'; Namespace = 'ROOT\WMI'; Class = 'WmiMonitorBasicDisplayParams' }
    @{ File = 'wmi_processorperformance.json'; Namespace = 'ROOT\WMI'; Class = 'ProcessorPerformance' }
    @{ File = 'wmi_batterystaticdata.json'; Namespace = 'ROOT\WMI'; Class = 'BatteryStaticData' }
    @{ File = 'wmi_batteryfullchargedcapacity.json'; Namespace = 'ROOT\WMI'; Class = 'BatteryFullChargedCapacity' }
    @{ File = 'wmi_batterystatus.json'; Namespace = 'ROOT\WMI'; Class = 'BatteryStatus' }
)

# Artefakty PowerShell - klient WMI w Rust ich nie zwraca.
$dropProperties = @(
    'CimClass', 'CimInstanceProperties', 'CimSystemProperties', 'PSComputerName',
    '__PATH', '__CLASS', '__RELPATH', '__NAMESPACE', '__SERVER'
)

# Wartosci zastepcze wspolne dla wszystkich klas.
$scrubCommon = @{
    SerialNumber     = 'SN-REDACTED'
    UUID             = '{00000000-0000-0000-0000-000000000000}'
    ProcessorId      = 'REDACTED'
    Signature        = 'REDACTED'
    # Nazwa komputera i wlasciciel wystepuja w kilku klasach.
    SystemName       = 'PC-TESTOWY'
    CSName           = 'PC-TESTOWY'
    RegisteredUser   = 'Technik serwisowy'
    PrimaryOwnerName = 'Technik serwisowy'
}

# Wartosci zastepcze dla konkretnych klas.
$scrubPerClass = @{
    'Win32_ComputerSystem' = @{
        Name        = 'PC-TESTOWY'
        Caption     = 'PC-TESTOWY'
        DNSHostName = 'pc-testowy'
        UserName    = 'SERWIS\technik'
    }
    'WmiMonitorID'         = @{
        # SerialNumberID to tablica kodow UTF-16 - zamieniamy na "0000".
        SerialNumberID = @(48, 48, 48, 48)
    }
}

# Zagniezdzone obiekty CIM (np. `SupportedDisplayFeatures`) nie serializuja sie
# sensownie i nie sa potrzebne - bierzemy tylko skalary i tablice skalarow.
function Test-ScalarValue {
    param($Value)

    if ($null -eq $Value) { return $true }
    if ($Value -is [string]) { return $true }
    if ($Value -is [ValueType]) { return $true }
    if ($Value -is [System.Array]) {
        foreach ($item in $Value) {
            if (-not (Test-ScalarValue -Value $item)) { return $false }
        }
        return $true
    }
    return $false
}

function Convert-Instance {
    param(
        [Parameter(Mandatory)] $Instance,
        [Parameter(Mandatory)][string] $ClassName
    )

    $ordered = [ordered]@{}
    foreach ($property in $Instance.PSObject.Properties) {
        if ($dropProperties -contains $property.Name) { continue }
        if (-not (Test-ScalarValue -Value $property.Value)) { continue }

        $value = $property.Value
        if ($scrubCommon.ContainsKey($property.Name)) {
            $value = $scrubCommon[$property.Name]
        }
        if ($scrubPerClass.ContainsKey($ClassName) -and $scrubPerClass[$ClassName].ContainsKey($property.Name)) {
            $value = $scrubPerClass[$ClassName][$property.Name]
        }
        if ($value -is [datetime]) {
            $value = $value.ToString('yyyy-MM-ddTHH:mm:sszzz')
        }

        $ordered[$property.Name] = $value
    }
    return $ordered
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

foreach ($target in $targets) {
    $path = Join-Path $OutDir $target.File
    try {
        $instances = @(Get-CimInstance -Namespace $target.Namespace -ClassName $target.Class -ErrorAction Stop)
    }
    catch {
        Write-Warning "$($target.Class) niedostepne: $($_.Exception.Message)"
        Write-JsonFile -Path $path -Content '[]'
        continue
    }

    if ($instances.Count -eq 0) {
        Write-Warning "$($target.Class): brak instancji na tej maszynie - zapisuje pusta tablice"
        Write-JsonFile -Path $path -Content '[]'
        continue
    }

    $rows = @(foreach ($instance in $instances) { Convert-Instance -Instance $instance -ClassName $target.Class })
    $json = ConvertTo-Json -InputObject $rows -Depth 4
    Write-JsonFile -Path $path -Content $json
    Write-Host ("{0,-48} {1} instancji" -f $target.File, $rows.Count)
}

Write-Host "Fixtures zapisane w $OutDir"
