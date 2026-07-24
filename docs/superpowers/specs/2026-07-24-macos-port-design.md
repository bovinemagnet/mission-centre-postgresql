# Mission Centre PostgreSQL — macOS Port Design

**Author:** Paul Snow
**Date:** 2026-07-24
**Version:** 0.0.0
**Status:** Approved — ready for implementation planning
**Licence:** GPL-3.0-or-later
**Parent spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## 1. Summary

This spec covers a native macOS build of Mission Centre PostgreSQL, delivered as a relocatable,
ad-hoc-signed `MissionCentrePg.app` for Apple Silicon.

The port is far smaller than the author's earlier macOS port of Mission Center itself. That one
needed `platform-macos` — an entire native data-gathering backend (IOKit, SMC, launchd) replacing
`/proc`. This application has **no system-introspection layer at all**. Its only data source is a
PostgreSQL server reached over TCP via `tokio-postgres` and rustls, which behaves identically on
every platform.

A sweep of `src/` for `cfg(target_os)`, `cfg(unix)`, `/proc`, `/etc`, `std::os::unix` and XDG paths
returns no hits. The application logic is already portable. Every problem this spec solves lives in
the dependency graph, the build system, or the bundle.

### 1.1 Prior art

`/home/paul/gitlab/mission-center` is the author's macOS fork of Mission Center, and
`/home/paul/gitlab/gng` is its magpie backend. Decisions carried across:

- **Adopted:** the Homebrew dependency set and the `gettext`-on-the-link-path fix.
- **Adopted:** the documentation shape of `README-MAC.md` — prerequisites, build, run-from-build-dir,
  a platform-support table, and an explicit known-quirks section for macOS behaviour that looks like
  a bug but is not.
- **Adopted:** the precedent of `support/create-appimage.sh` — packaging lives in a standalone
  script under `support/`, not inside the meson graph.
- **Not adopted:** that fork stops at `ninja install` and running from the terminal. This port goes
  further, to a double-clickable bundle.
- **Not needed:** cmake and protobuf, which that fork required for `nng-c-sys` and the `types`
  crate. Neither appears in this dependency tree.

---

## 2. Scope

### 2.1 In scope

| # | Item |
|---|------|
| 1 | Removing the unused libdbus dependency so the tree builds on macOS |
| 2 | Bundle-relative resolution of the gresource, GSettings schemas, icon theme and pixbuf loaders |
| 3 | `host_machine`-guarded meson changes, and a `bundle` target on darwin |
| 4 | `support/create-macos-bundle.sh`, producing a relocatable ad-hoc-signed `.app` |
| 5 | A placeholder application icon, rendered to `.icns` |
| 6 | `README-MAC.md`, plus macOS notes in `docs/development.md` and `README.md` |

### 2.2 Explicitly out of scope

Recorded so the decisions are not silently relitigated:

- **Developer ID signing, notarisation, stapling, DMG.** Requires a paid Apple Developer account.
  The consequence is accepted and documented in §6.1: Gatekeeper will warn on any Mac other than the
  build machine.
- **Intel and universal2.** Homebrew ships per-architecture bottles, so a fat build means building
  the whole GTK stack twice. arm64 only.
- **Windows.** A separate exercise; see §9 for what this port leaves in a better state for it.
- **Any change to application behaviour, features, pages or SQL.**
- **Wiring up translation.** See §3.4.

### 2.3 Constraint

The Linux build must not regress. Every change is either platform-neutral or guarded by
`host_machine.system()` in meson. There are no `cfg(target_os)` guards in the Rust source: §3.2's
detection is runtime, not conditional compilation, so the same code compiles everywhere.

---

## 3. Dependencies and source changes

### 3.1 The keyring dependency is a deletion, not a target-gate

`Cargo.toml:27` currently reads:

```toml
keyring = { version = "4.1", features = ["dbus-secret-service-keyring-store", "apple-native-keyring-store"] }
```

