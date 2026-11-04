param(
    [Parameter(Position = 0)][string]$Command = 'status',
    [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Rest
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:PkgRoot   = $PSScriptRoot
$script:ScriptPath = $MyInvocation.MyCommand.Path
$script:CfgPath   = Join-Path $script:PkgRoot 'pkg.json'
$script:DataDir   = Join-Path $script:PkgRoot 'data'
$script:StateDir  = Join-Path $script:PkgRoot 'state'
$script:StateFile = Join-Path $script:StateDir 'registration.json'
$script:BackupDir = Join-Path $script:StateDir 'backups'
$script:Args      = @($Rest)
$script:Sep       = [IO.Path]::PathSeparator
$script:Version   = '1.0.0'

# Windows PowerShell 5.1 can inherit a PowerShell 7 module path and lose core
# cmdlets. Keep the 5.1 module directory first while this engine runs.
$script:OriginalPSModulePath = $env:PSModulePath
$script:CorePSModulePath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules'
if ($env:PSModulePath -notlike ($script:CorePSModulePath + '*')) {
    $env:PSModulePath = $script:CorePSModulePath + ';' + $env:PSModulePath
}

function Write-Info([string]$Message) { if (-not (Has-Flag '--quiet')) { Write-Host $Message } }
function Write-Warn([string]$Message) { Write-Host ("WARN  " + $Message) -ForegroundColor Yellow }
function Write-Err ([string]$Message) { Write-Host ("ERROR " + $Message) -ForegroundColor Red }

function Has-Flag([string]$Name) { return ($script:Args -contains $Name) }
function Get-Option([string]$Name, [string]$Default = $null) {
    $i = [array]::IndexOf($script:Args, $Name)
    if ($i -ge 0 -and ($i + 1) -lt $script:Args.Count) { return $script:Args[$i + 1] }
    return $Default
}

function Get-NormPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return '' }
    $x = $Path.Trim().Trim('"') -replace '/', '\'
    return $x.TrimEnd('\').ToLowerInvariant()
}

function Test-PathUnder([string]$Path, [string]$Base) {
    if ([string]::IsNullOrWhiteSpace($Path) -or [string]::IsNullOrWhiteSpace($Base)) { return $false }
    $n = Get-NormPath $Path
    $b = Get-NormPath $Base
    return ($n -eq $b) -or $n.StartsWith($b + '\')
}

function Split-PathList([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value)) { return @() }
    return @($Value -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
}

function Ensure-Directory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Force -Path $Path | Out-Null
    }
    return $Path
}

function Get-Config {
    if (-not (Test-Path -LiteralPath $script:CfgPath)) { throw "missing pkg.json: $script:CfgPath" }
    return (Get-Content -LiteralPath $script:CfgPath -Raw -Encoding UTF8 | ConvertFrom-Json)
}

function Expand-Value([string]$Value, $Cfg) {
    if ($null -eq $Value) { return '' }
    return $Value.Replace('{pkg}', $script:PkgRoot).Replace('{data}', $script:DataDir) -replace '/', '\'
}

function Ensure-DataDirs($Cfg) {
    if ($Cfg.dataDirs) {
        foreach ($d in @($Cfg.dataDirs)) {
            $p = Expand-Value ([string]$d) $Cfg
            if (-not [IO.Path]::IsPathRooted($p)) { $p = Join-Path $script:PkgRoot $p }
            Ensure-Directory $p | Out-Null
        }
    }
}

