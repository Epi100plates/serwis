# Zrzut okna przez DWM (PrintWindow z PW_RENDERFULLCONTENT). Na tej maszynie
# przechwytuje rowniez tresc renderowana przez wgpu, wiec dziala takze wtedy, gdy
# okno jest zasloniete przez pelnoekranowa aplikacje. Opcjonalnie klika w okno
# (wspolrzedne obrazu z poprzedniego zrzutu) — bez ruszania kursora uzytkownika.
#
# Przyklad: wybierz modul "Czujniki" i odczekaj na probki wykresu:
#   powershell -ExecutionPolicy Bypass -File tools\screenshot-window.ps1 `
#       -Out docs\screenshot-sensors-cpu.png -Pid_ 1234 -ClickX 60 -ClickY 255 -ClickDelay 25
param(
    [string]$Out = "docs\screenshot.png",
    [int]$Pid_ = 0,
    [int]$ClickX = -1,
    [int]$ClickY = -1,
    [int]$ClickDelay = 0
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT2 { public int Left, Top, Right, Bottom; }
public struct POINT2 { public int X, Y; }
public class WinPrint {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT2 r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT2 p);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT2 r);
    [DllImport("user32.dll")] public static extern IntPtr PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
"@

$proc = Get-Process -Id $Pid_
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) { throw "brak okna (pid=$Pid_)" }

$window = New-Object RECT2
[WinPrint]::GetWindowRect($hwnd, [ref]$window) | Out-Null
$client = New-Object RECT2
[WinPrint]::GetClientRect($hwnd, [ref]$client) | Out-Null
$origin = New-Object POINT2
[WinPrint]::ClientToScreen($hwnd, [ref]$origin) | Out-Null

# Zrzut jest obrazem calego okna, wiec wspolrzedne kliknięcia trzeba przesunac
# o ramke i pasek tytulu. WM_LBUTTONDOWN/UP przyjmuja wspolrzedne klienta.
$offsetX = $origin.X - $window.Left
$offsetY = $origin.Y - $window.Top

if ($ClickX -ge 0 -and $ClickY -ge 0) {
    $cx = $ClickX - $offsetX
    $cy = $ClickY - $offsetY
    $lParam = [IntPtr](($cy -shl 16) -bor ($cx -band 0xFFFF))
    [WinPrint]::PostMessage($hwnd, 0x0200, [IntPtr]::Zero, $lParam) | Out-Null  # WM_MOUSEMOVE
    [WinPrint]::PostMessage($hwnd, 0x0201, [IntPtr]1, $lParam) | Out-Null       # WM_LBUTTONDOWN
    [WinPrint]::PostMessage($hwnd, 0x0202, [IntPtr]0, $lParam) | Out-Null       # WM_LBUTTONUP
    if ($ClickDelay -gt 0) {
        Start-Sleep -Seconds $ClickDelay
    }
}

$w = $window.Right - $window.Left
$h = $window.Bottom - $window.Top
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [WinPrint]::PrintWindow($hwnd, $hdc, 2)   # PW_RENDERFULLCONTENT
$g.ReleaseHdc($hdc)
$g.Dispose()
$path = [System.IO.Path]::Combine((Get-Location).Path, $Out)
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "printwindow=$ok client=${($client.Right)}x${($client.Bottom)} offset=$offsetX,$offsetY saved=$path"