The first feature pulls `dbus-secret-service` → `dbus` → `libdbus-sys`, a C dependency needing
pkg-config and libdbus headers. Confirmed present:

```
$ cargo tree -i libdbus-sys
libdbus-sys v0.2.7
└── dbus v0.9.12
    └── dbus-secret-service v4.1.0
        └── dbus-secret-service-keyring-store v1.0.0
            └── keyring v4.1.5
```

That would be a build failure on Windows and a pointless `brew install dbus` on macOS. But reading
keyring 4.1.5's `src/v1.rs:95-107`, the store is selected by **`target_os`, not by enabled feature**:

| Platform | Store selected by the `v1` compat layer |
|---|---|
| macOS | `apple_native_keyring_store::keychain::Store` |
| Linux and other Unix | `zbus_secret_service_keyring_store::Store` |
| Windows | `windows_native_keyring_store::Store` |

All three arrive with keyring's default `v1` feature, which is already enabled — no
`default-features = false` is present. So the explicitly-listed `dbus-secret-service-keyring-store`
is **never reached at runtime on any platform**; Linux uses the zbus store. The listed
`apple-native-keyring-store` is redundant for the same reason, and is enabled by `v1` with its
`keychain` sub-feature, which the bare listing omits.

The change is therefore:

```toml
keyring = { version = "4.1" }
```

This drops libdbus from the tree on every platform, removes a build prerequisite from the **Linux**
build as well, and changes nothing at runtime. `src/connection/credentials.rs` is untouched.

### 3.2 Bundle-relative resource resolution

`src/main.rs:30-31` currently falls back to a hard-coded Linux path:

```rust
let resource_dir = std::env::var("MCPG_RESOURCE_DIR")
    .unwrap_or_else(|_| "/usr/share/mission-centre-pg".to_string());
```

Inside a bundle four things must be found relative to the executable, and GTK reads three of them
from the environment:

| What | Location under `Contents/` | Mechanism |
|---|---|---|
| `mission-centre-pg.gresource` | `Resources/` | read directly |
| GSettings schemas | `Resources/glib-2.0/schemas` | `GSETTINGS_SCHEMA_DIR` |
| Icon theme | `Resources/share` | `XDG_DATA_DIRS` |
| Pixbuf loader cache | `Resources/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache` | `GDK_PIXBUF_MODULE_FILE` |

**Decision: no launcher shell script.** The conventional approach makes `CFBundleExecutable` a shell
script that exports these and re-execs the real binary. This port instead sets them in-process, at
the top of `main()`. A launcher script complicates code signing and breaks `current_exe()`, which
§3.3's detection depends on. GLib reads all three variables lazily on first use, so setting them
before any glib call is early enough.

Two constraints on the implementation:

- An existing `MCPG_RESOURCE_DIR` must still win. Development builds run from the build directory
  and depend on it.
- `std::env::set_var` must run before any thread is spawned. The crate is edition 2021, where this
  is safe; it becomes `unsafe` under edition 2024, so the call sits at the very top of `main` with a
  comment recording why.

`GDK_PIXBUF_MODULE_FILE` is the one that matters most in practice. adwaita-icon-theme ships symbolic
icons as SVG, which GTK loads through librsvg's pixbuf loader. If the cache is missing or wrong, the
application launches successfully with every icon blank and no diagnostic — the classic
GTK-on-macOS bundling failure.

### 3.3 `bundle_root` is a pure function

Detection is: take `std::env::current_exe()`, and if its parent directory is named `MacOS` and its
grandparent is named `Contents`, the grandparent's parent is the bundle root.

This is extracted into the library as a pure function rather than written inline in `main`:

```rust
pub fn bundle_root(exe_path: &Path) -> Option<PathBuf>
```

It is the only piece of genuinely new logic in this port, and as a pure function it is unit-testable
on Linux. Everything else here is build scripting, provable only on the Mac. See §7.1.

### 3.4 gettext

`gettext-rs` uses the `gettext-system` feature, so it links the system `libintl`. On macOS that
requires Homebrew's gettext on the link path — the same fix already made in the mission-center fork.

