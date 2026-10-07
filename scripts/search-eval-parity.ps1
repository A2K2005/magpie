param(
    [Parameter(Mandatory = $true)][ValidateCount(2, 8)][string[]]$Reports
)
$ErrorActionPreference = 'Stop'
$loaded = @($Reports | ForEach-Object { Get-Content -LiteralPath $_ -Raw | ConvertFrom-Json })
$baseline = $loaded[0]
if (-not $baseline.embedding_sha256) { throw 'Reports need per-image embedding_sha256 fingerprints.' }
for ($i = 1; $i -lt $loaded.Count; $i++) {
    $candidate = $loaded[$i]
    if ($candidate.embedding_version -ne $baseline.embedding_version) { throw "Model contract differs: $($Reports[$i])" }
    if (($candidate.model_files | ConvertTo-Json -Depth 10 -Compress) -ne ($baseline.model_files | ConvertTo-Json -Depth 10 -Compress)) { throw "Model files differ: $($Reports[$i])" }
    if (($candidate.embedding_sha256 | ConvertTo-Json -Compress) -ne ($baseline.embedding_sha256 | ConvertTo-Json -Compress)) { throw "Image vectors differ: $($Reports[$i])" }
    if ($candidate.queries.Count -ne $baseline.queries.Count) { throw 'Query counts differ.' }
    for ($q = 0; $q -lt $baseline.queries.Count; $q++) {
        $fields = 'query', 'mode', 'k', 'relevant', 'returned', 'scores', 'evidence', 'has_more'
        $a = $baseline.queries[$q] | Select-Object $fields | ConvertTo-Json -Depth 10 -Compress
        $b = $candidate.queries[$q] | Select-Object $fields | ConvertTo-Json -Depth 10 -Compress
        if ($a -ne $b) { throw "Retrieval differs for query $q in $($Reports[$i])" }
    }
}
"PASS: identical image vectors and retrieval across $($loaded.Count) reports; $($baseline.images) images, $($baseline.queries.Count) queries."
