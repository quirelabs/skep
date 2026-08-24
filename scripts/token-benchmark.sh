#!/usr/bin/env bash
# Measures what an agent spends to answer the question it asks most often:
# "what dev services are running, and is the database ready?"
#
#   scripts/token-benchmark.sh [output-directory]
#
# Both transcripts are real. The baseline runs the commands an agent actually
# reaches for without skep and keeps their real output; the skep side makes one
# MCP call and keeps its real reply. Nothing here is illustrative, because a
# benchmark against an invented baseline is worth nothing.
#
# Token counts come from Anthropic's count_tokens endpoint when ANTHROPIC_API_KEY
# is set. Without it the script still runs and reports an estimate, clearly
# labelled as one.
#
# One asymmetry, stated rather than hidden: skep_status answers only for the
# services skep manages, while the shell commands survey the whole machine.
# That is the point rather than a trick, since an agent cannot narrow the shell
# search without already knowing the answer, but the numbers are not measuring
# identical questions. The size of the baseline also depends on how much the
# machine has installed.

set -euo pipefail

out=${1:-bench}
mcp_bin=${SKEP_MCP:-target/debug/skep-mcp}
model=${BENCH_MODEL:-claude-sonnet-5}

mkdir -p "$out"
manual="$out/manual.txt"
single="$out/skep.txt"

if [[ ! -x $mcp_bin ]]; then
    echo "no skep-mcp at $mcp_bin. Run: cargo build" >&2
    exit 1
fi

# --- what an agent does with only a shell ---------------------------------

: >"$manual"
step() {
    printf '$ %s\n' "$1" >>"$manual"
    eval "$1" >>"$manual" 2>&1 || true
    printf '\n' >>"$manual"
}

step "brew services list"
step "ps aux | grep -E 'postgres|redis|valkey|mysql|mariadb|mongod|mailpit' | grep -v grep"
step "lsof -nP -iTCP -sTCP:LISTEN"
step "pg_isready -h 127.0.0.1 -p 5432"

# --- what an agent does with skep ------------------------------------------

session() {
    echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"bench","version":"1"}}}'
    echo '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"skep_status","arguments":{}}}'
    sleep 1
}

reply=$(session | "$mcp_bin" 2>/dev/null | python3 -c "
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    if message.get('id') == 2:
        result = message['result']
        print(result['content'][0]['text'])
        sys.exit(1 if result.get('isError') else 0)
sys.exit(2)
") || {
    echo "skep_status did not answer. Is an engine running? Start one with: skep serve" >&2
    echo "$reply" >&2
    exit 1
}

{
    printf 'skep_status()\n'
    printf '%s\n' "$reply"
} >"$single"

# --- counting --------------------------------------------------------------

count() {
    local file=$1
    if [[ -n ${ANTHROPIC_API_KEY:-} ]]; then
        python3 - "$file" "$model" <<'PY'
import json, os, sys, urllib.request
path, model = sys.argv[1], sys.argv[2]
body = json.dumps({
    "model": model,
    "messages": [{"role": "user", "content": open(path).read()}],
}).encode()
request = urllib.request.Request(
    "https://api.anthropic.com/v1/messages/count_tokens",
    data=body,
    headers={
        "x-api-key": os.environ["ANTHROPIC_API_KEY"],
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
    },
)
print(json.load(urllib.request.urlopen(request))["input_tokens"])
PY
    else
        # Deliberately crude, and labelled as such wherever it is printed.
        python3 -c "import sys; print(len(open(sys.argv[1]).read()) // 4)" "$file"
    fi
}

manual_tokens=$(count "$manual")
single_tokens=$(count "$single")

if [[ -n ${ANTHROPIC_API_KEY:-} ]]; then
    label="tokens, counted by $model"
else
    label="tokens, ESTIMATED at 4 chars each (set ANTHROPIC_API_KEY to count for real)"
fi

printf '\n%s\n\n' "$label"
printf '  shell, %d commands   %6d   %s\n' 4 "$manual_tokens" "$manual"
printf '  skep, one call       %6d   %s\n' "$single_tokens" "$single"
if ((single_tokens > 0)); then
    printf '\n  %sx cheaper\n\n' "$(python3 -c "print(f'{$manual_tokens / $single_tokens:.1f}')")"
fi
