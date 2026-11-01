<#
.SYNOPSIS
  GitHop hook performance benchmark (Plan section 11, Phase 3 section 4.5).

.DESCRIPTION
  Builds the release binary, sets up an ISOLATED git + GitHop environment in a
  temp folder (HOME, USERPROFILE, GIT_CONFIG_GLOBAL, GIT_CONFIG_NOSYSTEM=1,
  GITHOP_HOME, GITHOP_NO_REGISTRY=1 - your real config is never read or
  written), then measures:

    version             githop --version (cold start)
    commit-pinned       githop check --stage commit, pinned repo (fast path)
    commit-pinned-hook  same, with GIT_AUTHOR_EMAIL set the way git exports it
                        to a pre-commit hook
    commit-unpinned     githop check --stage commit, repo resolved by owner
    push-100            githop check --stage push, 100 new commits
    git-commit-hooks    whole `git commit` with GitHop's hooks (sh + githop)
    git-commit-nohooks  whole `git commit` with no hooks at all

  Uses hyperfine when it is on PATH, else a .NET process loop. Reports p50,
  p95, mean (ms). -Check compares p95 with perf-baseline.json and fails on a
  regression above 20% or on a Plan section 11 budget miss.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File benches\perf.ps1 -Runs 60
  powershell -ExecutionPolicy Bypass -File benches\perf.ps1 -Check
  powershell -ExecutionPolicy Bypass -File benches\perf.ps1 -UpdateBaseline
