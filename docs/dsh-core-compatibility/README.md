# DSH Core Compatibility Evolution

## Summary

This directory records how Deepseek Harness Desktop follows breaking DSH releases. The index owns the stable compatibility architecture; one version-named record owns the upstream changes, observed conflicts, responsibility analysis, implementation decisions, verification evidence, and remaining debt for each supported core.

## Table of Contents

- [Purpose](#purpose)
- [Architecture](#architecture)
- [Compatibility ownership](#compatibility-ownership)
- [Version records](#version-records)
- [Adaptation method](#adaptation-method)
- [Acceptance standard](#acceptance-standard)
- [Long-term direction](#long-term-direction)

## Purpose

Desktop treats DSH as an independently evolving product, not as an implementation detail to freeze or patch in place. An adaptation follows the public contracts and security model of the target core, keeps the official core artifact immutable, and records enough evidence for another maintainer or coding agent to reproduce the decision.

Each version record answers six questions: what changed upstream, which Desktop or plugin assumptions conflicted, why each conflict occurred, which work was unavoidable, which work exposed Desktop architecture debt, and which architectural change reduces the cost of the next update.

## Architecture

1. `dependencies/cores/<tag>` stores one immutable official or locally packaged DSH artifact; `dependencies/active-core` selects a slot without copying or editing it.
2. `product-zlzhg` starts from the official Web bundles and keeps user configuration separate from Desktop compatibility behavior.
3. `src-tauri/resources/core-compatibility.json` is the single version-capability table consumed by runtime selection, plugin preparation and the surface matrix; `service/core_compatibility.rs` rejects versions absent from it.
4. Desktop-managed plugins persist as logical selections. Core switching projects those selections into exact version-named internal and preset artifacts, then runs a compatibility gate before launch.
5. The shell consumes a stable runtime result. Workflow owns processes and ports; React does not branch on DSH versions.
6. Machine-readable diagnostics and a real-frame surface matrix verify the boot graph, settings pages, lazy resources, and sidebar contributions without relying on manual clicking.

## Compatibility ownership

The core owns CLI syntax, readiness output, authentication, Host and Origin policy, public package exports, Loader semantics, slot declarations, RPC protocols, and durable session vocabulary. Desktop adapts to those contracts and must not weaken them to preserve an older integration.

Desktop owns core selection, process supervision, WebView navigation, product profile composition, plugin artifact selection, failure presentation, rollback, and compatibility diagnostics. A failure in these areas is a Desktop defect even when an upstream change exposed it.

Plugin authors own imports from public DSH packages, declared peer dependencies, slot contribution timing, and feature behavior. Desktop can carry a versioned artifact while an upstream plugin release catches up, but a compiled-JavaScript rewrite is transitional debt rather than a permanent compatibility mechanism.

## Version records

| Target core | Web launch protocol | Plugin artifact set | Result | Record |
| --- | --- | --- | --- | --- |
| `0.1.2-alpha.1` | `token-cookie-v1` | `dsh-v0.1.2-alpha.1` | Accepted with retained `0.1.1-rc.2` and recorded prerelease session limitation | [Adaptation record](./v0.1.2-alpha.1.md) |

New records use `v<core-version>.md` and `v<core-version>.zh.md`. A record is immutable after its compatibility implementation ships except for factual corrections and links to a superseding record; later releases receive new files so architectural progress remains visible.

## Adaptation method

1. Build and run the clean official target profile without Desktop plugins. Record its CLI arguments, stdout readiness signal, authentication exchange, Cookie policy, trusted-host source, browser Host-privilege decision, boot graph, settings namespaces, public client packages, slot declarations, RPC routes, and persisted-session behavior.
2. Compare those observations with the newest accepted record. Classify each difference as an upstream contract change, a Desktop-owned integration defect, a plugin-owned incompatibility, or an intentional prerelease limitation.
3. Add one exact core record and compose it from reusable protocol capabilities. Do not use open semver ranges or spread version conditions through workflow, React, profiles, or plugins.
4. Rebuild Desktop-managed plugins from pinned sources into the exact version-named artifact set. Prefer source fixes and upstream releases; isolate any temporary artifact transformation and record its removal condition.
5. Run static compatibility checks before launch, then execute the same runtime matrix against the new core and every retained core.
6. Record measured evidence and remaining debt in the version file. Do not mark a release accepted from screenshots or a shell-only startup.

## Acceptance standard

A core is accepted only when the clean official profile starts, the Desktop product profile reaches a live Loader state, all declared managed plugins join the real boot graph, all declared lazy resources return success, settings and sidebar surfaces open without known failure signatures, failed operations preserve the shell, and switching to every retained core does not crash Desktop.

`dsh-desktop-diagnostics snapshot` reports the selected slot, package version, source provenance, profile compatibility, and the latest frame reports. `pnpm diagnostics:select-core <version>` switches an installed test target through Tauri IPC and waits for health; `pnpm diagnostics:surface` then traverses the declared real-frame surfaces and exits nonzero on a failed assertion. Core packages and each plugin retain responsibility for their own domain mutations and end-to-end behavior.

## Long-term direction

Adaptation cost should converge toward three bounded tasks: describe the new core protocol, build plugins against its public packages, and extend the compatibility matrix. Repeated edits to process lifecycle, React state, user profiles, or installed core files indicate an architecture defect and require a structural correction in the same adaptation.

Desktop now rejects every core without an exact tested record and exposes its Web launch protocol, client ABI, slot protocol, session format and plugin artifact set through diagnostics. The intended next step is an upstream packaged capability manifest carrying these declarations; the exact version record remains provenance and the acceptance-matrix key.