function Get-Plan {
    $cfg = Get-Config
    Ensure-Directory $script:StateDir | Out-Null
    Ensure-DataDirs $cfg

    $paths = New-Object System.Collections.Generic.List[string]
    $seen = @{}
    foreach ($raw in @($cfg.path)) {
        if ([string]::IsNullOrWhiteSpace([string]$raw)) { continue }
        $p = Expand-Value ([string]$raw) $cfg
        if (-not [IO.Path]::IsPathRooted($p)) { $p = Join-Path $script:PkgRoot $p }
        if ($p -match '[*?]') {
            $found = @(Get-ChildItem -Path $p -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending)
            if ($found.Count -gt 0) { $p = $found[0].FullName } else { continue }
        } elseif (-not (Test-Path -LiteralPath $p)) {
            continue
        }
        $p = [IO.Path]::GetFullPath($p)
        $key = Get-NormPath $p
        if (-not $seen.ContainsKey($key)) {
            $seen[$key] = $true
            $paths.Add($p)
        }
    }

    $vars = [ordered]@{}
    if ($cfg.env) {
        foreach ($prop in $cfg.env.PSObject.Properties) {
            $vars[$prop.Name] = Expand-Value ([string]$prop.Value) $cfg
        }
    }
    return [pscustomobject]@{ Paths = $paths.ToArray(); Vars = $vars; Config = $cfg }
}

function Get-State {
    if (Test-Path -LiteralPath $script:StateFile) {
        try { return (Get-Content -LiteralPath $script:StateFile -Raw -Encoding UTF8 | ConvertFrom-Json) }
        catch { Write-Warn "state file is unreadable, ignoring: $($_.Exception.Message)" }
    }
    return [pscustomobject]@{ schemaVersion = 1; root = $script:PkgRoot; scopes = [pscustomobject]@{} }
}

function Save-State($State) {
    Ensure-Directory $script:StateDir | Out-Null
    [IO.File]::WriteAllText($script:StateFile, ($State | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding($false)))
}

function Get-ScopeState($State, [string]$Scope) {
    if ($null -eq $State -or $null -eq $State.scopes) { return $null }
    $prop = $State.scopes.PSObject.Properties[$Scope]
    if ($null -eq $prop) { return $null }
    return $prop.Value
}

function Test-OwnedEntry([string]$Entry, $State, $ScopeState) {
    if (Test-PathUnder $Entry $script:PkgRoot) { return $true }
    if ($null -ne $State -and $State.root -and (Test-PathUnder $Entry ([string]$State.root))) { return $true }
    if ($null -ne $ScopeState -and $ScopeState.entries) {
        $n = Get-NormPath $Entry
        foreach ($e in @($ScopeState.entries)) {
            if ((Get-NormPath ([string]$e)) -eq $n) { return $true }
        }
    }
    return $false
}

function Get-ScopeKey([string]$Scope, [bool]$Writable) {
    if ($Scope -eq 'machine') {
        return [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $Writable)
    }
    return [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $Writable)
}

