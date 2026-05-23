# Platform Support

| Platform        | Binary type | Status              |
|-----------------|-------------|---------------------|
| Linux x86_64 (glibc) | Dynamic | Supported      |
| Linux x86_64 (musl)  | Static  | Supported (v0.5.0+) |
| macOS arm64     | Dynamic     | Supported           |
| Windows         | —           | Unguaranteed (tracking) |

`tyra build --static` is only supported on musl Linux (Alpine or similar).
glibc static linking is unsupported (breaks getaddrinfo/NSS).

See README.md § Platform support for full details.