The scope is **only** linking. This repository has no `po/` directory, and `src/i18n.rs` calls
`gettextrs::gettext` without ever calling `bindtextdomain` or `textdomain`, so translation is
currently a passthrough that returns the msgid. No `LOCALEDIR` wiring, no locale data in the bundle,
and no change to `src/i18n.rs` is needed. Wiring translation up properly is separate work on all
platforms.

---

## 4. Build system

Three changes to meson, all guarded on `host_machine.system() == 'darwin'`:

1. `meson.build:23` — `gnome.post_install(glib_compile_schemas: true)` becomes Linux-only. It
   updates the system schema cache, which is meaningless for a bundle that compiles its own schemas
   into `Contents/Resources`.
2. A `bundle` run target on darwin invoking `support/create-macos-bundle.sh`, so the workflow is
   `ninja -C build bundle`.
3. `dependency('gtk4')` and `dependency('libadwaita-1')` resolve through Homebrew's pkg-config path.
   No meson change is needed, but `PKG_CONFIG_PATH` must include `$(brew --prefix)/lib/pkgconfig`,
   which is a documented prerequisite rather than a build-file change.

`build-aux/cargo-build.sh` needs **no** change. Its `/bin/sh` shebang and its `cp` of an
extension-less binary are both correct on macOS. Both only break on Windows, which is out of scope.

### 4.1 Prerequisites

| Homebrew formula | Needed for |
|---|---|
| `meson`, `ninja`, `pkg-config` | Build system |
| `gtk4`, `libadwaita`, `adwaita-icon-theme` | The toolkit, and the icons the bundle ships |
| `blueprint-compiler` | Compiling `resources/ui/*.blp`; see the note below |
| `gettext` | `libintl`, per §3.4 |
| `librsvg` | The SVG pixbuf loader the bundle ships, and `rsvg-convert` for the icon |
| `dylibbundler` | The dylib relocation of §5.3 |

Rust comes from rustup, not Homebrew. `iconutil` and `sips` are part of macOS.
`glib-compile-schemas` and `gdk-pixbuf-query-loaders` arrive with glib and gdk-pixbuf as gtk4
dependencies. `PKG_CONFIG_PATH` must include `$(brew --prefix)/lib/pkgconfig`.

Note on **blueprint-compiler**: `meson.build:18` declares
`find_program('blueprint-compiler', required: true)` with no fallback, so an absent compiler is a
hard build failure. The mission-center fork sidesteps this by vendoring it as a meson wrap
(`subprojects/blueprint-compiler.wrap`), which is why its own prerequisite list omits it. This
repository has no `subprojects/` directory. The prerequisite above is the primary route; if the
Homebrew formula proves unusable, the documented fallback is to add the same wrap file, which also
requires `pygobject3` since blueprint-compiler is Python and imports `gi`.

---

## 5. The bundle

### 5.1 Approach

Three options were weighed:

- **`gtk-mac-bundler`** — GNOME's official tool, and it natively understands pixbuf loaders, GIO
  modules and schemas. Rejected: it is a jhbuild-era Python tool that expects a jhbuild prefix
  rather than Homebrew, is lightly maintained, and GTK4/libadwaita is not a well-trodden path
  through it.
- **`cargo-bundle` / `cargo-packager`** — clean `Info.plist` and `.icns` generation from Cargo
  metadata. Rejected: neither relocates a Homebrew GTK dylib stack, so all of the work below remains,
  plus the friction of driving a Cargo-based bundler from a meson-based build.
- **A hand-rolled script (chosen).** It matches the `support/create-appimage.sh` precedent, and this
  application's dylib surface is far smaller than Mission Center's — no nng, no protobuf, no libudev,
  no separate gatherer process. Critically, this application uses **rustls, not glib-networking**,
  which eliminates the worst GTK-bundling failure mode: GIO TLS modules failing silently inside a
  bundle and taking all HTTPS with them.

