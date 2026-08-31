param(
    [Parameter(Mandatory = $true)]
    [string]$PstPath
)

$ErrorActionPreference = "Stop"

cargo run --release -- $PstPath
