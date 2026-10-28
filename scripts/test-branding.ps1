param([string]$ExecutableDirectory, [string]$ShortcutPath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
$projectTemp = Enter-ProjectTemp
try {
if (-not $ExecutableDirectory) { $ExecutableDirectory = Join-Path $PSScriptRoot '../target/debug' }
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class VEditShellIconTest {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct FileInfo {
        public IntPtr Icon; public int Index; public uint Attributes;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string DisplayName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 80)] public string TypeName;
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconEx(string path, int index, IntPtr large, IntPtr small, uint count);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr SHGetFileInfo(string path, uint attributes, out FileInfo info, uint size, uint flags);
    [DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr icon);
}
'@
$checks = 0
function Assert($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
function Get-ShellBitmap([string]$Path) {
    $info = New-Object VEditShellIconTest+FileInfo
    $result = [VEditShellIconTest]::SHGetFileInfo($Path, 0, [ref]$info, [Runtime.InteropServices.Marshal]::SizeOf($info), 0x100)
    Assert ($result -ne [IntPtr]::Zero -and $info.Icon -ne [IntPtr]::Zero) "Windows could not resolve the icon: $Path"
    try {
        $icon = [Drawing.Icon]::FromHandle($info.Icon)
        try { return $icon.ToBitmap() } finally { $icon.Dispose() }
    } finally { [VEditShellIconTest]::DestroyIcon($info.Icon) | Out-Null }
}
function Assert-BrandIcon([string]$Path, [Drawing.Bitmap]$Expected) {
    $bitmap = Get-ShellBitmap $Path
    try {
        Assert ($bitmap.Size -eq $Expected.Size) "Shell icon dimensions differ from the artwork: $Path"
        $different = 0
        $compared = 0
        for ($y = 0; $y -lt $bitmap.Height; $y++) {
            for ($x = 0; $x -lt $bitmap.Width; $x++) {
                # Windows may place its shortcut arrow over the lower-left corner.
                if ($Path.EndsWith('.lnk') -and $x -lt $bitmap.Width / 2 -and $y -ge $bitmap.Height / 2) { continue }
                $pixel = $bitmap.GetPixel($x, $y)
                $reference = $Expected.GetPixel($x, $y)
                if ([Math]::Abs([int]$pixel.R - $reference.R) -gt 8 -or
                    [Math]::Abs([int]$pixel.G - $reference.G) -gt 8 -or
                    [Math]::Abs([int]$pixel.B - $reference.B) -gt 8 -or
                    [Math]::Abs([int]$pixel.A - $reference.A) -gt 8) { $different++ }
                $compared++
            }
        }
        Assert ($different -le $compared * 0.02) "Windows returned an icon that differs from the bundled artwork: $Path ($different/$compared pixels)"
    } finally { $bitmap.Dispose() }
}
$temporaryShortcut = $null
$expected = Get-ShellBitmap ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../assets/vedit.ico')))
try {
    foreach ($name in @('vedit.exe', 'vedit-cli.exe')) {
        $path = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory $name))
        $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($path)
        Assert ($version.ProductName -eq 'vEdit') "Missing vEdit product metadata: $name"
        Assert ($version.FileDescription -eq 'vEdit text and code editor') "Missing file description: $name"
        Assert ($version.FileVersion -match '^\d+\.\d+\.\d+') "Missing file version: $name"
        Assert ([VEditShellIconTest]::ExtractIconEx($path, -1, [IntPtr]::Zero, [IntPtr]::Zero, 0) -gt 0) "Missing embedded icon: $name"
        Assert-BrandIcon $path $expected
    }
    if (-not $ShortcutPath) {
        $temporaryShortcut = Join-Path $projectTemp.Root ('vedit-icon-test-' + [Guid]::NewGuid().ToString('N') + '.lnk')
        $ShortcutPath = $temporaryShortcut
        $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath)
        $shortcut.TargetPath = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory 'vedit.exe'))
        $shortcut.IconLocation = $shortcut.TargetPath + ',0'
        $shortcut.Save()
    }
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath)
    Assert ($shortcut.IconLocation -eq ($shortcut.TargetPath + ',0')) 'The shortcut stores quotes or an external icon dependency.'
    Assert-BrandIcon $ShortcutPath $expected
    Write-Output "$checks Windows resource and Shell icon checks passed."
} finally {
    $expected.Dispose()
    if ($temporaryShortcut) { Remove-Item -LiteralPath $temporaryShortcut -ErrorAction SilentlyContinue }
}
} finally {
    Exit-ProjectTemp $projectTemp
}
