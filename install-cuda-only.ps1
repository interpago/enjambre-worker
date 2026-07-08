# Enjambre Worker - Instalación de soporte CUDA
# Ejecutado automáticamente por el instalador si detecta NVIDIA GPU

$ErrorActionPreference = "Stop"
$WORKSPACE = "$env:LOCALAPPDATA\Enjambre"

Write-Host "=== Instalando soporte CUDA ===" -ForegroundColor Cyan

Write-Host "[1/3] Descargando CUDA Runtime..." -ForegroundColor Yellow
curl.exe -L -o "$env:TEMP\cudart.zip" "https://developer.download.nvidia.com/compute/cuda/redist/cuda_cudart/windows-x86_64/cuda_cudart-windows-x86_64-12.4.127-archive.zip"
curl.exe -L -o "$env:TEMP\cublas.zip" "https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/libcublas-windows-x86_64-12.4.5.8-archive.zip"

Write-Host "[2/3] Extrayendo DLLs..." -ForegroundColor Yellow
New-Item -ItemType Directory -Path "$env:TEMP\cuda_dlls" -Force | Out-Null
tar -xf "$env:TEMP\cudart.zip" -C "$env:TEMP\cuda_dlls" --strip-components=2 "cuda_cudart-windows-x86_64-12.4.127-archive/bin/cudart64_12.dll"
tar -xf "$env:TEMP\cublas.zip" -C "$env:TEMP\cuda_dlls" --strip-components=2 "libcublas-windows-x86_64-12.4.5.8-archive/bin/cublas64_12.dll" "libcublas-windows-x86_64-12.4.5.8-archive/bin/cublasLt64_12.dll"
Copy-Item "$env:TEMP\cuda_dlls\*" $WORKSPACE

Write-Host "[3/3] Limpiando..." -ForegroundColor Yellow
Remove-Item "$env:TEMP\cudart.zip", "$env:TEMP\cublas.zip", "$env:TEMP\cuda_dlls" -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "Soporte CUDA instalado correctamente!" -ForegroundColor Green
