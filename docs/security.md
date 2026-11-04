# Security and privacy

## Data flow

```text
Local Windows desktop output → DXGI frame → in-memory tiles → caller-owned bytes
```

LX Capture does not write files, transmit pixels, capture audio, or run a background service. The caller decides whether bytes are displayed, encoded, stored, or sent elsewhere.

## Sensitive screen contents

Captured pixels can include credentials, private messages, documents, or other sensitive information. Capture only with appropriate authorization. Keep resulting frames in memory only as long as needed, and protect any recording or transport managed by the calling application.

## Platform boundary

The DXGI/D3D11 interop is the only unsafe area. The code checks output rectangles, mapped pointers, row pitch, and buffer lengths before copying pixels. A frame lease releases the DXGI frame through `Drop` on success and error paths. Windows, graphics drivers, and the local logged-in desktop remain trusted platform components.

For a vulnerability report, follow the private process in the root [Security Policy](../SECURITY.md). Dependency license information is in [Third-party notices](../THIRD-PARTY-NOTICES.md).
