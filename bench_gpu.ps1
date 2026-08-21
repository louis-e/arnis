# GPU Benchmark Script for Arnis
# Compares GPU vs CPU on a very large area
# Usage: .\bench_gpu.ps1

$ErrorActionPreference = "Stop"
$exe = "target\release\arnis.exe"

# Large bbox — this will produce a grid near MAX_ELEVATION_GRID_DIM (16384)
# terrain-only skips OSM fetching, focusing benchmark on elevation processing
$bbox = "48.00 11.40 48.15 11.55"

Write-Output "========================================="
Write-Output "  Arnis GPU Benchmark — Large Area"
Write-Output "  Bbox: $bbox"
Write-Output "========================================="

# ── CPU run ──
Write-Output "`n=== CPU RUN ==="
Remove-Item -Recurse -Force "world" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path "world\region" -Force | Out-Null
Remove-Item env:ARNIS_GPU -ErrorAction SilentlyContinue

$cpu_start = Get-Date
$cpu_out = & $exe --path=".\world" --mode=terrain-only --benchmark --bbox="$bbox" 2>&1 | Out-String
$cpu_elapsed = [math]::Round(((Get-Date) - $cpu_start).TotalSeconds, 1)
$cpu_gen = 0
if ($cpu_out -match 'generation_time_ms=(\d+)') { $cpu_gen = [int]$Matches[1] }

# Extract GPU-relevant timings
$cpu_repair = 0; if ($cpu_out -match 'elev_landcover_repair_ms=(\d+)') { $cpu_repair = [int]$Matches[1] }
$cpu_gaussian = 0; if ($cpu_out -match 'elev_builtup_gaussian_ms=(\d+)') { $cpu_gaussian = [int]$Matches[1] }

Write-Output "  Wall clock:      ${cpu_elapsed}s"
Write-Output "  Generation:      ${cpu_gen}ms"
Write-Output "  LandCover repair: ${cpu_repair}ms"

# ── GPU run ──
Write-Output "`n=== GPU RUN ==="
Remove-Item -Recurse -Force "world\Arnis World 1" -ErrorAction SilentlyContinue
$env:ARNIS_GPU = "1"

$gpu_start = Get-Date
$gpu_out = & $exe --path=".\world" --mode=terrain-only --benchmark --bbox="$bbox" 2>&1 | Out-String
$gpu_elapsed = [math]::Round(((Get-Date) - $gpu_start).TotalSeconds, 1)
$gpu_gen = 0
if ($gpu_out -match 'generation_time_ms=(\d+)') { $gpu_gen = [int]$Matches[1] }
$gpu_repair = 0; if ($gpu_out -match 'elev_landcover_repair_ms=(\d+)') { $gpu_repair = [int]$Matches[1] }

Write-Output "  Wall clock:      ${gpu_elapsed}s"
Write-Output "  Generation:      ${gpu_gen}ms"
Write-Output "  LandCover repair: ${gpu_repair}ms"

# ── Summary ──
Write-Output "`n========================================="
Write-Output "  SUMMARY"
Write-Output "========================================="
$speedup = if ($gpu_gen -gt 0) { [math]::Round($cpu_gen / $gpu_gen, 2) } else { "N/A" }
$repair_speedup = if ($gpu_repair -gt 0) { [math]::Round($cpu_repair / $gpu_repair, 2) } else { "N/A" }
Write-Output "  CPU wall: ${cpu_elapsed}s  |  GPU wall: ${gpu_elapsed}s"
Write-Output "  CPU gen : ${cpu_gen}ms  |  GPU gen : ${gpu_gen}ms  (speedup: ${speedup}x)"
Write-Output "  CPU repair: ${cpu_repair}ms  |  GPU repair: ${gpu_repair}ms  (speedup: ${repair_speedup}x)"

# Save to file
$results = @"
scenario,wall_cpu_s,wall_gpu_s,gen_cpu_ms,gen_gpu_ms,repair_cpu_ms,repair_gpu_ms,speedup
large,$cpu_elapsed,$gpu_elapsed,$cpu_gen,$gpu_gen,$cpu_repair,$gpu_repair,$speedup
"@
$results | Set-Content "benchmark_gpu_large.csv" -Encoding UTF8
Write-Output "`nSaved to benchmark_gpu_large.csv"
