param(
    [Parameter(Mandatory=$true)][string]$TunnelId,
    [string]$KeyFile,
    [string]$KeyEnv,
    [string]$DataDir,
    [string]$Listen='127.0.0.1:18700',
    [string]$Health='127.0.0.1:18710',
    [switch]$DryRun
)
$ErrorActionPreference='Stop'
if($KeyFile -and $KeyEnv){throw 'Specify KeyFile or KeyEnv, not both'}
$artifacts=ConvertFrom-Json @'
@ARTIFACTS@
'@
$platform=if($env:PROCESSOR_ARCHITECTURE -eq 'ARM64'){'windows-arm64'}else{'windows-amd64'}
$asset=$artifacts.$platform
if(-not $asset){throw "No native local release for $platform"}
[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
$stage=Join-Path $env:TEMP ('shellmcp-bootstrap-'+[Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    foreach($component in @('agent','host')) {
        $entry=$asset.$component
        $name=if($component -eq 'agent'){'shellmcp-agent.exe'}else{'shellmcp-host.exe'}
        $path=Join-Path $stage $name
        Invoke-WebRequest -UseBasicParsing -Uri $entry.url -OutFile $path
        if((Get-Item $path).Length -eq 0){throw 'Empty executable download'}
        if((Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant() -ne $entry.sha256){throw 'Executable SHA256 mismatch'}
    }
    $argv=@('local','install','--tunnel-id',$TunnelId,'--listen',$Listen,'--health',$Health)
    if($KeyFile){$argv+=@('--key-file',$KeyFile)}
    if($KeyEnv){$argv+=@('--key-env',$KeyEnv)}
    if($DataDir){$argv+=@('--data-dir',$DataDir)}
    if($DryRun){$argv+='--dry-run'}
    & (Join-Path $stage 'shellmcp-agent.exe') @argv
    if($LASTEXITCODE -ne 0){throw "Local installer failed: $LASTEXITCODE"}
} finally { Remove-Item -Recurse -Force $stage }
