#!/usr/bin/env bash
set -euo pipefail

PORT=3799
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RUST_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_DIR="$(cd "$RUST_DIR/.." && pwd)"
BINARY="$RUST_DIR/target/release/taskcast"

echo "=== Building Rust binary ==="
cd "$RUST_DIR"
cargo build --release -p taskcast-cli

echo "=== Starting Rust server on port $PORT ==="
$BINARY start --port $PORT &
SERVER_PID=$!

# Wait for server to be ready
for i in {1..30}; do
    if curl -s "http://localhost:$PORT/tasks" > /dev/null 2>&1; then
        echo "Server ready!"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "Server failed to start"
        kill $SERVER_PID 2>/dev/null || true
        exit 1
    fi
    sleep 0.1
done

# Cleanup function
cleanup() {
    echo "=== Stopping server ==="
    kill $SERVER_PID 2>/dev/null || true
    wait $SERVER_PID 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Running integration tests ==="
cd "$ROOT_DIR"

# Run the integration test
TASKCAST_TEST_URL="http://localhost:$PORT" npx vitest run --config rust/tests/vitest.config.ts --reporter=verbose

echo "=== All integration tests passed! ==="
