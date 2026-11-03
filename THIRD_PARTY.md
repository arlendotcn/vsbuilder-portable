# Prior art and third-party notices

## Reference implementations

This project is an independent Rust implementation. It does not include source
code from the projects below, but their manifest handling, package selection,
and CLI behavior were used as references during design and testing.

- **@mmozeiko / portable-msvc.py**
  - https://gist.github.com/mmozeiko/7f3162ec2988e81e56d5c4e22cde9977
  - Initial Python implementation for downloading a portable MSVC + Windows
    SDK tree from Visual Studio channel manifests.

- **Data-Oriented-House / PortableBuildTools**
  - https://github.com/Data-Oriented-House/PortableBuildTools
  - C implementation with a GUI and CLI for the same portable Build Tools
    workflow. Its README credits @mmozeiko's Python implementation.

The upstream repositories do not declare a license in their GitHub metadata.
No code from either reference project is vendored or copied here. If you
believe attribution is incomplete, please open an issue.

## Microsoft components

`vsbuilder` downloads MSVC Build Tools and Windows SDK payloads from Microsoft
Visual Studio channel manifests. Those payloads are not part of this
repository and remain subject to Microsoft's Build Tools license terms.

The tool verifies payload SHA-256 values from the Microsoft manifest. It does
not redistribute the downloaded payloads.

## Rust dependencies

Dependency versions and checksums are pinned in `Cargo.lock`. Their licenses
are declared in the crates.io metadata and should be reviewed when upgrading
dependencies.
