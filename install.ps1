# Installs capsa tools from the newest release of each tool.
# Usage: .\install.ps1 [acus pons census all]   (default: all; velum is macOS-only)
#        .\install.ps1 -Uninstall <acus|pons|census|all>
# Env: CAPSA_INSTALL_DIR (default %USERPROFILE%\.local\bin), CAPSA_INSTALL_SCOPE=user|system,
#      CAPSA_NO_SKILL=1 skips `acus self skill --install` (ACUS_NO_SKILL works too) and
#      keeps the acus skill on -Uninstall,
#      GITHUB_TOKEN (else `gh auth token`) while the repository is private.
param([string[]]$Tools, [switch]$Uninstall)

$ErrorActionPreference = "Stop"

$Repo = "LuMiSxh/capsa"
$Api = "https://api.github.com/repos/$Repo"
$InstallDir = if ($env:CAPSA_INSTALL_DIR) { $env:CAPSA_INSTALL_DIR } else { "$env:USERPROFILE\.local\bin" }
$Target = "x86_64-pc-windows-msvc"
# Choose with $env:CAPSA_INSTALL_SCOPE = "user" (default, no admin needed) or "system"
# (all users, needs an elevated shell).
$EnvTarget = if ($env:CAPSA_INSTALL_SCOPE -ieq "system") { "Machine" } else { "User" }
$Token = $null

function Main {
    if (-not $Tools) {
        if ($Uninstall) { throw "usage: install.ps1 -Uninstall <acus|pons|census|all>" }
        $Tools = @("all")
    }
    $selected = @()
    foreach ($t in $Tools) {
        switch ($t.ToLower()) {
            "all"   { $selected += "acus", "pons", "census" }
            "acus"  { $selected += "acus" }
            "pons"  { $selected += "pons" }
            "census" { $selected += "census" }
            "velum" { throw "velum is macOS-only for now" }
            default { throw "unknown tool: $t (use acus, pons, census or all)" }
        }
    }

    if ($Uninstall) {
        foreach ($tool in ($selected | Select-Object -Unique)) {
            Uninstall-Tool $tool
        }
        return
    }

    $script:Token = Get-Token
    foreach ($tool in ($selected | Select-Object -Unique)) {
        Install-Tool $tool
    }
    Add-ToPath
}

function Install-Tool([string]$Tool) {
    try {
        $tag = Get-LatestTag $Tool
    } catch {
        $tag = $null
    }
    if (-not $tag) {
        throw "no $Tool release found in $Repo, or GitHub denied access (the repository is private: set GITHUB_TOKEN or run gh auth login)"
    }

    $asset = "$Tool-$Target.zip"
    $release = Invoke-Api "$Api/releases/tags/$tag" "application/vnd.github+json" | ConvertFrom-Json
    $archive = $release.assets | Where-Object { $_.name -eq $asset } | Select-Object -First 1
    $sums = $release.assets | Where-Object { $_.name -eq "$asset.sha256" } | Select-Object -First 1
    if (-not $archive -or -not $sums) { throw "release $tag has no asset $asset" }
    $archiveUrl = $archive.url
    $sumUrl = $sums.url

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zipPath = Join-Path $tmp $asset
        $sumPath = Join-Path $tmp "$asset.sha256"
        Write-Host "downloading $Tool $tag for $Target..."
        Invoke-Api $archiveUrl "application/octet-stream" $zipPath | Out-Null
        Invoke-Api $sumUrl "application/octet-stream" $sumPath | Out-Null

        $expected = ((Get-Content $sumPath -Raw).Trim() -split '\s+')[0].ToLower()
        $actual = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { throw "SHA-256 mismatch for ${asset}: expected $expected, got $actual" }

        Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        }
        Copy-Item (Join-Path $tmp "$Tool.exe") (Join-Path $InstallDir "$Tool.exe") -Force
        Write-Host "installed $Tool $tag to $InstallDir\$Tool.exe"

        # Installs the agent skill for Claude Code and Codex.
        if ($Tool -eq "acus" -and -not $env:CAPSA_NO_SKILL -and -not $env:ACUS_NO_SKILL) {
            & (Join-Path $InstallDir "acus.exe") self skill --install
        }
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Removes the binary from $InstallDir and, for acus, the skill the installer wrote.
# Only a regular file named exactly like the tool is removed; a link or directory aborts.
function Uninstall-Tool([string]$Tool) {
    $path = Join-Path $InstallDir "$Tool.exe"
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if ($item) {
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "refusing to remove ${path}: not a regular file"
        }
        Remove-Item -LiteralPath $path -Force
        Write-Host "removed $path"
    } else {
        Write-Host "$Tool is not installed in $InstallDir"
    }

    # The installer writes the skill for Claude Code and Codex; CAPSA_NO_SKILL keeps it.
    if ($Tool -eq "acus" -and -not $env:CAPSA_NO_SKILL -and -not $env:ACUS_NO_SKILL) {
        Remove-Skill (Join-Path $env:USERPROFILE ".claude\skills\acus")
        Remove-Skill (Join-Path $env:USERPROFILE ".agents\skills\acus")
    }
}

