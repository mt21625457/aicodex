#!/bin/bash
cd "$(dirname "$0")"
echo "🎮 Battle Royale Server"
echo "======================="
PORT=${1:-3000}
if lsof -i :$PORT &>/dev/null; then
  echo "⚠️  Port $PORT in use. Killing..."
  lsof -ti :$PORT | xargs kill 2>/dev/null
  sleep 1
fi
node server.js &
PID=$!
echo "✅ Server PID: $PID"
echo "🌐 http://localhost:$PORT"
echo "📋 Press Ctrl+C or run: kill $PID"
trap "kill $PID 2>/dev/null; echo 'Server stopped'" EXIT INT TERM
wait $PID
