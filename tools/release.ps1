[CmdletBinding()]
param([switch]$Publish)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$config = Get-Content -LiteralPath "$root\src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json
$version = $config.version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Stable releases require a numeric x.y.z version.' }
$notes = "$root\releases\$version.md"
if (!(Test-Path -LiteralPath $notes)) { throw "Write release notes first: releases\$version.md" }
if ($Publish) {
    $dirty = & git status --porcelain
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'Commit and push the exact release source before publishing.' }
    $commit = & git rev-parse HEAD
    $remote = & git ls-remote origin refs/heads/main
    if ($LASTEXITCODE -ne 0 -or !($remote -match "^$commit\s")) { throw 'Push the release commit to origin/main first.' }
}
$key = Join-Path $env:LOCALAPPDATA 'TextureHelperRelease\updater.key'
if (!$env:TAURI_SIGNING_PRIVATE_KEY) {
    if (!(Test-Path -LiteralPath $key)) { throw 'Set TAURI_SIGNING_PRIVATE_KEY or provision your private updater signing key outside the repository.' }
    $env:TAURI_SIGNING_PRIVATE_KEY = $key
}
if (!$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '' }
Push-Location "$root\src-tauri"
try {
    & cargo tauri build --ci
    if ($LASTEXITCODE -ne 0) { throw 'Signed installer build failed.' }
} finally { Pop-Location }
$installer = "$root\src-tauri\target\release\bundle\nsis\PERPHEADS Texture Helper_${version}_x64-setup.exe"
foreach ($path in @($installer, "$installer.sig")) {
    if (!(Test-Path -LiteralPath $path)) { throw "Missing release output: $path" }
}
$dist = "$root\dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$name = "PERPHEADS-Texture-Helper-Setup-$version-win-x64.exe"
Copy-Item -LiteralPath $installer -Destination "$dist\$name" -Force
Copy-Item -LiteralPath "$installer.sig" -Destination "$dist\$name.sig" -Force
$sum = (Get-FileHash -LiteralPath "$dist\$name" -Algorithm SHA256).Hash.ToLowerInvariant()
"$sum  $name" | Set-Content -LiteralPath "$dist\SHA256SUMS-$version.txt" -Encoding ASCII
$manifest = @{
    version = $version
    notes = Get-Content -LiteralPath $notes -Raw
    pub_date = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = @{ 'windows-x86_64' = @{
        signature = (Get-Content -LiteralPath "$installer.sig" -Raw).Trim()
        url = "https://github.com/JoeyMarrey29/TextureHelper/releases/download/v$version/$name"
    } }
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$dist\latest.json" -Encoding UTF8
if ($Publish) {
    & gh release create "v$version" "$dist\$name" "$dist\$name.sig" "$dist\SHA256SUMS-$version.txt" "$dist\latest.json" `
        --repo JoeyMarrey29/TextureHelper --target $commit --title "PERPHEADS Texture Helper $version" --notes-file $notes
    if ($LASTEXITCODE -ne 0) { throw 'Release publication failed; inspect GitHub before retrying.' }
}
Get-Item -LiteralPath "$dist\$name"
