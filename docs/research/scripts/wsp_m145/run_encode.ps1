# M1.45 Davidson encoding driver (Windows PowerShell 7).
# Generates one chunk, encodes L12 (R12 budget) and L6 (rescan budget),
# renames the pure-f32 dump, then deletes the chunk to stay within disk.
$ErrorActionPreference = 'Stop'
$root = 'D:\trit-results\davidson'
$py = 'C:\Users\lbx13\AppData\Local\Programs\Python\Python314\python.exe'
$repo = 'C:\Users\lbx13\Projects\trit-tml\third_party\trit'
$bert6 = Join-Path $repo 'target\release\examples\bert6.exe'
$l12 = 'C:\Users\lbx13\.cache\huggingface\hub\models--sentence-transformers--all-MiniLM-L12-v2\snapshots\a50ef00143b4d5391434df20ae11632588ac25be'
$l6 = 'C:\Users\lbx13\.cache\huggingface\hub\models--sentence-transformers--all-MiniLM-L6-v2\snapshots\1110a243fdf4706b3f48f1d95db1a4f5529b4d41'
$chunks = Join-Path $root 'chunks-l12-1500'
$texts = Join-Path $root 'texts-l12.json'
$emb12 = Join-Path $root 'emb12'
$emb6 = Join-Path $root 'emb6'
New-Item -ItemType Directory -Force -Path $chunks, $emb12, $emb6, (Join-Path $root 'runs12'), (Join-Path $root 'runs6') | Out-Null

$n = 24781
$chunkSize = 1500
$chunkCount = [math]::Ceiling($n / $chunkSize)
$log = Join-Path $root 'encode.log'
"[$(Get-Date -Format o)] start chunks=$chunkCount" | Out-File $log -Append

for ($i = 0; $i -lt $chunkCount; $i++) {
    $tag = '{0:D3}' -f $i
    $f32L12 = Join-Path $emb12 "l12-pure-f32-$tag.f32"
    $qL12 = Join-Path $emb12 "l12-r12-$tag.f32"
    $f32L6 = Join-Path $emb6 "l6-pure-f32-$tag.f32"
    $qL6 = Join-Path $emb6 "l6-rescan-$tag.f32"
    if ((Test-Path $f32L12) -and (Test-Path $qL12) -and (Test-Path $f32L6) -and (Test-Path $qL6)) {
        "[$(Get-Date -Format o)] skip $tag (all present)" | Out-File $log -Append
        continue
    }

    "[$(Get-Date -Format o)] chunk $tag" | Out-File $log -Append
    & $py (Join-Path $repo 'docs\research\scripts\wsp_m140\make_chunk_inputs.py') `
        --train (Join-Path $root 'raw\train.jsonl') `
        --test (Join-Path $root 'raw\test.jsonl') `
        --model-dir $l12 --dst $chunks --texts-out $texts `
        --chunk-size $chunkSize --only-chunk $i | Out-File $log -Append

    $chunk = Join-Path $chunks "chunk-$tag"
    if (-not (Test-Path $chunk)) { throw "missing chunk $chunk" }

    if (-not ((Test-Path $f32L12) -and (Test-Path $qL12))) {
        $env:TRIT_LAYERS = '12'
        $env:TRIT_WEIGHT_BITS = '3'
        $env:TRIT_BUDGET = 'D:\trit-results\scifact\r12\swap-r12.json'
        $env:TRIT_THREADS = '16'
        $env:TRIT_SKIP_DIAG = '1'
        $env:TRIT_NO_LAYERS = '1'
        $env:TRIT_EMBED_DUMP = $emb12
        $env:TRIT_EMBED_TAG = "l12-r12-$tag"
        & $bert6 --file (Join-Path $l12 'model.safetensors') --data-dir $chunk `
            --activation-mode f32 --out (Join-Path $root "runs12\l12-r12-$tag.json") | Out-File $log -Append
        Move-Item (Join-Path $emb12 'pure-f32.f32') $f32L12 -Force
    }

    if (-not ((Test-Path $f32L6) -and (Test-Path $qL6))) {
        $env:TRIT_LAYERS = '6'
        $env:TRIT_WEIGHT_BITS = '3'
        $env:TRIT_BUDGET = 'D:\trit-results\l6\l6-rescan-k2.json'
        $env:TRIT_THREADS = '16'
        $env:TRIT_SKIP_DIAG = '1'
        $env:TRIT_NO_LAYERS = '1'
        $env:TRIT_EMBED_DUMP = $emb6
        $env:TRIT_EMBED_TAG = "l6-rescan-$tag"
        & $bert6 --file (Join-Path $l6 'model.safetensors') --data-dir $chunk `
            --activation-mode f32 --out (Join-Path $root "runs6\l6-rescan-$tag.json") | Out-File $log -Append
        Move-Item (Join-Path $emb6 'pure-f32.f32') $f32L6 -Force
    }

    Remove-Item $chunk -Recurse -Force
    $free = [math]::Round((Get-PSDrive D).Free / 1GB, 2)
    "[$(Get-Date -Format o)] done $tag free=${free}GB" | Out-File $log -Append
}
"[$(Get-Date -Format o)] all done" | Out-File $log -Append
