# Runs only in the disposable, non-admin CI test account created by the wrapper.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Artifacts, [Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($identity.Name -notmatch '\\SsvMsiTest[0-9a-f]+$' -or
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Installer tests require the dedicated non-administrator CI account.'
}
New-Item -ItemType Directory -Force -Path $Evidence | Out-Null
# Start-Process -Credential loads the hive but inherits the caller's environment.
# Resolve this account's profile by SID before expanding any known-folder paths
# or starting msiexec; never grant the test account access to runneradmin's data.
$profileKey = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\' + $identity.User.Value
$profilePath = [IO.Path]::GetFullPath((Get-ItemProperty -LiteralPath $profileKey).ProfileImagePath)
if ((Split-Path $profilePath -Leaf) -notlike 'SsvMsiTest*') { throw 'Unexpected disposable user profile' }
$env:USERPROFILE = $profilePath
$env:APPDATA = Join-Path $profilePath 'AppData\Roaming'
$env:LOCALAPPDATA = Join-Path $profilePath 'AppData\Local'
$env:HOMEDRIVE = [IO.Path]::GetPathRoot($profilePath).TrimEnd('\')
$env:HOMEPATH = $profilePath.Substring($env:HOMEDRIVE.Length)
$env:TEMP = Join-Path $env:LOCALAPPDATA 'Temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
$tempProbe = Join-Path $env:TEMP ([Guid]::NewGuid().ToString('N') + '.tmp')
Set-Content -LiteralPath $tempProbe -Value 'isolated MSI account'
Remove-Item -LiteralPath $tempProbe
if ([IO.Path]::GetTempPath().TrimEnd('\') -ne $env:TEMP) { throw 'Temporary directory escaped the disposable profile' }
$installerRoot = 'HKCU:\Software\BK927\SuiSuiView\Installer'
$associationRoot = 'HKCU:\Software\BK927\SuiSuiView\Associations\SuiSuiView.Installed'
$manifest = Get-Content -LiteralPath (Join-Path $Artifacts 'msi-manifest.json') -Raw | ConvertFrom-Json
$candidate = Join-Path $Artifacts $manifest.installer
$fixtures = Join-Path $Artifacts 'fixtures'
$zips = @(Get-ChildItem -LiteralPath $Artifacts -Filter '*-windows-x64.zip')
if ($zips.Count -ne 1) { throw 'Expected exactly one validated ZIP' }
$zip = $zips[0]
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\SuiSuiView'
$result = @{ non_admin = $true; profile = $profilePath; clean_pc_tested = $false; gui_tested = $false; tests = @() }

function Invoke-Msi([string]$Name, [string[]]$Arguments, [bool]$ExpectFailure = $false) {
    $log = Join-Path $Evidence "$Name.log"
    $process = Start-Process -FilePath "$env:SystemRoot\System32\msiexec.exe" -ArgumentList ($Arguments + @('/qn', '/norestart', '/L*v', "`"$log`"")) -WindowStyle Hidden -PassThru -Wait
    if ($ExpectFailure) {
        if ($process.ExitCode -ne 1603) { throw "$Name did not reach the expected MSI failure: $($process.ExitCode)" }
        $text = Get-Content -LiteralPath $log -Raw
        if ($Name -eq 'block-downgrade' -and $text -notmatch 'A newer SuiSuiView is already installed') {
            throw 'Downgrade failed for a different reason'
        }
        if ($Name -eq 'rollback-failed-upgrade') {
            $remove = [regex]::Match($text, '(?m)^Action start .*: RemoveExistingProducts\.')
            $failure = [regex]::Match($text, '(?m)^Action start .*: TestRollbackFailure\.')
            if (-not $remove.Success -or -not $failure.Success -or $remove.Index -ge $failure.Index -or
                $text -notmatch 'Intentional disposable-runner rollback test' -or
                $text.Substring($failure.Index) -notmatch 'Executing op: RollbackInfo\(') {
                throw 'Missing evidence of prior-product removal, injected late failure and rollback'
            }
        }
    } elseif ($process.ExitCode -notin @(0, 3010)) {
        throw "$Name returned unexpected Windows Installer exit code $($process.ExitCode); see $log"
    }
    $result.tests += @{ name = $Name; exit_code = $process.ExitCode; passed = $true }
}
function Assert-Installed([string]$Version) {
    $registration = Get-ItemProperty -LiteralPath $installerRoot
    if ($registration.ProductVersion -ne $Version) { throw "Installed version is not $Version" }
    if ([IO.Path]::GetFullPath($registration.InstallLocation).TrimEnd('\') -ne $installDir) { throw 'Unexpected installation directory' }
    if (-not (Test-Path -LiteralPath (Join-Path $installDir 'suisuiview.exe'))) { throw 'Installed EXE missing' }
}
function Assert-Payload {
    if (Test-Path -LiteralPath (Join-Path $installDir 'msi-test-marker.txt')) { throw 'An upgrade/rollback left the test fixture marker' }
    $extract = Join-Path $Evidence 'zip-payload'
    if (-not (Test-Path -LiteralPath $extract)) { Expand-Archive -LiteralPath $zip.FullName -DestinationPath $extract }
    $payload = @(Get-ChildItem -LiteralPath $extract -Directory)[0].FullName
    foreach ($file in Get-ChildItem -LiteralPath $payload -Recurse -File) {
        $relative = [IO.Path]::GetRelativePath($payload, $file.FullName)
        $installed = Join-Path $installDir $relative
        if (-not (Test-Path -LiteralPath $installed) -or
            (Get-FileHash -LiteralPath $installed).Hash -ne (Get-FileHash -LiteralPath $file.FullName).Hash) {
            throw "Installed file differs from validated ZIP: $relative"
        }
    }
}

try {
    if (Test-Path -LiteralPath $installerRoot) { throw 'Disposable account already contains an installation' }
    $settings = Join-Path $env:APPDATA 'SuiSuiView\msi-preservation-test.txt'
    New-Item -ItemType Directory -Force -Path (Split-Path $settings) | Out-Null
    Set-Content -LiteralPath $settings -Value 'preserve user settings and bookmarks'
    # A neighboring portable registration and another OpenWith value must survive.
    $portableKey = 'HKCU:\Software\BK927\SuiSuiView\Associations\SuiSuiView.Portable.CITest'
    New-Item -Path $portableKey -Force | Out-Null
    New-ItemProperty -LiteralPath $portableKey -Name OwnedExecutable -Value 'D:\portable\suisuiview.exe' | Out-Null
    $openWith = 'HKCU:\Software\Classes\.png\OpenWithProgids'
    New-Item -Path $openWith -Force | Out-Null
    New-ItemProperty -LiteralPath $openWith -Name 'SuiSuiView.Portable.CITest.png' -Value '' | Out-Null

    $versions = Get-Content -LiteralPath (Join-Path $fixtures 'versions.json') -Raw | ConvertFrom-Json
    Invoke-Msi 'install-older' @('/i', "`"$(Join-Path $fixtures 'older.msi')`"")
    Assert-Installed $versions.older
    if ((Get-Content -LiteralPath (Join-Path $installDir 'msi-test-marker.txt') -Raw) -notmatch 'older') { throw 'Older fixture marker missing' }
    $zipOpenWith = 'HKCU:\Software\Classes\.zip\OpenWithProgids'
    if ((Test-Path -LiteralPath $zipOpenWith) -and (Get-Item -LiteralPath $zipOpenWith).GetValueNames().Contains('SuiSuiView.Installed.zip')) { throw 'ZIP candidate must remain opt-in' }
    # Simulate a full optional ZIP registration before upgrading the old app.
    New-Item -Path $zipOpenWith -Force | Out-Null
    New-ItemProperty -LiteralPath $zipOpenWith -Name 'SuiSuiView.Installed.zip' -Value '' | Out-Null
    $zipClass = 'HKCU:\Software\Classes\SuiSuiView.Installed.zip'
    New-Item -Path "$zipClass\shell\open\command" -Force | Out-Null
    Set-Item -LiteralPath "$zipClass\shell\open\command" -Value "`"$(Join-Path $installDir 'suisuiview.exe')`" `"%1`""
    New-ItemProperty -LiteralPath "$associationRoot\Capabilities\FileAssociations" -Name '.zip' -Value 'SuiSuiView.Installed.zip' | Out-Null
    Invoke-Msi 'upgrade' @('/i', "`"$candidate`"")
    Assert-Installed $versions.current
    Assert-Payload
    $command = (Get-Item -LiteralPath 'HKCU:\Software\Classes\SuiSuiView.Installed.png\shell\open\command').GetValue('')
    if ($command -ne "`"$(Join-Path $installDir 'suisuiview.exe')`" `"%1`"") { throw 'File association command is not quoted correctly' }
    if (-not (Get-Item -LiteralPath $openWith).GetValueNames().Contains('SuiSuiView.Installed.png')) { throw 'OpenWith registration missing' }
    if (-not (Test-Path -LiteralPath "$zipClass\shell\open\command") -or -not (Get-Item -LiteralPath $zipOpenWith).GetValueNames().Contains('SuiSuiView.Installed.zip')) { throw 'Upgrade lost the selected ZIP association' }
    $shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\SuiSuiView\SuiSuiView.lnk'
    if (-not (Test-Path -LiteralPath $shortcut)) { throw 'Start menu shortcut missing' }

    Invoke-Msi 'block-downgrade' @('/i', "`"$(Join-Path $fixtures 'older.msi')`"") $true
    Assert-Installed $versions.current
    Invoke-Msi 'rollback-failed-upgrade' @('/i', "`"$(Join-Path $fixtures 'rollback.msi')`"") $true
    Assert-Installed $versions.current
    Assert-Payload
    if (-not (Get-Item -LiteralPath $zipOpenWith).GetValueNames().Contains('SuiSuiView.Installed.zip')) { throw 'Rollback lost optional ZIP registration' }

    $repairFile = Join-Path $installDir 'SOURCE.txt'
    Remove-Item -LiteralPath $repairFile
    Invoke-Msi 'repair' @('/i', "`"$candidate`"")
    Assert-Payload
    Invoke-Msi 'uninstall' @('/x', "`"$candidate`"")
    if (Test-Path -LiteralPath (Join-Path $installDir 'suisuiview.exe')) { throw 'Uninstall left the executable' }
    if (Test-Path -LiteralPath $zipClass) { throw 'Uninstall left the optional ZIP class' }
    if ((Test-Path -LiteralPath $installerRoot) -or (Test-Path -LiteralPath $associationRoot)) { throw 'Uninstall left owned registration keys' }
    if ((Get-Item -LiteralPath $openWith).GetValueNames().Contains('SuiSuiView.Installed.png')) { throw 'Uninstall left its OpenWith value' }
    if ((Test-Path -LiteralPath $zipOpenWith) -and (Get-Item -LiteralPath $zipOpenWith).GetValueNames().Contains('SuiSuiView.Installed.zip')) { throw 'Uninstall left optional ZIP registration' }
    if (-not (Test-Path -LiteralPath $portableKey) -or -not (Test-Path -LiteralPath $settings)) { throw 'Uninstall removed unrelated user data' }
    if (-not (Get-Item -LiteralPath $openWith).GetValueNames().Contains('SuiSuiView.Portable.CITest.png')) { throw 'Uninstall removed a portable association' }
    $result.passed = $true
} catch {
    $result.passed = $false
    $result.error = $_.ToString()
    throw
} finally {
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $Evidence 'msi-validation.json') -Encoding utf8
}
