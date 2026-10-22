param([string]$ExecutableDirectory, [string]$ShortcutPath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
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
function Assert-BrandIcon([string]$Path) {
    $info = New-Object VEditShellIconTest+FileInfo
    $result = [VEditShellIconTest]::SHGetFileInfo($Path, 0, [ref]$info, [Runtime.InteropServices.Marshal]::SizeOf($info), 0x100)
    Assert ($result -ne [IntPtr]::Zero -and $info.Icon -ne [IntPtr]::Zero) "Windows could not resolve the icon: $Path"
    try {
        $icon = [Drawing.Icon]::FromHandle($info.Icon)
        $bitmap = $icon.ToBitmap()
        try {
            $accent = 0
            for ($y = 0; $y -lt $bitmap.Height; $y++) {
                for ($x = 0; $x -lt $bitmap.Width; $x++) {
                    $pixel = $bitmap.GetPixel($x, $y)
                    if ($pixel.R -gt 180 -and $pixel.G -gt 75 -and $pixel.G -lt 175 -and $pixel.B -lt 60) { $accent++ }
                }
            }
            Assert ($accent -gt 10) "Windows returned a generic or incorrect icon: $Path"
        } finally { $bitmap.Dispose(); $icon.Dispose() }
    } finally { [VEditShellIconTest]::DestroyIcon($info.Icon) | Out-Null }
}
$temporaryShortcut = $null
try {
    foreach ($name in @('vedit.exe', 'vedit-cli.exe')) {
        $path = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory $name))
        $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($path)
        Assert ($version.ProductName -eq 'vEdit') "Missing vEdit product metadata: $name"
        Assert ($version.FileDescription -eq 'vEdit text and code editor') "Missing file description: $name"
        Assert ($version.FileVersion -match '^\d+\.\d+\.\d+') "Missing file version: $name"
        Assert ([VEditShellIconTest]::ExtractIconEx($path, -1, [IntPtr]::Zero, [IntPtr]::Zero, 0) -gt 0) "Missing embedded icon: $name"
        Assert-BrandIcon $path
    }
    if (-not $ShortcutPath) {
        $temporaryShortcut = Join-Path ([IO.Path]::GetTempPath()) ('vedit-icon-test-' + [Guid]::NewGuid().ToString('N') + '.lnk')
        $ShortcutPath = $temporaryShortcut
        $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath)
        $shortcut.TargetPath = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory 'vedit.exe'))
        $shortcut.IconLocation = $shortcut.TargetPath + ',0'
        $shortcut.Save()
    }
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath)
    Assert ($shortcut.IconLocation -eq ($shortcut.TargetPath + ',0')) 'The shortcut stores quotes or an external icon dependency.'
    Assert-BrandIcon $ShortcutPath
    Write-Output "$checks Windows resource and Shell icon checks passed."
} finally {
    if ($temporaryShortcut) { Remove-Item -LiteralPath $temporaryShortcut -ErrorAction SilentlyContinue }
}
