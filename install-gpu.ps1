# Enjambre Worker - Script de instalación para GPU
# Ejecutar en PowerShell como Administrador

$ErrorActionPreference = "Stop"
$WORKSPACE = "$env:LOCALAPPDATA\Enjambre"

Write-Host "=== Enjambre Worker: Instalación CUDA ===" -ForegroundColor Cyan

# 1. Crear workspace
New-Item -ItemType Directory -Path $WORKSPACE -Force | Out-Null

# 2. Descargar worker-node.exe desde el Master
Write-Host "[1/4] Descargando worker-node.exe..." -ForegroundColor Yellow
curl.exe -s -L -o "$WORKSPACE\worker-node.exe" "http://159.54.175.236:8081/download"

# 3. CUDA Runtime DLLs (cudart64_12, cublas64_12, cublasLt64_12)
Write-Host "[2/4] Descargando CUDA Runtime..." -ForegroundColor Yellow
curl.exe -L -o "$env:TEMP\cudart.zip" "https://developer.download.nvidia.com/compute/cuda/redist/cuda_cudart/windows-x86_64/cuda_cudart-windows-x86_64-12.4.127-archive.zip"
curl.exe -L -o "$env:TEMP\cublas.zip" "https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/libcublas-windows-x86_64-12.4.5.8-archive.zip"

Write-Host "[3/4] Extrayendo DLLs..." -ForegroundColor Yellow
New-Item -ItemType Directory -Path "$env:TEMP\cuda_dlls" -Force | Out-Null
tar -xf "$env:TEMP\cudart.zip" -C "$env:TEMP\cuda_dlls" --strip-components=2 "cuda_cudart-windows-x86_64-12.4.127-archive/bin/cudart64_12.dll"
tar -xf "$env:TEMP\cublas.zip" -C "$env:TEMP\cuda_dlls" --strip-components=2 "libcublas-windows-x86_64-12.4.5.8-archive/bin/cublas64_12.dll" "libcublas-windows-x86_64-12.4.5.8-archive/bin/cublasLt64_12.dll"
Copy-Item "$env:TEMP\cuda_dlls\*" $WORKSPACE

# 4. Limpiar
Remove-Item "$env:TEMP\cudart.zip", "$env:TEMP\cublas.zip", "$env:TEMP\cuda_dlls" -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "[4/4] Instalación completa!" -ForegroundColor Green
Write-Host "" -ForegroundColor Cyan
Write-Host "Para iniciar el worker:" -ForegroundColor White
Write-Host "  Start-Process -FilePath '$WORKSPACE\worker-node.exe'" -ForegroundColor Gray
Write-Host ""
Write-Host "Para verificar GPU:" -ForegroundColor White
Write-Host "  nvidia-smi" -ForegroundColor Gray