The mechanical `otool`/`install_name_tool` graph walk is delegated to Homebrew's `dylibbundler`
rather than hand-written. The hand-written parts are the GTK-specific data that no generic tool
handles.

### 5.2 Layout

```
MissionCentrePg.app/Contents/
├── Info.plist
├── MacOS/mission-centre-pg
├── Frameworks/                       relocated dylibs and pixbuf loaders
└── Resources/
    ├── mission-centre-pg.gresource
    ├── MissionCentrePg.icns
    ├── glib-2.0/schemas/gschemas.compiled
    ├── share/icons/                  adwaita-icon-theme and hicolor
    └── lib/gdk-pixbuf-2.0/2.10.0/{loaders/,loaders.cache}
```

This is precisely the tree §3.2's four paths point at.

### 5.3 `support/create-macos-bundle.sh`, stage by stage

**1. Preflight.** Assert arm64; assert `brew --prefix` resolves gtk4, libadwaita and
adwaita-icon-theme; assert the meson build has produced a binary. Failing loudly here is cheaper
than debugging a half-assembled bundle.

**2. Skeleton, `Info.plist`, icon.** `CFBundleIdentifier` is `io.github.paulsnow.MissionCentrePg` —
deliberately identical to the GSettings schema ID, so bundle identity and settings identity stay in
step. Also `CFBundleExecutable`, `CFBundleName`, `CFBundleShortVersionString` (0.0.0),
`CFBundleIconFile`, `CFBundlePackageType` (`APPL`), `NSHighResolutionCapable` (true),
`LSApplicationCategoryType` (`public.app-category.developer-tools`), and **`LSMinimumSystemVersion`
= 15.0**.

**3. Binary and gresource.** Copy both into place.

**4. Dylibs.** `dylibbundler -b -d Contents/Frameworks -p @executable_path/../Frameworks` over the
main binary.

**5. GTK runtime data.** Three pieces:

- *Icon theme.* Copy adwaita-icon-theme and hicolor into `Resources/share/icons`, preserving
  `index.theme` and the generated caches.
- *Pixbuf loaders.* Copy the loader objects, then regenerate `loaders.cache` with
  `gdk-pixbuf-query-loaders`. **Two traps, both silent:** the loaders are themselves Mach-O objects
  linking the bundled dylibs, so they must also go through `dylibbundler`; and
  `gdk-pixbuf-query-loaders` writes absolute paths into the cache, which must be rewritten relative
  to the cache file's own directory. Missing either half produces the blank-icon failure of §3.2.
- *Schemas.* Copy `io.github.paulsnow.MissionCentrePg.gschema.xml` alongside gtk4's and
  libadwaita's from the Homebrew share directory, then run `glib-compile-schemas` over all three
  into one `gschemas.compiled`. All three are required — libadwaita reads GTK's schemas at startup
  and aborts if they are absent.

**6. Sign, inner-out.** Every dylib and loader first, then the bundle itself, with
`codesign --force --sign -`. Not `--deep`, which is deprecated and signs in the wrong order.

### 5.4 The placeholder icon

A simple SVG committed at `resources/icons/`, rendered by the script to the full `.iconset` size
ladder (16, 32, 64, 128, 256, 512, each at 1x and 2x) with `rsvg-convert`, then assembled with
`iconutil -c icns`. Replacing it with a real icon later is a single-file change. The Linux build has
no application icon either; installing one is left as separate work.

---

## 6. Known macOS behaviour

### 6.1 Keychain access under ad-hoc signing

`src/connection/credentials.rs` needs no change — `Entry::new` reaches the login keychain through
`apple-native-keyring-store` (§3.1). Two consequences of ad-hoc signing must be documented, because
both look like application bugs:

- An **unsigned** binary run directly from `build/src/` fails keychain access with
  `errSecMissingEntitlement` (`-34018`). The development workflow therefore needs `codesign -s -`
  applied to the raw binary, not only to the bundle.