# Removes SKILL.md from a skill directory, then the directory when it is empty.
function Remove-Skill([string]$Dir) {
    $dirItem = Get-Item -LiteralPath $Dir -Force -ErrorAction SilentlyContinue
    $file = Join-Path $Dir "SKILL.md"
    $fileItem = Get-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
    if (($dirItem -and ($dirItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) -or
        ($fileItem -and ($fileItem.Attributes -band [IO.FileAttributes]::ReparsePoint))) {
        throw "refusing to remove ${Dir}: symbolic link"
    }
    if ($fileItem -and -not $fileItem.PSIsContainer) {
        Remove-Item -LiteralPath $file -Force
        Write-Host "removed $file"
    }
    if ($dirItem -and $dirItem.PSIsContainer) {
        if (-not (Get-ChildItem -LiteralPath $Dir -Force)) {
            Remove-Item -LiteralPath $Dir -Force
            Write-Host "removed $Dir"
        } else {
            Write-Host "kept ${Dir}: it holds other files"
        }
    }
}

# Prints the tag of the newest <Tool>-v* release, paging until one is found.
function Get-LatestTag([string]$Tool) {
    for ($page = 1; $page -le 10; $page++) {
        $releases = @((Invoke-Api "$Api/releases?per_page=100&page=$page" "application/vnd.github+json") | ConvertFrom-Json)
        if ($releases.Count -eq 0) { return $null }
        $hit = $releases | Where-Object { $_.tag_name -like "$Tool-v*" } | Select-Object -First 1
        if ($hit) { return $hit.tag_name }
    }
    return $null
}

# curl.exe ships with Windows 10 1803+ and drops the token when the asset download
# redirects to storage.
function Invoke-Api([string]$Url, [string]$Accept, [string]$OutFile) {
    $curlArgs = @("-fsSL", "-H", "Accept: $Accept")
    if ($Token) { $curlArgs += @("-H", "Authorization: Bearer $Token") }
    if ($OutFile) { $curlArgs += @("-L", "-o", $OutFile) }
    $curlArgs += $Url
    $body = (& curl.exe @curlArgs | Out-String)
    if ($LASTEXITCODE -ne 0) { throw "GitHub request failed: $Url" }
    return $body
}

function Get-Token {
    if ($env:GITHUB_TOKEN) { return $env:GITHUB_TOKEN }
    if (Get-Command gh -ErrorAction SilentlyContinue) {
        $token = & gh auth token 2>$null
        if ($LASTEXITCODE -eq 0 -and $token) { return "$token".Trim() }
    }
    return $null
}

# Adds $InstallDir to the user (default) or machine PATH.
function Add-ToPath {
    $existingPath = [Environment]::GetEnvironmentVariable("Path", $EnvTarget)
    if ($existingPath -like "*$InstallDir*") { return }

    try {
        [Environment]::SetEnvironmentVariable("Path", "$InstallDir;$existingPath", $EnvTarget)
        $env:Path = "$InstallDir;$env:Path"
        Write-Host "added $InstallDir to $EnvTarget PATH (open a new terminal to use it there)"
    } catch {
        Write-Host "could not update $EnvTarget PATH ($($_.Exception.Message))"
        if ($EnvTarget -eq "Machine") {
            Write-Host "system-wide install needs an elevated (Administrator) shell, re-run there, or add manually:"
        } else {
            Write-Host "add manually:"
        }
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$InstallDir;`$env:Path`", '$EnvTarget')"
    }
}

Main