#>
param(
    [int]$Runs = 60,
    [int]$Warmup = 5,
    [switch]$Check,
    [switch]$UpdateBaseline,
    [switch]$NoHyperfine,
    [string]$Baseline = (Join-Path $PSScriptRoot '..\perf-baseline.json')
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# Plan section 11 budgets (p95, ms).
$budgets = [ordered]@{
    'commit-pinned'      = 100
    'commit-pinned-hook' = 100
    'commit-unpinned'    = 100
    'push-100'           = 300
}

Write-Host 'Building release githop...'
Push-Location $repoRoot
try {
    cargo build --release -p githop-cli --quiet
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }
$exe = Join-Path $repoRoot 'target\release\githop.exe'
$gitExe = (Get-Command git -ErrorAction Stop).Source

# --- isolated environment (restored in finally) ------------------------------
$isoVars = @('HOME', 'USERPROFILE', 'GIT_CONFIG_GLOBAL', 'GIT_CONFIG_NOSYSTEM', 'GITHOP_HOME',
    'GITHOP_NO_REGISTRY', 'NO_COLOR', 'GIT_TERMINAL_PROMPT', 'GIT_AUTHOR_NAME', 'GIT_AUTHOR_EMAIL',
    'GIT_AUTHOR_DATE', 'GIT_COMMITTER_NAME', 'GIT_COMMITTER_EMAIL', 'GIT_COMMITTER_DATE',
    'GIT_SSH_COMMAND', 'GIT_DIR', 'GIT_WORK_TREE', 'GIT_CONFIG_PARAMETERS', 'GITHOP_DISABLE')
$saved = @{}
foreach ($v in $isoVars) { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('githop-perf-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$homeDir = Join-Path $tmp 'home'
$dataDir = Join-Path $tmp 'appdata'
$reposDir = Join-Path $tmp 'repos'
$emptyHooks = Join-Path $tmp 'no-hooks'
foreach ($d in @($homeDir, $dataDir, $reposDir, $emptyHooks)) { New-Item -ItemType Directory -Force $d | Out-Null }
[IO.File]::WriteAllText((Join-Path $homeDir '.gitconfig'), '')

function Set-Iso {
    foreach ($v in $isoVars) { [Environment]::SetEnvironmentVariable($v, $null, 'Process') }
    $env:HOME = $homeDir
    $env:USERPROFILE = $homeDir
    $env:GIT_CONFIG_GLOBAL = (Join-Path $homeDir '.gitconfig')
    $env:GIT_CONFIG_NOSYSTEM = '1'
    $env:GITHOP_HOME = $dataDir
    $env:GITHOP_NO_REGISTRY = '1'
    $env:NO_COLOR = '1'
    $env:GIT_TERMINAL_PROMPT = '0'
}

function Invoke-Native([string]$file, [string[]]$argv, [string]$cwd) {
    # PowerShell 5.1 turns native stderr into errors under 'Stop'.
    $ErrorActionPreference = 'Continue'
    Push-Location $cwd
    try {
        $out = & $file @argv 2>&1
        if ($LASTEXITCODE -ne 0) { throw "$file $($argv -join ' ') failed: $out" }
        return ($out | Out-String).Trim()
    } finally { Pop-Location }
}

function Get-Stats([double[]]$ms) {
    $s = $ms | Sort-Object
    $n = $s.Count
    $p = { param($q) $s[[Math]::Max(0, [Math]::Ceiling($q * $n) - 1)] }
    [ordered]@{
        runs = $n
        p50  = [Math]::Round((& $p 0.50), 1)
        p95  = [Math]::Round((& $p 0.95), 1)
        mean = [Math]::Round(($s | Measure-Object -Average).Average, 1)
        min  = [Math]::Round($s[0], 1)
        max  = [Math]::Round($s[$n - 1], 1)
    }
}

$hyperfine = $null
if (-not $NoHyperfine) {
    $hf = Get-Command hyperfine -ErrorAction SilentlyContinue
    if (-not $hf) { $hf = Get-Command (Join-Path $env:USERPROFILE '.cargo\bin\hyperfine.exe') -ErrorAction SilentlyContinue }
    if ($hf) { $hyperfine = $hf.Source }
}

function Measure-Case([string]$name, [string]$file, [string[]]$argv, [string]$cwd, [string]$stdinFile) {
    if ($hyperfine) {
        $json = Join-Path $tmp "$name.json"
        # Windows PowerShell 5.1 mangles embedded quotes, and hyperfine -N splits on
        # shell rules: pass forward-slash paths, no quotes (no spaces in these args).
        $prog = $file
        if ($prog -match ' ') { $prog = (New-Object -ComObject Scripting.FileSystemObject).GetFile($file).ShortPath }
        $cmdline = (@($prog -replace '\\', '/') + $argv) -join ' '
        $hfArgs = @('-N', '--warmup', "$Warmup", '--runs', "$Runs", '--export-json', $json, '--style', 'none')
        if ($stdinFile) { $hfArgs += @('--input', $stdinFile) }
        $hfArgs += $cmdline
        Push-Location $cwd
        try {
            & $hyperfine @hfArgs | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "hyperfine failed for $name" }
        } finally { Pop-Location }
        $times = (Get-Content $json -Raw | ConvertFrom-Json).results[0].times | ForEach-Object { [double]$_ * 1000.0 }
        return Get-Stats $times
    }
    $stdin = if ($stdinFile) { [IO.File]::ReadAllText($stdinFile) } else { $null }
    $times = New-Object System.Collections.Generic.List[double]
    for ($i = 0; $i -lt ($Warmup + $Runs); $i++) {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $file
        $psi.Arguments = (($argv | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' ')
        $psi.WorkingDirectory = $cwd
        $psi.UseShellExecute = $false
        $psi.RedirectStandardInput = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $proc = [Diagnostics.Process]::Start($psi)
        if ($stdin) { $proc.StandardInput.Write($stdin) }
        $proc.StandardInput.Close()
        $null = $proc.StandardOutput.ReadToEnd()
        $null = $proc.StandardError.ReadToEnd()
        $proc.WaitForExit()
        $sw.Stop()
        if ($i -ge $Warmup) { $times.Add($sw.Elapsed.TotalMilliseconds) }
    }
    return Get-Stats $times.ToArray()
}

$results = [ordered]@{}
try {
    Set-Iso
    $office = 'harshal@office.example'
    $personal = 'harshal@personal.example'
    $ssh = (Join-Path $homeDir '.ssh') -replace '\\', '/'
    Invoke-Native $gitExe @('config', '--global', 'user.name', 'Harshal') $homeDir | Out-Null
    Invoke-Native $gitExe @('config', '--global', 'user.email', $personal) $homeDir | Out-Null
    Invoke-Native $exe @('init', '--id', 'personal', '--label', 'Personal', '--yes') $homeDir | Out-Null
    Invoke-Native $exe @('edit', 'personal', '--owner', 'harshal-personal') $homeDir | Out-Null
    Invoke-Native $exe @('add', '--id', 'office', '--name', 'Harshal Office', '--email', $office,
        '--owner', 'office-org', '--key', "$ssh/githop_office") $homeDir | Out-Null

    # Pinned office repo.
    $pinned = Join-Path $reposDir 'billing-api'
    New-Item -ItemType Directory $pinned | Out-Null
    Invoke-Native $gitExe @('init', '-q', '-b', 'main') $pinned | Out-Null
    Invoke-Native $gitExe @('remote', 'add', 'origin', 'git@github.com:office-org/billing-api.git') $pinned | Out-Null
    Invoke-Native $exe @('pin', 'office') $pinned | Out-Null
    Invoke-Native $gitExe @('commit', '-q', '--allow-empty', '-m', 'base') $pinned | Out-Null

    # Unpinned repo resolved by owner (personal, matches the global identity).
    $unpinned = Join-Path $reposDir 'side'
    New-Item -ItemType Directory $unpinned | Out-Null
    Invoke-Native $gitExe @('init', '-q', '-b', 'main') $unpinned | Out-Null
    Invoke-Native $gitExe @('remote', 'add', 'origin', 'https://github.com/harshal-personal/side.git') $unpinned | Out-Null

    # Push repo: 100 new office commits on top of a base the remote has.
    $push = Join-Path $reposDir 'push-100'
    New-Item -ItemType Directory $push | Out-Null
    Invoke-Native $gitExe @('init', '-q', '-b', 'main') $push | Out-Null
    Invoke-Native $gitExe @('remote', 'add', 'origin', 'git@github.com:office-org/push-100.git') $push | Out-Null
    Invoke-Native $exe @('pin', 'office') $push | Out-Null
    $noHooks = @('-c', "core.hooksPath=$($emptyHooks -replace '\\', '/')")
    Invoke-Native $gitExe ($noHooks + @('commit', '-q', '--allow-empty', '-m', 'base')) $push | Out-Null
    $base = Invoke-Native $gitExe @('rev-parse', 'HEAD') $push
    Invoke-Native $gitExe @('update-ref', 'refs/remotes/origin/main', $base) $push | Out-Null
    for ($i = 1; $i -le 100; $i++) {
        Invoke-Native $gitExe ($noHooks + @('commit', '-q', '--allow-empty', '-m', "change $i")) $push | Out-Null
    }
    $head = Invoke-Native $gitExe @('rev-parse', 'HEAD') $push
    $stdinFile = Join-Path $tmp 'push-stdin.txt'
    [IO.File]::WriteAllText($stdinFile, "refs/heads/main $head refs/heads/main $base`n")

    $tool = if ($hyperfine) { 'hyperfine' } else { '.NET process loop' }
    Write-Host "Measuring ($tool, $Runs runs, $Warmup warm-up)..."
    $results['version'] = Measure-Case 'version' $exe @('--version') $homeDir $null
    $results['commit-pinned'] = Measure-Case 'commit-pinned' $exe @('check', '--stage', 'commit') $pinned $null
    $env:GIT_AUTHOR_NAME = 'Harshal Office'
    $env:GIT_AUTHOR_EMAIL = $office
    $results['commit-pinned-hook'] = Measure-Case 'commit-pinned-hook' $exe @('check', '--stage', 'commit') $pinned $null
    Remove-Item Env:GIT_AUTHOR_NAME, Env:GIT_AUTHOR_EMAIL
    $results['commit-unpinned'] = Measure-Case 'commit-unpinned' $exe @('check', '--stage', 'commit') $unpinned $null
    $results['push-100'] = Measure-Case 'push-100' $exe @('check', '--stage', 'push', '--', 'origin', 'git@github.com:office-org/push-100.git') $push $stdinFile
    $results['git-commit-hooks'] = Measure-Case 'git-commit-hooks' $gitExe @('commit', '-q', '--allow-empty', '-m', 'bench') $pinned $null
    $results['git-commit-nohooks'] = Measure-Case 'git-commit-nohooks' $gitExe ($noHooks + @('commit', '-q', '--allow-empty', '-m', 'bench')) $pinned $null
} finally {
    foreach ($v in $isoVars) { [Environment]::SetEnvironmentVariable($v, $saved[$v], 'Process') }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# --- report --------------------------------------------------------------------
Write-Host ''
Write-Host ('{0,-20} {1,8} {2,8} {3,8}' -f 'case', 'p50 ms', 'p95 ms', 'mean ms')
foreach ($k in $results.Keys) {
    $r = $results[$k]
    Write-Host ('{0,-20} {1,8} {2,8} {3,8}' -f $k, $r.p50, $r.p95, $r.mean)
}
$hookCost = [Math]::Round($results['git-commit-hooks'].p95 - $results['git-commit-nohooks'].p95, 1)
Write-Host ''
Write-Host "Hooks add $hookCost ms to git commit (p95 difference)."

$failed = $false
foreach ($k in $budgets.Keys) {
    if ($results[$k].p95 -gt $budgets[$k]) {
        Write-Host "! $k p95 $($results[$k].p95) ms is over the $($budgets[$k]) ms budget." -ForegroundColor Yellow
        $failed = $true
    }
}

if ($Check -and (Test-Path $Baseline)) {
    $base = Get-Content $Baseline -Raw | ConvertFrom-Json
    foreach ($k in $results.Keys) {
        $was = $base.p95_ms.$k
        if ($was -and $results[$k].p95 -gt ($was * 1.2)) {
            Write-Host "! $k p95 regressed: $($results[$k].p95) ms vs baseline $was ms (> 20%)." -ForegroundColor Yellow
            $failed = $true
        }
    }
}

if ($UpdateBaseline) {
    $p95 = [ordered]@{}
    foreach ($k in $results.Keys) { $p95[$k] = $results[$k].p95 }
    $doc = [ordered]@{
        note      = 'p95 in ms from benches/perf.ps1; -Check fails on > 20% regression. Machine-specific: refresh with -UpdateBaseline on the CI runner.'
        date      = (Get-Date -Format 'yyyy-MM-dd')
        machine   = "$([Environment]::OSVersion.VersionString), $([Environment]::ProcessorCount) logical CPUs"
        tool      = $(if ($hyperfine) { 'hyperfine' } else { 'dotnet-loop' })
        runs      = $Runs
        budget_ms = $budgets
        p95_ms    = $p95
        detail    = $results
    }
    [IO.File]::WriteAllText($Baseline, ($doc | ConvertTo-Json -Depth 5) + "`n", (New-Object System.Text.UTF8Encoding $false))
    Write-Host "Baseline written to $Baseline"
}

if ($failed -and $Check) { exit 1 }
exit 0