function Get-ScopePath([string]$Scope) {
    $key = Get-ScopeKey $Scope $false
    if ($null -eq $key) { return '' }
    try { return [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    finally { $key.Close() }
}

function Get-ScopeValue([string]$Scope, [string]$Name) {
    $key = Get-ScopeKey $Scope $false
    if ($null -eq $key) { return $null }
    try { return $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    finally { $key.Close() }
}

function Backup-ScopePath([string]$Scope) {
    Ensure-Directory $script:BackupDir | Out-Null
    $file = Join-Path $script:BackupDir ("$Scope-" + (Get-Date).ToString('yyyyMMdd-HHmmss') + '.json')
    $data = [ordered]@{ scope = $Scope; at = (Get-Date).ToString('o'); path = (Get-ScopePath $Scope) }
    [IO.File]::WriteAllText($file, ($data | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
    return $file
}

function Send-EnvironmentChanged {
    if (Has-Flag '--no-broadcast') { return }
    try {
        if (-not ('ArPkgNative' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ArPkgNative {
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
}
'@
        }
        $r = [UIntPtr]::Zero
        [void][ArPkgNative]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$r)
    } catch {
        Write-Warn "could not broadcast environment change: $($_.Exception.Message)"
    }
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p = New-Object Security.Principal.WindowsPrincipal($id)
    return $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Invoke-Elevated([string[]]$Arguments) {
    $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $argLine = @('-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $script:ScriptPath + '"')) + $Arguments
    try {
        $p = Start-Process -FilePath $ps -Verb RunAs -ArgumentList $argLine -PassThru -Wait
        return $p.ExitCode
    } catch {
        throw "elevation was cancelled or failed: $($_.Exception.Message)"
    }
}

function Set-ProcessPlan($Plan) {
    $state = Get-State
    $owned = @{}
    if ($state.root) { $owned[(Get-NormPath ([string]$state.root))] = $true }
    foreach ($s in @('machine', 'user')) {
        $ss = Get-ScopeState $state $s
        if ($null -ne $ss -and $ss.entries) {
            foreach ($e in @($ss.entries)) { $owned[(Get-NormPath ([string]$e))] = $true }
        }
    }
    $current = Split-PathList $env:PATH
    $keep = @($current | Where-Object {
        if (Test-PathUnder $_ $script:PkgRoot) { return $false }
        if ($owned.ContainsKey((Get-NormPath $_))) { return $false }
        return $true
    })
    $env:PATH = (@($Plan.Paths) + $keep) -join $script:Sep
    foreach ($k in $Plan.Vars.Keys) { Set-Item -Path ("Env:" + $k) -Value $Plan.Vars[$k] }
    if ($null -ne $script:OriginalPSModulePath) { $env:PSModulePath = $script:OriginalPSModulePath }
}

function Register-Scope([string]$Scope) {
    $cfg = Get-Config
    if (-not (Test-Path -LiteralPath (Join-Path $script:PkgRoot ([string]$cfg.payload)))) {
        throw "payload is missing; reinstall package $($cfg.id)"
    }
    if ($Scope -eq 'machine' -and -not (Test-Admin)) {
        if (Has-Flag '--elevated') { throw 'machine scope requires administrator rights' }
        $argsList = @('register', '--machine', '--elevated')
        if (Has-Flag '--quiet') { $argsList += '--quiet' }
        $code = Invoke-Elevated $argsList
        if ($code -ne 0) { throw 'machine registration did not complete' }
        return
    }

    $plan = Get-Plan
    $state = Get-State
    $oldScopeState = Get-ScopeState $state $Scope
    $backup = Backup-ScopePath $Scope
    $key = Get-ScopeKey $Scope $true
    if ($null -eq $key) { throw "cannot open $Scope environment registry key" }
    try {
        $oldPath = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = $key.GetValueKind('Path')
        $existing = Split-PathList $oldPath
        $kept = @($existing | Where-Object { -not (Test-OwnedEntry $_ $state $oldScopeState) })
        $key.SetValue('Path', ((@($plan.Paths) + $kept) -join ';'), $kind)

        $scopeState = [ordered]@{
            registeredAt = (Get-Date).ToString('o')
            pathBackup   = $backup
            entries      = @($plan.Paths)
            variables    = @()
        }
        $oldVars = @{}
        if ($null -ne $oldScopeState -and $oldScopeState.variables) {
            foreach ($ov in @($oldScopeState.variables)) { $oldVars[[string]$ov.name] = $ov }
        }
        foreach ($name in $plan.Vars.Keys) {
            $value = [string]$plan.Vars[$name]
            $previous = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($oldVars.ContainsKey($name)) {
                $ov = $oldVars[$name]
                if (($null -ne $previous) -and ([string]$previous -eq [string]$ov.value)) { $previous = $ov.previous }
            }
            $scopeState.variables += [ordered]@{
                name = $name; value = $value
                previous = if ($null -eq $previous) { $null } else { [string]$previous }
            }
            $kindVar = if ($value -match '%') { [Microsoft.Win32.RegistryValueKind]::ExpandString } else { [Microsoft.Win32.RegistryValueKind]::String }
            $key.SetValue($name, $value, $kindVar)
        }
        if ($null -eq $state.scopes) { $state | Add-Member -NotePropertyName scopes -NotePropertyValue ([pscustomobject]@{}) -Force }
        $state.scopes | Add-Member -NotePropertyName $Scope -NotePropertyValue ([pscustomobject]$scopeState) -Force
        $state | Add-Member -NotePropertyName root -NotePropertyValue $script:PkgRoot -Force
        Save-State $state
    } finally {
        $key.Close()
    }
    Send-EnvironmentChanged
    Write-Info "$($cfg.id) registered for $Scope : $(@($plan.Paths).Count) PATH entries, $($plan.Vars.Count) variables."
}

function Unregister-Scope([string]$Scope) {
    $cfg = Get-Config
    $state = Get-State
    $scopeState = Get-ScopeState $state $Scope
    $present = @((Split-PathList (Get-ScopePath $Scope)) | Where-Object { Test-OwnedEntry $_ $state $scopeState }).Count
    if ($present -eq 0 -and $null -eq $scopeState) {
        Write-Info "$($cfg.id) is not registered for $Scope."
        return
    }
    if ($Scope -eq 'machine' -and -not (Test-Admin)) {
        if (Has-Flag '--elevated') { throw 'machine scope requires administrator rights' }
        $argsList = @('unregister', '--machine', '--elevated')
        if (Has-Flag '--quiet') { $argsList += '--quiet' }
        $code = Invoke-Elevated $argsList
        if ($code -ne 0) { throw 'machine unregistration did not complete' }
        return
    }

    $backup = Backup-ScopePath $Scope
    $key = Get-ScopeKey $Scope $true
    if ($null -eq $key) { throw "cannot open $Scope environment registry key" }
    try {
        $oldPath = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = $key.GetValueKind('Path')
        $existing = Split-PathList $oldPath
        $kept = @($existing | Where-Object { -not (Test-OwnedEntry $_ $state $scopeState) })
        $removed = $existing.Count - $kept.Count
        $key.SetValue('Path', ($kept -join ';'), $kind)

        if ($null -ne $scopeState -and $scopeState.variables) {
            foreach ($v in $scopeState.variables) {
                $current = $key.GetValue($v.name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                if ($null -ne $current -and [string]$current -eq [string]$v.value) {
                    if ($null -eq $v.previous) { $key.DeleteValue($v.name, $false) }
                    else { $key.SetValue($v.name, [string]$v.previous, [Microsoft.Win32.RegistryValueKind]::ExpandString) }
                } elseif ($null -ne $current) {
                    Write-Warn "kept $($v.name): it was changed after registration"
                }
            }
        }
        if ($null -ne $state.scopes) { $state.scopes.PSObject.Properties.Remove($Scope) }
        Save-State $state
    } finally {
        $key.Close()
    }
    Send-EnvironmentChanged
    Write-Info "$($cfg.id) unregistered from $Scope : removed $removed PATH entries."
}

function Write-EnvFile([string]$Path, $Plan) {
    $state = Get-State
    $owned = @{}
    if ($state.root) { $owned[(Get-NormPath ([string]$state.root))] = $true }
    foreach ($s in @('machine', 'user')) {
        $ss = Get-ScopeState $state $s
        if ($null -ne $ss -and $ss.entries) { foreach ($e in @($ss.entries)) { $owned[(Get-NormPath ([string]$e))] = $true } }
    }
    $current = Split-PathList $env:PATH
    $keep = @($current | Where-Object {
        if (Test-PathUnder $_ $script:PkgRoot) { return $false }
        if ($owned.ContainsKey((Get-NormPath $_))) { return $false }
        return $true
    })
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('@echo off')
    $lines.Add('rem generated by arpkg - do not edit')
    $lines.Add(('set "PATH=' + ((@($Plan.Paths) + $keep) -join $script:Sep) + '"'))
    foreach ($k in $Plan.Vars.Keys) { $lines.Add(('set "' + $k + '=' + [string]$Plan.Vars[$k] + '"')) }
    if (-not $Path) { $Path = Join-Path $script:StateDir 'env.cmd' }
    $dir = Split-Path -Parent $Path
    if ($dir) { Ensure-Directory $dir | Out-Null }
    [IO.File]::WriteAllText($Path, ($lines -join "`r`n"), [Text.Encoding]::Default)
}

function Show-Status {
    $cfg = Get-Config
    $plan = Get-Plan
    $installed = Test-Path -LiteralPath (Join-Path $script:PkgRoot ([string]$cfg.payload))
    Write-Info ""
    Write-Info ("package  " + $cfg.id + " " + $cfg.version)
    Write-Info ("root     " + $script:PkgRoot)
    Write-Info ("payload  " + $(if ($installed) { 'installed' } else { 'missing' }))
    foreach ($s in @('machine', 'user')) {
        $reg = @((Split-PathList (Get-ScopePath $s)) | Where-Object { Test-OwnedEntry $_ (Get-State) (Get-ScopeState (Get-State) $s) }).Count -gt 0
        Write-Info ("{0,-8} {1}" -f $s, $(if ($reg) { 'registered' } else { 'not registered' }))
    }
    Write-Info ("paths    " + @($plan.Paths).Count + "   variables " + @($plan.Vars.Count))
    Write-Info ""
}

function Show-Doctor {
    $plan = Get-Plan
    Set-ProcessPlan $plan
    $cfg = Get-Config
    if (-not $cfg.probe -or @($cfg.probe).Count -eq 0) { Write-Info 'no probe defined.'; return }
    $exe = Expand-Value ([string]@($cfg.probe)[0]) $cfg
    if (-not [IO.Path]::IsPathRooted($exe)) { $exe = Join-Path $script:PkgRoot $exe }
    $pArgs = @()
    if (@($cfg.probe).Count -gt 1) { $pArgs = @($cfg.probe)[1..(@($cfg.probe).Count - 1)] }
    if (-not (Test-Path -LiteralPath $exe)) { Write-Err "missing probe: $exe"; exit 1 }
    $oldEap = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $out = & $exe @pArgs 2>&1
        $code = $LASTEXITCODE
        $line = @($out | Where-Object { $null -ne $_ -and "$_" -ne '' } | Select-Object -First 1)
        if ($code -eq 0) { Write-Info ("OK   " + $line); exit 0 }
        Write-Err ("FAIL " + $line + " (exit $code)"); exit 1
    } finally {
        $ErrorActionPreference = $oldEap
    }
}

function Show-Help {
    @'
arpkg - one portable environment package

  arpkg status
  arpkg register --machine|--user
  arpkg unregister --machine|--user|--all
  arpkg env [--apply] [--apply-file <file>]
  arpkg doctor
'@ | Write-Host
}

try {
    switch ($Command.ToLowerInvariant()) {
        'status' { Show-Status }
        'register' {
            $scope = if (Has-Flag '--machine') { 'machine' } elseif (Has-Flag '--user') { 'user' } else { $null }
            if (-not $scope) { throw 'use --machine or --user' }
            Register-Scope $scope
        }
        'unregister' {
            if (Has-Flag '--all') { Unregister-Scope 'machine'; Unregister-Scope 'user' }
            else {
                $scope = if (Has-Flag '--machine') { 'machine' } elseif (Has-Flag '--user') { 'user' } else { $null }
                if (-not $scope) { throw 'use --machine, --user or --all' }
                Unregister-Scope $scope
            }
        }
        'env' {
            $plan = Get-Plan
            if (Has-Flag '--apply') { Set-ProcessPlan $plan }
            if (Has-Flag '--apply-file') {
                $file = Get-Option '--apply-file'
                Write-EnvFile $file $plan
            }
            if ((-not (Has-Flag '--apply')) -and (-not (Has-Flag '--apply-file'))) {
                foreach ($p in $plan.Paths) { Write-Output ('PATH  ' + $p) }
                foreach ($k in $plan.Vars.Keys) { Write-Output ('VAR   ' + $k + '=' + $plan.Vars[$k]) }
            }
        }
        'doctor' { Show-Doctor }
        'version' { Write-Info $script:Version }
        'help' { Show-Help }
        '-h' { Show-Help }
        '--help' { Show-Help }
        default { Show-Help; exit 2 }
    }
} catch {
    Write-Err $_.Exception.Message
    exit 1
}



