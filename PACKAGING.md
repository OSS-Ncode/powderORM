# Packaging & Release

Powder ships one Rust core plus bindings, published to each ecosystem's registry.
Releases are driven by a git tag: [.github/workflows/release.yml](.github/workflows/release.yml).

```bash
# bump the version in all three canonical manifests first (they must match the tag):
#   Cargo.toml                                 [workspace.package] version
#   crates/powder-node/package.json            "version"
#   crates/powder-python/pyproject.toml        version
git tag v0.1.0 && git push origin v0.1.0
```

`verify-version` fails the release if the tag and those manifests disagree.

## Registries

| Registry | Package | Status | Trigger |
|---|---|---|---|
| crates.io | `powder-core`, `powder-cli` | ✅ automated | tag `v*` |
| npm | `@powder/node` (+ platform pkgs) | ✅ automated | tag `v*` |
| PyPI | `powder` | ✅ automated (OIDC) | tag `v*` |
| NuGet | `Powder` | ⚙️ opt-in (`vars.ENABLE_NUGET`) | tag `v*` |
| Maven Central | `dev.powder:powder-java`, `:powder-kotlin` | ⚙️ opt-in (`vars.ENABLE_MAVEN`) | tag `v*` |
| Go modules | `github.com/OSS-Ncode/powderORM/bindings/go` | ✅ tag-only | see below |

## Required secrets / variables

Set under **Settings → Secrets and variables → Actions**.

| Name | Kind | For |
|---|---|---|
| `CARGO_REGISTRY_TOKEN` | secret | crates.io |
| `NPM_TOKEN` | secret | npm (automation token, publish rights to `@powder`) |
| — | — | PyPI uses **OIDC Trusted Publishing** (no secret) |
| `ENABLE_NUGET` | variable = `true` | turn on the NuGet jobs |
| `NUGET_API_KEY` | secret | NuGet |
| `ENABLE_MAVEN` | variable = `true` | turn on the Maven jobs |
| `OSSRH_USERNAME`, `OSSRH_PASSWORD` | secret | Sonatype / Central Portal |
| `MAVEN_SIGNING_KEY`, `MAVEN_SIGNING_PASSWORD` | secret | GPG signing (armored private key) |

**PyPI Trusted Publishing:** register a publisher for this repo + `release.yml` at
`https://pypi.org/manage/project/powder/settings/publishing/`, and create a GitHub
Environment named `pypi` (referenced by the publish job).

## Per-registry notes

### crates.io
`powder-core` (library) and `powder-cli` (the `powder` binary) have no internal
path dependencies, so each publishes standalone. `powder-ffi` / `powder-python`
are `cdylib`s and are **not** crates.io targets.

### npm (napi-rs)
`npm-build` cross-compiles prebuilds for macOS (arm64/x64), Windows (x64), and
Linux (x64). `napi prepublish` publishes the per-platform `@powder/node-<triple>`
packages that the main package lists as optionalDependencies. Extend the matrix
in `release.yml` for musl / linux-arm64 / win-arm64 when needed.

### PyPI (maturin, abi3)
`crates/powder-python` builds with pyo3 `abi3-py39`, so there's **one wheel per
platform** covering CPython 3.9+ (plus an sdist). No per-Python-version matrix.

### NuGet (C#)
`nuget-natives` builds `powder-ffi` for each RID and drops the shared library into
`bindings/csharp/Powder/runtimes/<rid>/native/`; `dotnet pack` bundles them so the
right native loads automatically (`POWDER_LIB` still overrides). A local
`dotnet build` with no natives present still works — the `<None>` items are
`Condition="Exists(...)"`.

### Maven Central (Java + Kotlin)
Gradle build lives in [jvm/](jvm/); it points its sourceSets at the existing
`crates/powder-java/java` and `bindings/kotlin/src`. It publishes **pure-JVM
jars** — the JNI native (`powder_java`) is shipped on the GitHub Release and
loaded via `Powder.loadLibrary(path)` / `POWDER_LIB`.

- `group` is `dev.powder`, which must be verified as a Maven Central namespace
  (prove ownership of `powder.dev`). If you don't own it, set `group` in
  [jvm/gradle.properties](jvm/gradle.properties) to `io.github.oss-ncode`, which
  Central auto-verifies from the GitHub repo.
- The build targets the OSSRH staging endpoint. Newer Central accounts publish via
  the **Central Portal**; if so, point `-PossrhUrl=` at the portal endpoint or
  adopt the `com.vanniktech.maven.publish` plugin.

### Go modules
Nothing to publish — the module proxy indexes a pushed tag. Because the module
lives in a subdirectory, tag it with the subdir prefix:

```bash
git tag bindings/go/v0.1.0 && git push origin bindings/go/v0.1.0
# consumers: go get github.com/OSS-Ncode/powderORM/bindings/go@v0.1.0
```

The native lib is loaded at runtime via `powder.Load(path)` / `POWDER_LIB`.

### C / C++
No central registry. Distributed as headers + the `powder-ffi` source; consumers
build `powder-ffi` and link. (vcpkg/Conan ports are a possible future addition.)

## Deferred / follow-ups

- **Internal Java package rename `com.powder → dev.powder`.** Cosmetic only —
  groupId `dev.powder` publishes fine with the `com.powder` package as-is. Doing
  the rename means updating the JNI symbol names in
  [crates/powder-java/src/lib.rs](crates/powder-java/src/lib.rs)
  (`Java_com_powder_PowderNative_*` → `Java_dev_powder_*`), moving the `java/com/powder`
  files, and updating the Kotlin `import com.powder.*`. Do it with a JVM + cargo
  build in the loop to verify — it is not safe to do blind.
- **Self-contained JVM / native auto-extraction.** Bundle `powder_java` in jar
  resources and extract+load at runtime so consumers don't need `POWDER_LIB`.
- **linux-arm64 / musl** prebuilds for npm and NuGet.
