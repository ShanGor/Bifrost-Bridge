#!/usr/bin/env python3
"""
FastAPI server for integration testing of reverse proxy with multiple targets and sticky sessions.

This server can be started multiple times to simulate different backend targets.
It will try ports starting from 9001, 9002, etc., and use the first available one.
"""

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
import socket
import sys
from typing import Optional

app = FastAPI()


def find_available_port(start_port: int = 9001, max_attempts: int = 10) -> Optional[int]:
    """Find an available port starting from start_port."""
    for i in range(max_attempts):
        port = start_port + i
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            try:
                s.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    return None


def get_server_identity(port: int) -> str:
    """Get server identity based on port for easier testing."""
    port_to_id = {9001: "api-a", 9002: "api-b"}
    return port_to_id.get(port, f"api-{port}")


@app.get("/")
@app.post("/")
@app.get("/{path:path}")
@app.post("/{path:path}")
async def handle_request(request: Request, path: str = ""):
    """Handle all requests and print details for testing."""
    x_user_id = request.headers.get("X-User-Id", "NOT_SET")

    # Get request method and endpoint
    method = request.method
    endpoint = f"/{path}" if path else "/"

    # Get payload for POST requests
    payload = None
    if method == "POST":
        try:
            body = await request.body()
            payload = body.decode("utf-8") if body else None
        except Exception:
            payload = "<error reading body>"

    # Print request details
    print(f"\n{'='*60}")
    print(f"Server: {get_server_identity(server_port)} (listening on port {server_port})")
    print(f"Method: {method}")
    print(f"Endpoint: {endpoint}")
    print(f"X-User-Id: {x_user_id}")
    if payload:
        print(f"Payload: {payload}")
    print(f"{'='*60}\n")

    # Return response with server info
    return JSONResponse(
        {
            "server": get_server_identity(server_port),
            "port": server_port,
            "endpoint": endpoint,
            "x_user_id": x_user_id,
            "path": path,
        }
    )


def main(start_port: int = 9001):
    """Start the server with the specified or auto-detected port."""
    global server_port

    # Check if a specific port was provided
    if len(sys.argv) > 1:
        try:
            start_port = int(sys.argv[1])
        except ValueError:
            print(f"Invalid port argument: {sys.argv[1]}")
            print(f"Usage: python {sys.argv[0]} [port]")
            sys.exit(1)

    # Find available port
    server_port = find_available_port(start_port)
    if server_port is None:
        print(f"Error: Could not find an available port starting from {start_port}")
        sys.exit(1)

    server_identity = get_server_identity(server_port)
    print(f"\n{'='*60}")
    print(f"Starting {server_identity} server on port {server_port}")
    print(f"Listening on http://127.0.0.1:{server_port}")
    print(f"{'='*60}\n")

    import uvicorn
    uvicorn.run(app, host="127.0.0.1", port=server_port)


if __name__ == "__main__":
    main()
