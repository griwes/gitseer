# Third-party Dependencies

Gitseer is licensed under Apache-2.0. Its Rust dependencies are resolved by
`Cargo.lock` and retain their respective upstream licenses; no dependency
source is vendored in this repository.

`scripts/ci/check-licenses.py` evaluates every package returned by
`cargo metadata --locked`, including target-specific transitive packages. A
package passes only when its SPDX expression offers a complete path composed of
the approved permissive licenses and exceptions below:

- Apache-2.0
- MIT
- ISC
- Zlib
- Unicode-3.0
- CC0-1.0
- Unlicense
- LLVM-exception when attached to an approved license

Expressions with an incompatible alternative still pass when an approved
alternative is available; conjunctions pass only when every required license
is approved. Packages without license metadata fail the audit. Dependency
updates must pass this check and be reviewed for notice or source-distribution
requirements before release.

