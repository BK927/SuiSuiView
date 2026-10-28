[CmdletBinding()]
param([Parameter(Mandatory)][string]$Artifacts, [Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'This account-creating test wrapper runs only on disposable GitHub-hosted runners.'
}
$user = 'SsvMsiTest' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$password = ConvertTo-SecureString ([Guid]::NewGuid().ToString('N') + '!aA9') -AsPlainText -Force
$credential = [PSCredential]::new("$env:COMPUTERNAME\$user", $password)
$testRoot = Join-Path $env:PUBLIC $user
try {
    New-LocalUser -Name $user -Password $password -AccountNeverExpires | Out-Null
    Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $user
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    Copy-Item -LiteralPath (Resolve-Path $Artifacts) -Destination (Join-Path $testRoot 'artifacts') -Recurse
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'test_windows_msi.ps1') -Destination $testRoot
    & icacls $testRoot /grant "${user}:(OI)(CI)M" /T /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Could not grant access to the isolated test directory' }
    $script = Join-Path $testRoot 'test_windows_msi.ps1'
    $logs = Join-Path $testRoot 'evidence'
    $args = @('-NoProfile', '-File', "`"$script`"", '-Artifacts', "`"$(Join-Path $testRoot 'artifacts')`"", '-Evidence', "`"$logs`"")
    $process = Start-Process -FilePath (Get-Command pwsh).Source -ArgumentList $args -Credential $credential -LoadUserProfile -WindowStyle Hidden -Wait -PassThru
    if (Test-Path -LiteralPath $logs) {
        New-Item -ItemType Directory -Force -Path $Evidence | Out-Null
        Copy-Item -Path (Join-Path $logs '*') -Destination $Evidence -Recurse
    }
    if ($process.ExitCode -ne 0) { throw "Non-admin MSI test failed with exit code $($process.ExitCode)" }
    if (-not (Test-Path -LiteralPath (Join-Path $Evidence 'msi-validation.json'))) { throw 'MSI test produced no evidence' }
} finally {
    Remove-LocalUser -Name $user -ErrorAction SilentlyContinue
    # The runner destroys this dedicated account's files with the VM. Do not
    # issue recursive profile deletion while Windows Installer may hold handles.
}
