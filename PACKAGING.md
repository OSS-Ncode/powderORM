# Packaging & Release

Powder ships one Rust core plus bindings. A git tag drives
[.github/workflows/release.yml](.github/workflows/release.yml).

```bash
# bump the version in all three canonical manifests first (they must match the tag):
#   Cargo.toml                                 [workspace.package] version
#   crates/powder-node/package.json            "version"
#   crates/powder-python/pyproject.toml        version
git tag v0.1.0 && git push origin v0.1.0
```

`verify-version` fails the release if the tag and those manifests disagree.

## What a tag does today

**By default a tag only builds artifacts and creates a GitHub Release** — it does
**not** publish to any registry. The release carries, for every platform
(macOS arm64/x64, Linux x64, Windows x64):

- `powder-native-<platform>.tar.gz` — the native libraries `powder_ffi`
  (Go / C / C++ / C#) and `powder_java` (Java / Kotlin), loaded at runtime via
  `POWDER_LIB` / `loadLibrary`.
- `*.whl` — Python abi3 wheels (+ an sdist).
- `*.node` — the napi prebuilds for `@powder/node`.

You can also trigger it manually (`workflow_dispatch`) to build without tagging.

## Registry publishing — opt-in, do it later

Every registry job is **off until you set a repo variable** (Settings → Secrets
and variables → Actions → *Variables*). Flip the variable and add the matching
secrets — no workflow edits needed.

| Registry | Package | Enable with | Auth |
|---|---|---|---|
| crates.io | `powder-orm`, `powder-orm-cli`, `powder-orm-studio` | `ENABLE_PUBLISH=true` | **OIDC** (no secret) |
| npm | `powder-orm` (+ platform pkgs) | `ENABLE_PUBLISH=true` | **OIDC** (no secret) |
| PyPI | `powder-orm` | `ENABLE_PUBLISH=true` | **OIDC** (no secret) |
| NuGet | `PowderORM` | `ENABLE_NUGET=true` | **OIDC** + variable `NUGET_USER` |
| Maven Central | `io.github.oss-ncode:powder-java`, `:powder-kotlin` | `ENABLE_MAVEN=true` | secrets `MAVEN_CENTRAL_USERNAME`, `MAVEN_CENTRAL_PASSWORD`, `MAVEN_SIGNING_KEY`, `MAVEN_SIGNING_PASSWORD` |
| Go modules | `github.com/OSS-Ncode/powder-orm/bindings/go` | — (tag only) | see below |

Four registries use **OIDC Trusted Publishing** — the job requests
`id-token: write` and the registry returns a short-lived credential, so there's no
long-lived token to store. Configure the trusted publisher once on each registry,
matching this repo + `release.yml`:

- **crates.io** (`rust-lang/crates-io-auth-action` exchanges the token): crates.io
  → each crate → Settings → *Trusted Publishing* → GitHub, owner `OSS-Ncode`,
  repo `powder-orm`, workflow `release.yml`. Do it for `powder-core` **and**
  `powder-cli`. A brand-new crate needs one initial token publish first, then OIDC.
- **npm** (job upgrades to npm ≥ 11.5.1 automatically): npmjs.com → the package →
  Settings → *Trusted Publisher* → GitHub Actions, repo `OSS-Ncode/powder-orm`,
  workflow `release.yml`. Do it for `@powder/node` **and each platform package**
  (`@powder/node-*`). Brand-new packages may need one initial token publish before
  a publisher can be attached, then OIDC thereafter. Provenance is automatic.
- **PyPI**: pypi.org → account → Publishing → *Add a pending publisher* → project
  `powder-orm`, owner `OSS-Ncode`, repo `powder-orm`, workflow `release.yml`,
  environment `pypi`. Also create a GitHub Environment named `pypi`. **Repo name
  is case-/spelling-sensitive** — it must match the current GitHub repo name
  exactly, or PyPI rejects the OIDC claim with "Non-user identities cannot
  create new projects."
- **NuGet**: nuget.org → account → *Trusted Publishing* → policy for this repo +
  `release.yml`, then set repo **variable** `NUGET_USER` to your nuget.org
  **login username** (not display name or org name — `NuGet/login` errors with
  "No matching trust policy owned by user '...'" if these two don't match
  exactly the account that created the policy).

Only Maven Central still uses stored secrets (no GitHub-OIDC trusted publishing
flow yet) — see the Maven section below.

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

- Uses the **`com.vanniktech.maven.publish`** plugin targeting the **Central
  Portal** (central.sonatype.com) — the current onboarding path. The plugin adds
  the sources/javadoc jars, signs, uploads, and releases in one task
  (`publishAndReleaseToMavenCentral`).
- `group` is **`io.github.oss-ncode`** ([jvm/gradle.properties](jvm/gradle.properties)),
  auto-verified as a namespace from the GitHub repo — no domain needed.

**Onboarding (do once):**
1. **central.sonatype.com** → Sign in with GitHub → *Namespaces* → add
   `io.github.oss-ncode` → verify (create the public repo it names).
2. *View Account* → **Generate User Token** → the **Username** and **Password**
   become secrets `MAVEN_CENTRAL_USERNAME` / `MAVEN_CENTRAL_PASSWORD`. (Ignore the
   base64 `user:pass` blob — that's only for manual HTTP / settings.xml.)
3. GPG key: `gpg --gen-key`, publish it
   (`gpg --keyserver keys.openpgp.org --send-keys <KEYID>`), then set
   `MAVEN_SIGNING_KEY` to the armored private key
   (`gpg --armor --export-secret-keys <KEYID>`) and `MAVEN_SIGNING_PASSWORD` to its
   passphrase.

### Go modules
Nothing to publish — the module proxy indexes a pushed tag. Because the module
lives in a subdirectory, tag it with the subdir prefix:

```bash
git tag bindings/go/v0.1.0 && git push origin bindings/go/v0.1.0
# consumers: go get github.com/OSS-Ncode/powder-orm/bindings/go@v0.1.0
```

The native lib is loaded at runtime via `powder.Load(path)` / `POWDER_LIB`.

### C / C++
No central registry. Distributed as headers + the `powder-ffi` source; consumers
build `powder-ffi` and link. (vcpkg/Conan ports are a possible future addition.)

## Deferred / follow-ups

- **Internal Java package rename `com.powder → dev.powder`.** Cosmetic only —
  the Maven groupId (`io.github.oss-ncode`) is independent of the Java package, so
  `com.powder` publishes fine as-is. Doing the rename means updating the JNI symbol
  names in
  [crates/powder-java/src/lib.rs](crates/powder-java/src/lib.rs)
  (`Java_com_powder_PowderNative_*` → `Java_dev_powder_*`), moving the `java/com/powder`
  files, and updating the Kotlin `import com.powder.*`. Do it with a JVM + cargo
  build in the loop to verify — it is not safe to do blind.
- **Self-contained JVM / native auto-extraction.** Bundle `powder_java` in jar
  resources and extract+load at runtime so consumers don't need `POWDER_LIB`.
- **linux-arm64 / musl** prebuilds for npm and NuGet.