- An **ad-hoc** signature carries no stable identity, so its designated requirement changes on every
  rebuild. macOS re-prompts for keychain access after each rebuild and "Always Allow" does not
  persist. This is only fixable with a Developer ID, which §2.2 puts out of scope.

Gatekeeper will also warn when the bundle is opened on any Mac other than the one that built it, for
the same reason.

### 6.2 Relocatability is easy to get wrong invisibly

A bundle that still references `/opt/homebrew` runs perfectly on the build machine and fails on
every other Mac. Two checks, per §7.2.

---

## 7. Testing

### 7.1 Automated, on Linux

- **`bundle_root` unit tests** (§3.3), written first: a well-formed bundle path yields a root; a
  plain `/usr/local/bin/mission-centre-pg` yields `None`; a path containing `Contents` but not
  `MacOS` yields `None`; a path too shallow to have a grandparent yields `None`.
- **Regression gate:** `cargo build`, `cargo test` and the meson/ninja build all pass with libdbus
  removed from the tree.

Credential storage itself has no automated test — it needs a live secret service — so the keyring
change of §3.1 also gets a manual smoke test on Linux: add a server with a password, restart, confirm
it reconnects.

### 7.2 Manual, on the Mac

Executed in a later session, on hardware:

1. `meson setup` and `ninja` complete.
2. The binary runs from the build directory with `MCPG_RESOURCE_DIR` and `GSETTINGS_SCHEMA_DIR` set.
3. `ninja -C build bundle` produces `MissionCentrePg.app`.
4. The bundle launches by double-click.
5. Icons render — this is the §3.2 loader-cache check.
6. Adding a server persists across a restart, proving both the bundled schemas and the keychain.
7. **`otool` sweep:** every Mach-O object in the bundle is walked and none references
   `/opt/homebrew`. Automated inside the script so it fails the build, not the user.
8. **`brew unlink` test:** `brew unlink gtk4 libadwaita adwaita-icon-theme`, launch, relink.

Step 8 is the one that actually proves self-containment. The `otool` sweep of step 7 cannot see a
library that is only `dlopen`ed at runtime — which is exactly how the pixbuf loaders are loaded.

### 7.3 The container suite on macOS

`docs/development.md` documents podman-on-Linux for `cargo test --test portability`. A Mac needs
Colima, `podman machine`, or Docker Desktop, with `DOCKER_HOST` pointed at the corresponding socket.
This is a documentation addition; the tests themselves need no change.

Note that `tests/portability.rs` tests portability across **PostgreSQL versions**, not across
operating systems. Nothing in this spec changes it.

---

## 8. Documentation

| File | Change |
|---|---|
| `README-MAC.md` | New. Prerequisites, build, run-from-build-dir, bundling, platform-support table, known quirks from §6. Mirrors the mission-center fork's structure. |
| `docs/development.md` | Add the macOS container-runtime section from §7.3. |
| `README.md` | One line noting macOS support and pointing at `README-MAC.md`. |

Markdown throughout, matching this repository's existing `docs/` convention.

---

## 9. What this leaves for Windows

Not in scope, but recorded because §3.1 resolves the hardest blocker for free. After this port,
Windows needs: an MSYS2/UCRT64 toolchain targeting `x86_64-pc-windows-gnu`; `.exe` handling in
`build-aux/cargo-build.sh`; an exe-relative resource path, for which §3.3's `bundle_root` is the
wrong shape but the right idea; and GTK DLL bundling. `windows-native-keyring-store` already works
untouched, per the table in §3.1.

---

## 10. Success criteria

1. `MissionCentrePg.app` launches by double-click on an Apple Silicon Mac running macOS 15 and
   connects to a PostgreSQL server.
2. It runs with Homebrew's gtk4, libadwaita and adwaita-icon-theme unlinked.
3. Icons render, and configured servers persist across a restart.
4. The Linux build, tests and packaging are unchanged in behaviour, and no longer require libdbus.
5. No `cfg(target_os)` guard appears in `src/`.
