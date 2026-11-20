# Zrzut okna GUI SerwisKit do pliku — zrzuty do dokumentacji (docs/*.png).
#
# Uruchomienie:
#   powershell -ExecutionPolicy Bypass -File tools\screenshot-gui.ps1 -Out docs\shot.png `
#       -ClickX 40 -ClickY 200 -ClickDelay 20
#
# Dlaczego tak: `PrintWindow` nie przechwytuje treści renderowanej przez wgpu
# (dostaje się biały obraz), a `SetForegroundWindow` bywa blokowane przez okno
# pełnoekranowe innej aplikacji — dlatego okno jest podnoszone na wierzch przez
# `SetWindowPos(HWND_TOPMOST)` i przenoszone w ustalone miejsce. Skrypt jest
# ASCII-only: PowerShell 5.1 bez BOM czyta plik jako CP1250 i psuje parser.
param(
    [string]$Out = "docs\screenshot.png",
    [string]$Exe = "target\release\serwiskit.exe",
    [int]$X = 60,
    [int]$Y = 40,
    [int]$Width = 1400,
    [int]$Height = 900,
    [int]$Pid_ = 0,
    # Opcjonalne kliknięcie w oknie (współrzędne okna) — np. wybór modułu.
    [int]$ClickX = -1,
    [int]$ClickY = -1,
    # Ile sekund czekać po kliknięciu, zanim powstanie zrzut (wykres potrzebuje próbek).
    [int]$ClickDelay = 0
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public class Win {
    [DllImport("user32.dll")] public static extern bool SetWindowPos(
        IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
"@

if ($Pid_ -eq 0) {
    $proc = Start-Process -FilePath $Exe -PassThru
    $Pid_ = $proc.Id
}
$proc = Get-Process -Id $Pid_
$hwnd = $proc.MainWindowHandle
for ($i = 0; $i -lt 30 -and $hwnd -eq [IntPtr]::Zero; $i++) {
    Start-Sleep -Milliseconds 300
    $proc.Refresh()
    $hwnd = $proc.MainWindowHandle
}
if ($hwnd -eq [IntPtr]::Zero) {
    throw "okno GUI nie powstalo (pid=$Pid_)"
}

# SW_RESTORE = 9, HWND_TOPMOST = -1, SWP_SHOWWINDOW = 0x40
[Win]::ShowWindow($hwnd, 9) | Out-Null
[Win]::SetWindowPos($hwnd, [IntPtr](-1), $X, $Y, $Width, $Height, 0x40) | Out-Null
[Win]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 1200

if ($ClickX -ge 0 -and $ClickY -ge 0) {
    $rect = New-Object RECT
    [Win]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    [Win]::SetCursorPos($rect.Left + $ClickX, $rect.Top + $ClickY) | Out-Null
    Start-Sleep -Milliseconds 200
    # LEFTDOWN | LEFTUP
    [Win]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
    [Win]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
    if ($ClickDelay -gt 0) {
        Start-Sleep -Seconds $ClickDelay
    }
}

$rect = New-Object RECT
[Win]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$bw = $rect.Right - $rect.Left
$bh = $rect.Bottom - $rect.Top
Write-Output "pid=$Pid_ rect=$($rect.Left),$($rect.Top),$($bw)x$($bh)"

$bmp = New-Object System.Drawing.Bitmap $bw, $bh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
$g.Dispose()
$path = [System.IO.Path]::Combine((Get-Location).Path, $Out)
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved=$path"
