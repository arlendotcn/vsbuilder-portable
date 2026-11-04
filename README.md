# vsbuilder-portable

Portable, reproducible MSVC Build Tools / Windows SDK package builder for Windows x64.

**Status:** v1.0.3 · [Repository](https://github.com/arlendotcn/vsbuilder-portable) · [Releases](https://github.com/arlendotcn/vsbuilder-portable/releases)

This builds a portable MSVC + Windows SDK tree from Microsoft Visual Studio
channel manifests. It downloads and verifies payloads, extracts the toolchain,
generates `pkg.json` / `devcmd.bat` / `devcmd.ps1` / `arpkg`, and validates the candidate.

It is **not** an installer and never modifies an active package, PATH, or the
registry.

## Download

Download the latest `vsbuilder.exe` from GitHub Releases. To build from source,
see [Build](#build).

## Commands

```text
vsbuilder list  [--vs 2022|2026|2019] [--channel stable|insiders|lts] [--json]
vsbuilder build [--channel stable|insiders|lts] [--msvc latest] [--sdk latest] [--out <empty-dir>] [--accept-license] [--dry-run]
vsbuilder check <dir> [--smoke] [--json]
```

### Channels

`--channel` defaults to `stable`.

| Channel | VS 2019 | VS 2022 | VS 2026 |
|---|---|---|---|
| `stable` | `vs/16/release` | `vs/17/release` | `vs/18/stable` |
| `insiders` | `vs/16/pre` | `vs/17/pre` | `vs/18/insiders` |
| `lts` | not available | not available | not yet published |

`--preview` is retained as a deprecated alias for `--channel insiders`.

`lts` is accepted as a value but currently returns a clear error: Microsoft has
not published an LTSC channel URL for the compiler workflow yet.

### list

```bat
vsbuilder list
vsbuilder list --json
```

MSVC versions are printed as package version / package stem. `build` also
accepts the minor (`14.44`), the full package version (`14.44.35229`), or the
package stem (`14.44.17.14`).

The Windows SDK can be selected by short version (`26100`) or the manifest
package version (`10.0.26100.15`).

### build

```bat
vsbuilder build
```

With no options, `build` uses the latest stable MSVC and Windows SDK and writes
to `<exe-dir>\msvc-2022-<msvc>-sdk<sdk>` using the resolved versions. It prompts for the Microsoft Build Tools license when
stdin is a TTY; non-interactive use requires `--accept-license`.

Explicit selection:

```bat
vsbuilder build ^
  --msvc 14.44 ^
  --sdk 26100 ^
  --out .build\msvc-next ^
  --accept-license
```

- `--msvc` and `--sdk` default to `latest`; `list` shows what that resolves to.
- `--out` defaults to `<exe-dir>\msvc-<vs>-<msvc>-sdk<sdk>` (for example `msvc-2022-14.44.35229-sdk10.0.26100.15`) and must be missing or empty; no `--force` is provided.
- The candidate is built in a temporary sibling directory and only renamed to
  `--out` after validation succeeds.
- `--dry-run` resolves and prints the plan without downloading or writing.
- Without `--accept-license`, interactive builds prompt for the Microsoft
  Build Tools license; non-interactive builds require the flag. `list` and
  `--dry-run` do not require it.
- Cache is always `<exe-dir>\cache`; the tool remains green/portable and reuses payloads across invocations. The directory must be writable.
- `--refresh` (global) ignores cached manifests. Manifests are reused for 6 hours by default; a stale manifest is used as fallback if refresh fails.
- `--proxy` (global) sets an explicit HTTP proxy.

The selected package version is not necessarily the compiler's actual
directory version. For example, package `14.44.35229` currently extracts
`VC\Tools\MSVC\14.44.35207`. The generated `pkg.json` and `BUILD-INFO.json`
record the actual extracted version.

### check

```bat
vsbuilder check .build\msvc-next
vsbuilder check .build\msvc-next --smoke
vsbuilder check .build\msvc-next --json
```

`check` is read-only. It validates `pkg.json`, `BUILD-INFO.json`, `cl.exe`,
SDK paths, `devcmd` files, the `arpkg` engine files, `PAYLOAD-MANIFEST.tsv`,
the payload tree SHA-256, and the paths referenced by `INCLUDE` / `LIB`.
`--smoke` additionally runs `cl.exe /Bv`.

## Output

```text
<out>\
├─ pkg.json
├─ BUILD-INFO.json
├─ PAYLOAD-MANIFEST.tsv
├─ arpkg.cmd
├─ arpkg.ps1
└─ payload\
   ├─ VC\
   ├─ Windows Kits\
   ├─ devcmd.bat
   └─ devcmd.ps1
```

`BUILD-INFO.json` records the channel, license URL, requested selectors,
selected package IDs, actual versions, every downloaded payload's file name,
URL and SHA-256, the `arpkg` engine hashes, and the final payload tree SHA-256.

`PAYLOAD-MANIFEST.tsv` lists every payload file as:

```text
<sha256>	<size>	<relative-path>
```

The candidate is a complete package layout: `arpkg.cmd` / `arpkg.ps1` support
standalone `status`, `doctor`, `register` and `unregister`.

## Bare Windows

The release binary is linked with the static MSVC CRT:

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Verified with `dumpbin /dependents`: no `VCRUNTIME140.dll` or `MSVCP140.dll`.
Only system DLLs are required. The tool uses `msiexec` for Windows SDK MSI
extraction; no Python, PowerShell, Visual Studio, or VC++ Redistributable is
required to run `list`, `build`, or the default `check`.

## Build

From an environment with Rust 1.99.1 and an MSVC toolchain:

```bat
cargo build --release
```

Expected output:

```text
target\x86_64-pc-windows-msvc\release\vsbuilder.exe
```

## Design boundaries

- `vsbuilder` is a builder, not an installer: it does not modify PATH,
  registry, or an active package.
- No implicit `latest`: `build` requires explicit `--msvc` and `--sdk`.
- No destructive overwrite: the output directory must be empty.
- No hidden global state: manifests and payloads live under `<exe-dir>\cache`.
- No GUI and no interactive prompts.

## Trust model

- Channel and BuildTools manifests are fetched over HTTPS.
- Every downloaded payload is verified against the SHA-256 in the Microsoft
  manifest; a mismatch fails the build.
- The manifest itself is authenticated by TLS, not by a separate signature.
- Windows SDK extraction invokes the Microsoft `msiexec /a` administrative
  install on the downloaded MSIs.
- The release binary is not Authenticode-signed. Verify the SHA-256 published
  with each release.
- `--proxy` is explicit; system proxy auto-detection is not performed.

## Credits

This is an independent Rust implementation. The channel/package selection
workflow was informed by these prior-art projects:

- [@mmozeiko / portable-msvc.py](https://gist.github.com/mmozeiko/7f3162ec2988e81e56d5c4e22cde9977)
- [Data-Oriented-House / PortableBuildTools](https://github.com/Data-Oriented-House/PortableBuildTools)

No source code from either project is included. See
[THIRD_PARTY.md](THIRD_PARTY.md) for details.

## License

MIT. See [LICENSE](LICENSE).

Downloaded MSVC and Windows SDK payloads are not part of this repository and
remain subject to Microsoft's Build Tools license.

## Tested

- Windows 11 x64
- Rust 1.99.1, MSVC target, static CRT
- VS 2022 release: full build, payload tree hash, `check`, `check --smoke`
- VS 2022: MSVC selected `14.44.35229`, extracted `14.44.35207`
- VS 2022: Windows SDK selected `26100`, extracted `10.0.26100.0`
- Candidate package: `arpkg status`, `doctor`, `register --user` and
  `unregister --user` validated
- VS 2026 release: `list` and `build --dry-run` validated; latest resolves to
  MSVC `14.52.36815` and Windows SDK `10.0.28000.5`
- VS 2019: not yet validated









