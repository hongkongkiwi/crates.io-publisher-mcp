import subprocess
import json
import sys
import os

def check_stdio():
    # Path to the executable
    executable = "./target/debug/cratesdotio-mcp"
    
    if not os.path.exists(executable):
        print(f"Executable not found at {executable}. Please build first.")
        return

    print(f"Starting {executable} in stdio mode...")
    
    # Start the process
    process = subprocess.Popen(
        [executable],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr, # let logs go to stderr
        text=True,
        bufsize=0 # Unbuffered for easier interaction? actually line buffered is better for json
    )

    # 1. Initialize
    init_request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test-script", "version": "1.0"}
        }
    }
    
    print("Sending initialize...")
    process.stdin.write(json.dumps(init_request) + "\n")
    process.stdin.flush()
    
    # Read response
    response_line = process.stdout.readline()
    print(f"Received: {response_line}")
    
    if not response_line:
        print("No response received.")
        return

    # 2. Initialized Notification
    notify_payload = {
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }
    process.stdin.write(json.dumps(notify_payload) + "\n")
    process.stdin.flush()

    # 3. Call Tool
    tool_payload = {
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "get_crate_info",
            "arguments": {
                "name": "optimization-solvers"
            }
        }
    }
    
    print("Sending tool call...")
    process.stdin.write(json.dumps(tool_payload) + "\n")
    process.stdin.flush()
    
    # Read response
    response_line = process.stdout.readline()
    print(f"Tool Response: {response_line}")
    
    process.terminate()

if __name__ == "__main__":
    check_stdio()
