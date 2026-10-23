#!/bin/bash
# $1 mode, $2 cores for server (taskset list), $3 workers, $4 size MB, $5 port
BIN="$(cd "$(dirname "$0")" && pwd)/target/release"
taskset -c $2 $BIN/burst_server $1 $3 $4 $5 2>/dev/null &
PID=$!
for i in $(seq 1 100); do curl -s localhost:$5/stats >/dev/null && break; sleep 0.2; done
echo "### mode=$1 server-cores=$2 workers=$3 file=${4}MB"
for n in 20 60 120; do taskset -c 2,3 $BIN/burst_client http://127.0.0.1:$5 $n get; done
taskset -c 2,3 $BIN/burst_client http://127.0.0.1:$5 120 revalidate
taskset -c 2,3 $BIN/burst_client http://127.0.0.1:$5 120 save 8000
kill $PID; wait $PID 2>/dev/null
