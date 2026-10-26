# M1 capture research on a Windows CI runner (docs/capture-research.md).
# Puts a moving picture on screen, then measures each capture option for $Seconds:
# Windows.Graphics.Capture and DXGI duplication (wincap), FFmpeg gdigrab and ddagrab from
# our LGPL FFmpeg build, and WASAPI loopback + mic. Results: results/*.json and the job
# summary. Usage: run.ps1 -Wincap PATH -FFmpegDir DIR
param(
    [Parameter(Mandatory)] [string] $Wincap,
    [Parameter(Mandatory)] [string] $FFmpegDir,
    [int] $Seconds = 20
)
$ErrorActionPreference = "Continue"
$results = New-Item -ItemType Directory -Force results
$ffmpeg = Join-Path $FFmpegDir "ffmpeg.exe"
$ffprobe = Join-Path $FFmpegDir "ffprobe.exe"
$summary = @("## Windows capture research ($env:RUNNER_OS, $((Get-CimInstance Win32_OperatingSystem).Caption))", "")

function Note($text) { Write-Host $text; $script:summary += $text }

# Runs a program, returns its output plus the CPU time it used (all its threads).
function Measure($name, $exe, [string[]] $argList) {
    $out = Join-Path $results "$name.out.txt"
    $err = Join-Path $results "$name.err.txt"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru -NoNewWindow `
        -RedirectStandardOutput $out -RedirectStandardError $err
    $handle = $p.Handle # keeps the process object usable after exit
    $p.WaitForExit()
    $wall = $watch.Elapsed.TotalSeconds
    $cpu = $p.TotalProcessorTime.TotalSeconds
    $cores = [Environment]::ProcessorCount
    $line = "| $name | $([math]::Round($wall, 1)) s | $([math]::Round(100 * $cpu / $wall, 1)) % of one core ($cores logical) | exit $($p.ExitCode) |"
    Note $line
    return (Get-Content $out -Raw), (Get-Content $err -Raw)
}

Note "Display adapters: $((Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) $($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)" }) -join '; ')"
Note "Logical CPUs: $([Environment]::ProcessorCount)"
Note ""

# A window that repaints 60 times per second, so the screen changes all the time
# (the capture APIs send frames only when something changes).
$animator = @'
Add-Type -AssemblyName System.Windows.Forms
$form = New-Object Windows.Forms.Form
$form.Text = "moving"; $form.Width = 1280; $form.Height = 720; $form.StartPosition = "CenterScreen"; $form.TopMost = $true
$label = New-Object Windows.Forms.Label
$label.Dock = "Fill"; $label.Font = New-Object Drawing.Font("Consolas", 72); $label.TextAlign = "MiddleCenter"
$form.Controls.Add($label)
$n = 0
$timer = New-Object Windows.Forms.Timer
$timer.Interval = 16
$timer.Add_Tick({ $script:n++; $label.Text = "$script:n"; $form.BackColor = [Drawing.Color]::FromArgb(255, ($script:n * 7) % 256, ($script:n * 3) % 256, ($script:n * 11) % 256) })
$timer.Start()
[Windows.Forms.Application]::Run($form)
'@
Set-Content -Path animator.ps1 -Value $animator
$anim = Start-Process powershell -ArgumentList "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "animator.ps1" -PassThru
Start-Sleep -Seconds 4

Note "### FFmpeg build"
$devices = & $ffmpeg -hide_banner -devices 2>&1 | Out-String
$filters = & $ffmpeg -hide_banner -filters 2>&1 | Out-String
$encoders = & $ffmpeg -hide_banner -encoders 2>&1 | Out-String
foreach ($want in "gdigrab", "dshow") { Note "- input device $($want): $(if ($devices -match "\b$want\b") { 'yes' } else { 'NO' })" }
Note "- ddagrab filter: $(if ($filters -match '\bddagrab\b') { 'yes' } else { 'NO' })"
foreach ($want in "h264_mf", "libopenh264", "aac") { Note "- encoder $($want): $(if ($encoders -match "\b$want\b") { 'yes' } else { 'NO' })" }
$dshow = & $ffmpeg -hide_banner -list_devices true -f dshow -i dummy 2>&1 | Out-String
Set-Content (Join-Path $results "dshow-devices.txt") $dshow
Note ""

Note "### Runs"
Note "| run | wall | CPU | exit |"
Note "|---|---|---|---|"
$runs = @{}
$runs["wincap-list"] = (Measure "wincap-list" $Wincap @("list"))[0]
$runs["wgc"] = (Measure "wgc" $Wincap @("wgc", "$Seconds"))[0]
$runs["wgc-readback"] = (Measure "wgc-readback" $Wincap @("wgc", "$Seconds", "--readback"))[0]
$runs["dxgi"] = (Measure "dxgi" $Wincap @("dxgi", "$Seconds"))[0]
$runs["dxgi-readback"] = (Measure "dxgi-readback" $Wincap @("dxgi", "$Seconds", "--readback"))[0]
$runs["audio"] = (Measure "wasapi" $Wincap @("audio", "10"))[0]

# FFmpeg: capture only (decoded frames thrown away), then capture + encode to fragmented MP4.
$ff = @("-hide_banner", "-y", "-stats_period", "5")
$null = Measure "gdigrab-null" $ffmpeg ($ff + @("-f", "gdigrab", "-framerate", "30", "-i", "desktop", "-t", "$Seconds", "-f", "null", "-"))
$null = Measure "gdigrab-h264_mf" $ffmpeg ($ff + @("-f", "gdigrab", "-framerate", "30", "-i", "desktop", "-t", "$Seconds",
    "-vf", "scale=1920:-2,format=nv12", "-c:v", "h264_mf", "-b:v", "8M", "-g", "30",
    "-movflags", "+frag_keyframe+empty_moov+default_base_moof", "results/gdigrab.mp4"))
$null = Measure "ddagrab-null" $ffmpeg ($ff + @("-f", "lavfi", "-i", "ddagrab=framerate=30", "-t", "$Seconds",
    "-vf", "hwdownload,format=bgra", "-f", "null", "-"))
$null = Measure "ddagrab-h264_mf" $ffmpeg ($ff + @("-f", "lavfi", "-i", "ddagrab=framerate=30", "-t", "$Seconds",
    "-vf", "hwdownload,format=bgra,scale=1920:-2,format=nv12", "-c:v", "h264_mf", "-b:v", "8M", "-g", "30",
    "-movflags", "+frag_keyframe+empty_moov+default_base_moof", "results/ddagrab.mp4"))
Note ""

Note "### wincap results"
Note '```'
foreach ($k in "wincap-list", "wgc", "wgc-readback", "dxgi", "dxgi-readback", "audio") { Note "$($k): $($runs[$k])" }
Note '```'
Note "### FFmpeg results (last stats line; dup/drop are FFmpeg's own counters)"
Note '```'
foreach ($k in "gdigrab-null", "gdigrab-h264_mf", "ddagrab-null", "ddagrab-h264_mf") {
    $err = Get-Content (Join-Path $results "$k.err.txt") -Raw
    $stats = ([regex]::Matches($err, "frame=\s*\d+[^\r\n]*") | Select-Object -Last 1).Value
    $errors = ([regex]::Matches($err, "(?im)^.*(error|failed|invalid|could not)[^\r\n]*") | Select-Object -First 3 | ForEach-Object { $_.Value }) -join " / "
    Note "$($k): $stats $errors"
}
Note '```'
foreach ($f in "gdigrab", "ddagrab") {
    $file = "results/$f.mp4"
    if (Test-Path $file) {
        $probe = & $ffprobe -v error -count_frames -select_streams v:0 -show_entries "stream=codec_name,profile,width,height,pix_fmt,nb_read_frames,avg_frame_rate:format=duration" -of compact $file 2>&1 | Out-String
        Note "- $($f).mp4: $($probe.Trim())"
    }
}

Stop-Process -Id $anim.Id -ErrorAction SilentlyContinue
$summary -join "`n" | Out-File -Append -Encoding utf8 $env:GITHUB_STEP_SUMMARY
$summary -join "`n" | Out-File -Encoding utf8 (Join-Path $results "summary.md")
