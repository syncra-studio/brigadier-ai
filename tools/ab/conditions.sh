#!/bin/bash
# conditions.sh <meter-data-dir> : quota (from a dev daemon kept running only to read it),
# thermal, memory and load, now. One reading per arm start and end.
HERE=$(cd "$(dirname "$0")" && pwd)
echo "time: $(date '+%F %T')"
MAXC=1000000 python3 "$HERE/bipc.py" "${1:?meter data dir}" '{"method":"getUsage"}' | python3 -c "
import json,sys; d=json.load(sys.stdin)['value']['usage']
for p in d['providers']:
  q=p.get('quota') or {}
  print('quota', p['provider'], [(w['window']['id'], w['window']['usedPercent'], w['window'].get('resetsAtMs')) for w in q.get('windows',[])])"
pmset -g therm | grep -v '^$' | sed 's/^/therm: /'
memory_pressure -Q 2>/dev/null | tail -1 | sed 's/^/mem: /'
sysctl -n vm.loadavg | sed 's/^/load: /'
