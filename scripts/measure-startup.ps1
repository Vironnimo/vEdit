[CmdletBinding()]
param(
    [string]$ExecutableDirectory = 'target/release',
    [ValidateRange(1, 20)][int]$Runs = 5
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
$projectTemp = Enter-ProjectTemp
$previous = @{}
foreach ($name in @('VEDIT_PROFILE_DIR', 'VEDIT_CONTROL_DIR', 'MORROW_CONTROL_DIR')) {
    $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $executable = (Resolve-Path -LiteralPath (Join-Path $ExecutableDirectory 'vedit.exe')).Path
    $fixture = [IO.Path]::GetFullPath((Join-Path $projectTemp.Root ('startup-measurement/' + [Guid]::NewGuid().ToString('N'))))
    New-Item -ItemType Directory -Path $fixture -Force | Out-Null
    if (-not ('VEditStartupMeasure' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public class VEditStartupSample {
    public Process Process;
    public Stopwatch Clock;
    public double VisibleMs = -1;
}
public static class VEditStartupMeasure {
    delegate bool EnumProc(IntPtr window, IntPtr param);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr param);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr window, int attr, out int value, int length);
    public static VEditStartupSample Start(string executable, string profile) {
        var info = new ProcessStartInfo(executable);
        info.UseShellExecute = false;
        info.CreateNoWindow = true;
        info.WindowStyle = ProcessWindowStyle.Hidden;
        info.WorkingDirectory = profile;
        var sample = new VEditStartupSample { Clock = Stopwatch.StartNew() };
        sample.Process = Process.Start(info);
        while (sample.Clock.Elapsed.TotalSeconds < 15 && !sample.Process.HasExited) {
            bool visible = false;
            EnumWindows((window, _) => {
                uint pid;
                GetWindowThreadProcessId(window, out pid);
                if (pid != sample.Process.Id || !IsWindowVisible(window)) return true;
                var title = new StringBuilder(256);
                GetWindowText(window, title, 256);
                int cloaked;
                if (title.ToString() == "vEdit" && (DwmGetWindowAttribute(window, 14, out cloaked, 4) != 0 || cloaked == 0)) visible = true;
                return true;
            }, IntPtr.Zero);
            if (visible) {
                sample.VisibleMs = sample.Clock.Elapsed.TotalMilliseconds;
                break;
            }
            Thread.Sleep(2);
        }
        return sample;
    }
}
'@
    }
    function Observe([string]$EndpointPath, $Request) {
        $endpoint = Get-Content -LiteralPath $EndpointPath -Raw | ConvertFrom-Json
        $client = [Net.Sockets.TcpClient]::new()
        try {
            $client.ReceiveTimeout = 10000
            $client.SendTimeout = 10000
            $client.Connect([Net.IPAddress]::Loopback, [int]$endpoint.port)
            $stream = $client.GetStream()
            $packet = @{version=1;token=$endpoint.token;request=$Request} | ConvertTo-Json -Compress -Depth 8
            $bytes = [Text.Encoding]::UTF8.GetBytes($packet + "`n")
            $stream.Write($bytes, 0, $bytes.Length)
            $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8)
            try { $response = $reader.ReadLine() | ConvertFrom-Json }
            finally { $reader.Dispose() }
            if (-not $response.ok) { throw 'The private editor rejected a startup observation.' }
            $response.result
        } finally { $client.Dispose() }
    }
    Remove-Item Env:VEDIT_CONTROL_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:MORROW_CONTROL_DIR -ErrorAction SilentlyContinue
    $samples = @()
    foreach ($projectCount in @(0, 5, 20)) {
        for ($run = 1; $run -le $Runs; $run++) {
            $profile = Join-Path $fixture "$projectCount-projects-$run"
            New-Item -ItemType Directory -Path "$profile/recovery" -Force | Out-Null
            $projects = @(for ($project = 0; $project -lt $projectCount; $project++) {
                $root = Join-Path $profile "project-$project"
                New-Item -ItemType Directory -Path $root -Force | Out-Null
                $tabs = @(for ($tab = 0; $tab -lt 6; $tab++) {
                    $path = Join-Path $root "notes-$tab.txt"
                    [IO.File]::WriteAllText($path, ("Example line for startup measurement.`n" * 200))
                    @{path=$path;title="notes-$tab.txt"}
                })
                @{root=$root;tabs=$tabs;active_tab_index=5}
            })
            if ($projectCount) {
                $session = @{projects=$projects;active_project=($projectCount-1);settings=@{autosave=$false}} | ConvertTo-Json -Depth 12
                [IO.File]::WriteAllText((Join-Path $profile 'recovery/session.json'), $session, [Text.UTF8Encoding]::new($false))
            }
            $env:VEDIT_PROFILE_DIR = $profile
            foreach ($stage in @('fresh', 'restored')) {
                $endpointPath = Join-Path $profile 'recovery/control.json'
                $sample = [VEditStartupMeasure]::Start($executable, $profile)
                try {
                    if ($sample.VisibleMs -lt 0) { throw 'No visible private editor window appeared within 15 seconds.' }
                    while (-not (Test-Path -LiteralPath $endpointPath)) {
                        if ($sample.Clock.Elapsed.TotalSeconds -ge 15) { throw 'Private editor controls did not become ready.' }
                        Start-Sleep -Milliseconds 2
                    }
                    do {
                        $status = Observe $endpointPath @{op='status'}
                        if ($status.pid -ne $sample.Process.Id -or -not $status.recovery_directory.StartsWith($profile, [StringComparison]::OrdinalIgnoreCase)) {
                            throw 'Startup observation reached a different editor instance.'
                        }
                        if ($status.tabs.total -ne ($projectCount * 6) -or $status.workspaces.Count -ne $projectCount -or ($projectCount -gt 0 -and -not $status.active_tab)) {
                            throw "Expected $projectCount Projects and $($projectCount * 6) Documents; got $($status.workspaces.Count) Projects, $($status.tabs.total) Documents, active=$($status.active_tab)."
                        }
                        $page = $status.tabs
                        $active = @($page.items | Where-Object id -eq $status.active_tab)
                        while ($status.active_tab -and $active.Count -eq 0 -and $null -ne $page.next_offset) {
                            $page = Observe $endpointPath @{op='tabs';limit=100;offset=$page.next_offset}
                            $active = @($page.items | Where-Object id -eq $status.active_tab)
                        }
                        $ready = -not $status.active_tab -or ($active.Count -eq 1 -and -not $active[0].loading -and $null -eq $active[0].error)
                        if (-not $ready -and $sample.Clock.Elapsed.TotalSeconds -ge 15) { throw 'The restored active document did not become usable.' }
                    } while (-not $ready)
                    $samples += [pscustomobject]@{
                        projects=$projectCount;documents=($projectCount*6);stage=$stage;run=$run
                        visible_ms=[Math]::Round($sample.VisibleMs, 1)
                        confirmed_ready_ms=[Math]::Round($sample.Clock.Elapsed.TotalMilliseconds, 1)
                    }
                } finally {
                    if (-not $sample.Process.HasExited) {
                        $sample.Process.Refresh()
                        $null = $sample.Process.CloseMainWindow()
                        if (-not $sample.Process.WaitForExit(10000)) {
                            if ($sample.Process.Path -eq $executable) { Stop-Process -Id $sample.Process.Id }
                            throw 'The recorded private fixture did not close normally.'
                        }
                    }
                    if (Test-Path -LiteralPath $endpointPath) { throw 'A normally closed fixture retained its control endpoint.' }
                }
            }
        }
    }
    @{executable=$executable;sha256=(Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash;samples=$samples} |
        ConvertTo-Json -Depth 6 | Set-Content -LiteralPath "$fixture/results.json" -Encoding UTF8
    $samples | Format-Table -AutoSize
    Write-Output "Measurements: $fixture/results.json"
} finally {
    foreach ($name in $previous.Keys) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
    Exit-ProjectTemp $projectTemp
}
