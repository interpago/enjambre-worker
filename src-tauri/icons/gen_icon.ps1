function Create-GreenIcon {
    $path = "C:\Users\Pc\worker-node\src-tauri\icons\icon.png"
    Add-Type -AssemblyName System.Drawing
    $bmp = New-Object System.Drawing.Bitmap 32, 32
    $green = [System.Drawing.Color]::FromArgb(255, 0, 200, 0)
    for ($y = 0; $y -lt 32; $y++) {
        for ($x = 0; $x -lt 32; $x++) {
            $bmp.SetPixel($x, $y, $green)
        }
    }
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "Icon created: $path"
}
Create-GreenIcon
