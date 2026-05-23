# Platform Support

> This file is bundled in every release artifact for quick reference.
> **README.md § Platform support** is the authoritative source of truth.

| Platform        | Binary type | Status              |
|-----------------|-------------|---------------------|
| Linux x86_64 (glibc) | Dynamic | Supported      |
| Linux x86_64 (musl)  | Static  | Supported (v0.5.0+) |
| macOS arm64     | Dynamic     | Supported           |
| macOS x86_64 (Intel) | —      | Unguaranteed (tracking) |
| Windows         | —           | Unguaranteed (tracking) |

`tyra build --static` is only supported on musl Linux (Alpine or similar).
glibc static linking is unsupported (breaks getaddrinfo/NSS).

## Release artifact names

| Artifact | Platform |
|---|---|
| `tyra-<version>-linux-x86_64.tar.gz` | Linux x86_64 glibc (dynamic) |
| `tyra-<version>-linux-musl-x86_64-static.tar.gz` | Linux x86_64 musl (static) |
| `tyra-<version>-macos-arm64.tar.gz` | macOS arm64 (dynamic) |

See README.md § Platform support for full details.
