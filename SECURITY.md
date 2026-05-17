# Security Policy

Thank you for taking the time to help keep Tyra and its users safe.

## Project status

Tyra v0.1.0 is an initial release suitable for small CLI tools, learning, and language evaluation. It is not recommended for production services. Known limitations (string GC, experimental `http.server`, no package manager) are documented in the README.

We take security reports seriously and will respond to them.

## Reporting a vulnerability

**Do not report security issues through public GitHub issues, pull requests, or discussions.**

Instead, please report vulnerabilities through one of these channels:

### Preferred: GitHub Security Advisories

Use the "Report a vulnerability" button on the [Security tab](https://github.com/tyra-lang/tyra/security/advisories/new) of this repository. This creates a private advisory visible only to maintainers.

### Alternative: Email

Send a report to **mizumoto@andgenie.jp**.

For sensitive reports, you may encrypt your message with the maintainer's PGP key (fingerprint published in `SECURITY.asc` when available).

## What to include

A useful report includes:

- **Description**: what the vulnerability is and what it affects
- **Impact**: what an attacker could do (code execution, data disclosure, denial of service, etc.)
- **Reproduction steps**: minimal code, configuration, or commands that demonstrate the issue
- **Environment**: Tyra version, OS, LLVM version, and any other relevant context
- **Suggested fix** (optional): if you have ideas for how to address it
- **Disclosure preference**: whether you want to be credited and how

We do not require you to use a specific format. Clear and reproducible is more important than formal.

## Response timeline

Because this is a side project, response times are best-effort:

| Step | Target |
| --- | --- |
| Acknowledge receipt | Within 5 business days |
| Initial assessment | Within 14 days |
| Fix or mitigation plan | Within 30 days for high-severity issues |
| Public disclosure | Coordinated with reporter (typically 90 days max) |

If a vulnerability is being actively exploited in the wild, we will accelerate this timeline.

## Scope

The following are **in scope** for security reports:

- Bugs in the Tyra compiler that allow:
  - Arbitrary code execution during compilation
  - Reading or writing files outside the project directory unexpectedly
  - Network access during compilation when not requested
- Bugs in the Tyra runtime that allow:
  - Memory corruption from safe Tyra code
  - Bypassing the type system from safe Tyra code
  - Denial of service from well-formed inputs
- Bugs in the standard library that violate documented safety guarantees
- Issues in build infrastructure that could compromise releases (signed tags, release binaries)

The following are **out of scope**:

- Vulnerabilities in `stdlib/http.server` — it is explicitly marked **experimental** and not production-safe (no TLS, single-threaded, no authentication middleware). Do not expose it to untrusted networks.
- Issues that require an attacker to already have write access to the source code being compiled
- Bugs that require modified versions of the compiler or runtime
- Memory safety issues in code that uses planned future FFI features (FFI is inherently unsafe)
- Compiler crashes on malformed input that do not allow code execution (these are bugs, but report as regular issues)
- Vulnerabilities in third-party dependencies (report to the dependency upstream; we will update once they release a fix)
- Social engineering attacks on contributors or users
- Issues in old or unsupported versions

If you are unsure whether something is in scope, report it anyway. We would rather review extra reports than miss real issues.

## Supported versions

| Version | Security support |
| --- | --- |
| Pre-release (current) | Best effort, no guarantees |
| v0.1.x (when released) | Yes, until v0.2 ships |
| v1.0+ (future) | Defined when v1.0 is released |

Security patches will be released as new patch versions (e.g., `v0.1.0` → `v0.1.1`). When a security fix is released, the corresponding GitHub release notes and CHANGELOG will mark it as a security update.

## Disclosure policy

We follow **coordinated disclosure**:

1. You report the vulnerability privately
2. We acknowledge and investigate
3. We develop a fix and prepare a release
4. We coordinate a public disclosure date with you
5. We release the fix and publish a security advisory simultaneously
6. After disclosure, the advisory is public on GitHub

We aim to disclose within 90 days of the report. Earlier disclosure may happen if:

- You request it
- The vulnerability becomes public independently
- A fix is straightforward and low-risk

We will credit reporters in the security advisory unless you prefer to remain anonymous.

## Out-of-scope: AI-generated code

Tyra is designed for human-AI collaboration. If you discover that AI-generated Tyra code commonly produces vulnerable patterns, this is **interesting but not a security vulnerability in Tyra itself**. Please report such findings as a regular issue with the `ai-pattern` label so we can consider whether the language design contributes to the problem.

If, however, the Tyra compiler itself produces unsafe machine code from safe-looking Tyra source, that is in scope.

## Bug bounty

We do not currently offer a bug bounty program. Tyra is a volunteer project. We will publicly acknowledge security researchers in the security advisory and the release notes for the fix.

## Hall of fame

Researchers who have responsibly disclosed vulnerabilities will be listed here once the project has received its first reports.

---

Thank you for helping keep Tyra secure.
