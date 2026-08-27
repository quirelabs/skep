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

# The baseline is a picture of a real machine, which is the point, but a real
# machine carries a name, an address, and occasionally a secret in a process
# argument. Those go before the file is published or counted, and the header
# says exactly what went, so the transcript can still be trusted as evidence.
python3 - "$manual" <<'REDACT'
import getpass, re, socket, sys

path = sys.argv[1]
text = open(path).read()
user = getpass.getuser()
host = socket.gethostname().split(".")[0]
notes = []


def swap(pattern, replacement, label):
    global text
    text, count = re.subn(pattern, replacement, text)
    if count:
        notes.append(f"#   {label} ({count})")


swap(re.escape(user), "you", "username -> you")
if host and host.lower() != "localhost":
    swap(re.escape(host) + r"\b", "this-mac", "hostname -> this-mac")
swap(
    r"\b(?!127\.|0\.0\.0\.0)(?:\d{1,3}\.){3}\d{1,3}\b",
    "[redacted]",
    "addresses outside loopback -> [redacted]",
)
# Link local v6 addresses carry an interface identifier, so they name the
# machine's hardware as surely as a MAC does.
swap(
    r"\[(?!::1\])[0-9a-fA-F:]{2,}(?:%[A-Za-z0-9]+)?\]",
    "[redacted]",
    "v6 addresses outside loopback -> [redacted]",
)
swap(r"\b(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}\b", "[redacted]", "hardware addresses -> [redacted]")
# Process arguments are where machines leak: anything that looks like a URL,
# a query string or a long opaque token loses that argument, not the line.
swap(r"\S*(?:https?://|[?&](?:token|key|secret|password|sig)=)\S*", "[redacted]", "arguments carrying urls or credentials -> [redacted]")
swap(r"\b(?:eyJ|sk-|ghp_|gho_|xox[abprs]-)[A-Za-z0-9._-]{8,}", "[redacted]", "credential-shaped strings -> [redacted]")

header = [
    "# Real output from the four commands below, with identifying details",
    "# replaced before publishing:",
]
header += notes or ["#   nothing needed replacing"]
header += [
    "# The list of processes itself is unchanged: it is the point of the",
    "# comparison, and it is what an agent would actually have to read.",
    "",
]
open(path, "w").write("\n".join(header) + text)
REDACT

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
