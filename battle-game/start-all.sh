#!/bin/bash
cd "$(dirname "$0")"
echo "🎮 Battle Royale — Starting Both Servers"
lsof -ti :3000 | xargs kill 2>/dev/null
lsof -ti :3001 | xargs kill 2>/dev/null
sleep 1
node server.js &
PID1=$!
echo "✅ Server 1 PID: $PID1 | http://localhost:3000 (完整版)"
node server2.js &
PID2=$!
echo "✅ Server 2 PID: $PID2 | http://localhost:3001 (精简版)"
echo ""
echo "🎯 Both running! Press Ctrl+C to stop"
trap "kill $PID1 $PID2 2>/dev/null" EXIT
wait
