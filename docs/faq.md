# Frequently Asked Questions

## Configuration Reloads and Connections

### Does configuration reload use separate master and worker processes, like Nginx?

No. Bifrost Bridge remains a single operating-system process with one Tokio runtime. Its in-process
supervisor owns the listening socket and replaces the proxy configuration generation when it
handles a reload request. On Unix, `bifrost-bridge --reload` sends `SIGHUP` to the process recorded
in the PID file.

The new configuration and TLS material are prepared before the active generation is replaced. The
listening socket stays open: new connections are handled by the new generation, while already
accepted connections can continue using the generation that accepted them. These generations are
not separate processes and do not provide process-level isolation.

### What happens to an SSE connection accepted before a reload?

The SSE stream continues on its existing connection using the old generation. When the connection
closes, its connection task finishes and releases its references to that generation. The old
generation's remaining connection-specific resources are dropped once no active connections still
refer to them. This does not interrupt the server or affect connections handled by the current
generation.

An SSE response ending does not necessarily close the underlying TCP connection: HTTP keep-alive
may leave that connection open. In that case, some old-generation state can remain until the TCP
connection closes. Long-lived SSE or WebSocket connections can therefore keep resources from an old
generation alive for an extended time. Bifrost does not currently force-close old connections after
a drain timeout. Freed allocations become available to the process allocator, but the process's
reported resident memory may not decrease immediately.

If an SSE client reconnects after its previous connection closes, the reconnect is a new connection
and is handled by the current generation.
