# Mission Centre PostgreSQL — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a GTK4/libadwaita desktop application that connects to an arbitrary PostgreSQL 14–18 server and renders live Overview graphs and a Sessions table, read-only and in-memory.

**Architecture:** Single process, two threads. A collector thread owns a tokio runtime and a `tokio-postgres` client, samples `pg_stat_database` and `pg_stat_activity` on a serial loop, and sends immutable `CollectorEvent` values down an `async_channel`. The GTK main thread consumes them via `glib::spawn_future_local` and updates widgets. No IPC, no gatherer process.

**Tech Stack:** Rust 1.97, gtk4-rs 0.11, libadwaita 0.9, Blueprint, Meson + Cargo, tokio-postgres 0.7, rustls 0.23, keyring 4.1, testcontainers 0.27.

**Spec:** `docs/superpowers/specs/2026-07-22-mission-centre-postgresql-design.md`

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Repository:** `/home/paul/gitHUB/mission-centre-postgresql`. All code and docs live here.
- **Licence:** GPL-3.0-or-later. Every new source file carries a GPL header naming **Paul Snow** as author.
- **Version:** `0.0.0` in `Cargo.toml` and `meson.build`.
- **PostgreSQL floor:** 14 (`server_version_num >= 140000`). Never refuse a connection on version; gate at the page level.
- **Spelling:** British English in all user-facing strings, comments and documentation (`behaviour`, `initialise`, `colour`).
- **Never log or display a password**, nor a full connection string containing one.
- **Never touch GTK widgets off the main thread.** Collector output reaches the UI only through the `async_channel`.
- **`cargo fmt` must produce no diff** before any commit. There is no custom `rustfmt.toml`.
- **File size:** no source file over ~800 lines. Split by responsibility if one approaches it.
- **Rates are per-interval deltas**, never cumulative-since-reset.
- **App ID:** `io.github.paulsnow.MissionCentrePg`. **Binary:** `mission-centre-pg`.

### Conventions established by Task 1 (later tasks must follow)

- **Cargo renames the GTK crates.** `Cargo.toml` uses `[dependencies.gtk] package = "gtk4"`
  and `[dependencies.adw] package = "libadwaita"`, so all Rust code says `gtk::` and `adw::`.
- **`keyring` features are `dbus-secret-service-keyring-store` and
  `apple-native-keyring-store`.** The `-native` names in earlier drafts do not exist in keyring
  4.1. The dbus/Secret Service backend is the correct choice: credentials must survive a reboot,
  and `linux-keyutils-keyring-store` is an ephemeral kernel keyring that does not. Task 13
  Step 5 verifies with `secret-tool`, which queries Secret Service.
- **`glib::wrapper!` blocks for `CompositeTemplate` widgets must list the widget interfaces** —
  at minimum `gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget`, plus `gtk::Native,
  gtk::Root, gtk::ShortcutManager` for windows. The derive macro requires them.
- **`gnome.compile_resources()` needs `gresource_bundle: true`**, and blueprint-compiler's
  `batch-compile` takes `@CURRENT_SOURCE_DIR@` (not `@CURRENT_SOURCE_DIR@/ui`) as its source
  directory, so output lands at `ui/<name>.ui` where the gresource manifest expects it.
- **`.gitignore` carries `!/build-aux/`** immediately after `/build-*/`, which would otherwise
  exclude the cargo shim.

### Verified environment (this machine, Garuda/Arch)

| Tool | State |
|------|-------|
| rustc / cargo | 1.97.0 — present |
| ninja | 1.13.2 — present |
| gtk4 | 4.22.4 — present |
| libadwaita-1 | 1.9.2 — present |
| meson | **missing** — `sudo pacman -S --needed meson` (1.11.2) |
| blueprint-compiler | **missing** — `sudo pacman -S --needed blueprint-compiler` (0.22.2) |
| docker | **missing**; `podman` is present at `/usr/bin/podman` — see Task 6 |
| PostgreSQL | 18.4 running locally on `/run/postgresql:5432` — useful as a manual test target |

`index.crates.io` is reachable, so `cargo` can fetch dependencies. Note that SSH to `gitlab.com:22` is blocked on this machine; if you need to push to a remote, use HTTPS.

---

## File Structure

| Path | Responsibility |
|------|----------------|
| `Cargo.toml` | Crate manifest, pinned dependencies |
| `meson.build`, `src/meson.build`, `data/meson.build`, `resources/meson.build` | Build wiring |
| `build-aux/cargo-build.sh` | Meson → Cargo shim (custom_target cannot chain commands) |
| `src/main.rs` | Entry point: gettext, GResource registration, application run |
| `src/application.rs` | `adw::Application` subclass; owns `gio::Settings` |
| `src/window.rs` | Main window: split view, sidebar, stack, privilege banner, event routing |
| `src/i18n.rs` | gettext wrappers |
| `src/connection/params.rs` | `ConnectionParams`, `SslMode`, redacted `Debug` |
| `src/connection/credentials.rs` | keyring store/fetch/delete by server UUID |
| `src/connection/probe.rs` | `ServerInfo`, `PrivilegeLevel`, probe SQL and row mapping |
| `src/connection/registry.rs` | Server list persistence to GSettings as JSON |
| `src/collector/snapshot.rs` | `CollectorEvent`, `Snapshot`, `DatabaseCounters`, `DatabaseRates`, `Session` |
| `src/collector/rates.rs` | Pure rate derivation from counter pairs |
| `src/collector/queries.rs` | The three SQL statements and their row-mapping functions |
| `src/collector/mod.rs` | Collector thread, tokio runtime, serial sample loop, backoff |
| `src/widgets/graph_widget.rs` | Vendored from Mission Center, verbatim |
| `src/widgets/graph_widget_utils.rs` | Vendored from Mission Center, two edits |
| `src/widgets/sidebar_row.rs` | Ours: sparkline row composing `GraphWidget`, which owns its own buffer |
| `src/pages/overview.rs` | Overview page |
| `src/pages/sessions.rs` | Sessions `ColumnView` table |
| `src/dialogs/add_server.rs` | Add Server dialog |
| `resources/ui/*.blp` | Blueprint UI definitions |
| `data/io.github.paulsnow.MissionCentrePg.gschema.xml` | GSettings schema |
| `tests/integration/` | testcontainers tests against PG 14 and 18 |

---

## Task 1: Project skeleton that builds and opens a window

**Files:**
- Create: `Cargo.toml`, `meson.build`, `build-aux/cargo-build.sh`, `data/meson.build`, `data/io.github.paulsnow.MissionCentrePg.gschema.xml`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`, `resources/ui/window.blp`, `src/meson.build`, `src/main.rs`, `src/application.rs`, `src/window.rs`, `src/i18n.rs`, `.gitignore`, `README.md`, `COPYING`

**Interfaces:**
- Consumes: nothing
- Produces: `MissionCentrePgApplication` (in `application.rs`), `MissionCentrePgWindow` (in `window.rs`), the `mcpg_resources()` registration in `main.rs`, and a working `ninja -C build` producing `build/src/mission-centre-pg`

No TDD here: there is nothing to assert until a window exists. Verification is the build succeeding and the window appearing.

- [ ] **Step 1: Install the two missing build tools**

```bash
sudo pacman -S --needed meson blueprint-compiler
meson --version && blueprint-compiler --version
```

Expected: `1.11.2` and `0.22.2` (or newer).

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "mission-centre-pg"
version = "0.0.0"
edition = "2021"
rust-version = "1.90"
authors = ["Paul Snow"]
license = "GPL-3.0-or-later"
description = "A GTK4 desktop monitor for PostgreSQL servers"

# lib + bin from the start: the integration tests in Task 6 can only reach
# library code, and the pages/widgets/dialogs modules need the i18n helpers.
[lib]
name = "mission_centre_pg"
path = "src/lib.rs"

[[bin]]
name = "mission-centre-pg"
path = "src/main.rs"

[dependencies]
gtk4 = { version = "0.11", features = ["v4_14"] }
libadwaita = { version = "0.9", features = ["v1_5"] }
tokio = { version = "1.53", features = ["rt", "time", "macros", "sync"] }
tokio-postgres = { version = "0.7", features = ["with-chrono-0_4"] }
tokio-postgres-rustls = "0.14"
rustls = "0.23"
rustls-native-certs = "0.8"
chrono = "0.4"
keyring = { version = "4.1", features = ["linux-native", "apple-native"] }
async-channel = "2.5"
uuid = { version = "1.24", features = ["v4"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
gettext-rs = { version = "0.7", features = ["gettext-system"] }
thiserror = "2.0"
futures-util = "0.3"

[dev-dependencies]
testcontainers = "0.27"
testcontainers-modules = { version = "0.15", features = ["postgres"] }
tokio = { version = "1.53", features = ["rt-multi-thread", "macros"] }
```

- [ ] **Step 3: Write the meson files**

`meson.build`:

```meson
project(
  'mission-centre-pg',
  version: '0.0.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0.0',
)

gnome = import('gnome')

app_id = 'io.github.paulsnow.MissionCentrePg'

dependency('gtk4', version: '>= 4.14')
dependency('libadwaita-1', version: '>= 1.5')

cargo = find_program('cargo', required: true)
blueprint_compiler = find_program('blueprint-compiler', required: true)

subdir('data')
subdir('resources')
subdir('src')

gnome.post_install(glib_compile_schemas: true)
```

`data/meson.build`:

```meson
install_data(
  '@0@.gschema.xml'.format(app_id),
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

gnome.compile_schemas(build_by_default: true, depend_files: files('@0@.gschema.xml'.format(app_id)))
```

`resources/meson.build`:

```meson
blueprints = custom_target(
  'blueprints',
  input: files('ui/window.blp'),
  output: '.',
  command: [blueprint_compiler, 'batch-compile', '@OUTPUT@', '@CURRENT_SOURCE_DIR@/ui', '@INPUT@'],
)

resources_gresource = gnome.compile_resources(
  'mission-centre-pg',
  'mission-centre-pg.gresource.xml',
  dependencies: blueprints,
  source_dir: meson.current_build_dir(),
  build_by_default: true,
)
```

`src/meson.build`:

```meson
cargo_profile = get_option('buildtype') == 'release' ? 'release' : 'debug'

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'mission-centre-pg',
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  depends: resources_gresource,
  command: [
    meson.project_source_root() / 'build-aux' / 'cargo-build.sh',
    meson.project_source_root(),
    meson.project_build_root(),
    '@OUTPUT@',
    cargo_profile,
  ],
)
```

- [ ] **Step 4: Write `build-aux/cargo-build.sh` and make it executable**

```sh
#!/bin/sh
# Meson -> Cargo shim. custom_target() cannot chain commands, so the build
# and the copy of the resulting binary happen here.
set -eu

SOURCE_ROOT="$1"
BUILD_ROOT="$2"
OUTPUT="$3"
PROFILE="$4"

CARGO_TARGET_DIR="$BUILD_ROOT/cargo"
export CARGO_TARGET_DIR

if [ "$PROFILE" = "release" ]; then
    cargo build --manifest-path "$SOURCE_ROOT/Cargo.toml" --release
    cp "$CARGO_TARGET_DIR/release/mission-centre-pg" "$OUTPUT"
else
    cargo build --manifest-path "$SOURCE_ROOT/Cargo.toml"
    cp "$CARGO_TARGET_DIR/debug/mission-centre-pg" "$OUTPUT"
fi
```

```bash
chmod +x build-aux/cargo-build.sh
```

- [ ] **Step 5: Write the GSettings schema**

`data/io.github.paulsnow.MissionCentrePg.gschema.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<schemalist>
  <schema id="io.github.paulsnow.MissionCentrePg" path="/io/github/paulsnow/MissionCentrePg/">
    <key name="window-width" type="i">
      <default>1100</default>
      <summary>Main window width</summary>
    </key>
    <key name="window-height" type="i">
      <default>700</default>
      <summary>Main window height</summary>
    </key>
    <key name="window-maximised" type="b">
      <default>false</default>
      <summary>Whether the main window is maximised</summary>
    </key>
    <key name="servers" type="s">
      <default>'[]'</default>
      <summary>Configured servers as a JSON array. Never contains passwords.</summary>
    </key>
    <key name="sample-interval-ms" type="i">
      <range min="500" max="60000"/>
      <default>2000</default>
      <summary>Minimum gap between samples in milliseconds</summary>
    </key>
    <key name="graph-points" type="i">
      <range min="10" max="600"/>
      <default>300</default>
      <summary>Number of points retained per graph series</summary>
    </key>
    <key name="hide-idle-sessions" type="b">
      <default>true</default>
      <summary>Hide idle sessions in the Sessions table</summary>
    </key>
  </schema>
</schemalist>
```

- [ ] **Step 6: Write the GResource manifest and the window Blueprint**

`resources/mission-centre-pg.gresource.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/io/github/paulsnow/MissionCentrePg">
    <file preprocess="xml-stripblanks">ui/window.ui</file>
  </gresource>
</gresources>
```

`resources/ui/window.blp`:

```blueprint
using Gtk 4.0;
using Adw 1;

template $MissionCentrePgWindow: Adw.ApplicationWindow {
  title: _("Mission Centre PostgreSQL");
  default-width: 1100;
  default-height: 700;

  content: Adw.ToolbarView {
    [top]
    Adw.HeaderBar {}

    content: Adw.StatusPage {
      icon-name: "network-server-symbolic";
      title: _("No server selected");
      description: _("Add a server to begin monitoring.");
    };
  };
}
```

- [ ] **Step 7: Write `src/i18n.rs`**

```rust
/* i18n.rs
 *
 * Copyright 2026 Paul Snow
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

pub fn i18n(format: &str) -> String {
    gettextrs::gettext(format)
}

pub fn i18n_f(format: &str, args: &[&str]) -> String {
    let mut output = gettextrs::gettext(format);
    for arg in args {
        output = output.replacen("{}", arg, 1);
    }
    output
}
```

Also create `src/lib.rs` with the single module it holds so far. Later tasks add
to it; `i18n` lives here rather than in the binary because the pages, dialogs
and widgets modules all use it.

```rust
pub mod i18n;
```

- [ ] **Step 8: Write `src/application.rs`**

Use the same GPL header as Step 7 on this and every subsequent new file.

```rust
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::window::MissionCentrePgWindow;

pub const APP_ID: &str = "io.github.paulsnow.MissionCentrePg";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MissionCentrePgApplication;

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgApplication {
        const NAME: &'static str = "MissionCentrePgApplication";
        type Type = super::MissionCentrePgApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MissionCentrePgApplication {}

    impl ApplicationImpl for MissionCentrePgApplication {
        fn activate(&self) {
            let app = self.obj();
            let window = app
                .active_window()
                .unwrap_or_else(|| MissionCentrePgWindow::new(&*app).upcast());
            window.present();
        }
    }

    impl GtkApplicationImpl for MissionCentrePgApplication {}
    impl AdwApplicationImpl for MissionCentrePgApplication {}
}

glib::wrapper! {
    pub struct MissionCentrePgApplication(ObjectSubclass<imp::MissionCentrePgApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MissionCentrePgApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::empty())
            .build()
    }

    pub fn settings(&self) -> gio::Settings {
        gio::Settings::new(APP_ID)
    }
}

impl Default for MissionCentrePgApplication {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 9: Write `src/window.rs`**

```rust
use adw::subclass::prelude::*;
use gtk::prelude::IsA;
use gtk::{gio, glib};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/window.ui")]
    pub struct MissionCentrePgWindow;

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgWindow {
        const NAME: &'static str = "MissionCentrePgWindow";
        type Type = super::MissionCentrePgWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MissionCentrePgWindow {}
    impl WidgetImpl for MissionCentrePgWindow {}
    impl WindowImpl for MissionCentrePgWindow {}
    impl ApplicationWindowImpl for MissionCentrePgWindow {}
    impl AdwApplicationWindowImpl for MissionCentrePgWindow {}
}

glib::wrapper! {
    pub struct MissionCentrePgWindow(ObjectSubclass<imp::MissionCentrePgWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl MissionCentrePgWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}
```

- [ ] **Step 10: Write `src/main.rs`**

```rust
mod application;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

use application::MissionCentrePgApplication;

fn main() -> glib::ExitCode {
    let resource_dir = std::env::var("MCPG_RESOURCE_DIR")
        .unwrap_or_else(|_| "/usr/share/mission-centre-pg".to_string());
    let resource_path = format!("{resource_dir}/mission-centre-pg.gresource");

    let resources = gio::Resource::load(&resource_path)
        .unwrap_or_else(|e| panic!("Failed to load resources from {resource_path}: {e}"));
    gio::resources_register(&resources);

    MissionCentrePgApplication::new().run()
}
```

Every `use` in this project goes at the top of its file. If a code block later in
this plan shows a trailing `use`, move it to the top — that is a transcription
slip in the plan, not a style to copy.

- [ ] **Step 11: Write `.gitignore`, `README.md` and `COPYING`**

`.gitignore`:

```
/target/
/build/
/build-*/
*.gresource
```

`README.md` must credit Mission Center, since Task 8 vendors code from it:

```markdown
# Mission Centre PostgreSQL

A GTK4/libadwaita desktop monitor for PostgreSQL servers, in the style of
[Mission Center](https://gitlab.com/mission-center-devs/mission-center).

Licensed GPL-3.0-or-later. Portions of `src/widgets/` are derived from Mission Center,
copyright the Mission Center Developers, used under the GPL.

## Building

    sudo pacman -S --needed meson ninja blueprint-compiler gtk4 libadwaita
    meson setup build
    ninja -C build

## Running from the build directory

    export MCPG_RESOURCE_DIR="$PWD/build/resources"
    export GSETTINGS_SCHEMA_DIR="$PWD/build/data"
    ./build/src/mission-centre-pg
```

Copy the GPL text:

```bash
cp /home/paul/gitlab/mission-center/COPYING /home/paul/gitHUB/mission-centre-postgresql/COPYING
```

- [ ] **Step 12: Build**

```bash
cd /home/paul/gitHUB/mission-centre-postgresql
meson setup build
ninja -C build
```

Expected: build succeeds, `build/src/mission-centre-pg` exists.

- [ ] **Step 13: Run and confirm a window appears**

```bash
export MCPG_RESOURCE_DIR="$PWD/build/resources"
export GSETTINGS_SCHEMA_DIR="$PWD/build/data"
./build/src/mission-centre-pg
```

Expected: a window titled "Mission Centre PostgreSQL" showing the "No server selected" status page. Close it.

- [ ] **Step 14: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: project skeleton with meson/cargo build and empty window"
```

---

## Task 2: Snapshot types and rate derivation

This is the heart of the correctness story, it is pure, and it is where TDD earns its keep. No database, no GTK.

**Files:**
- Create: `src/collector/mod.rs` (module declarations only for now), `src/collector/snapshot.rs`, `src/collector/rates.rs`
- Modify: `src/lib.rs` (add `pub mod collector;`)

**Interfaces:**
- Consumes: nothing
- Produces:
  - `DatabaseCounters { xact_commit, xact_rollback, blks_read, blks_hit, tup_returned, tup_fetched, tup_inserted, tup_updated, tup_deleted, deadlocks, temp_bytes: i64 }`
  - `DatabaseRates { transactions_per_sec, tuples_returned_per_sec, tuples_fetched_per_sec, tuples_inserted_per_sec, tuples_updated_per_sec, tuples_deleted_per_sec, deadlocks_per_sec, temp_bytes_per_sec: f64, cache_hit_ratio: Option<f64> }`
  - `pub fn derive_rates(prev: &DatabaseCounters, cur: &DatabaseCounters, elapsed: Duration) -> Option<DatabaseRates>`
  - `impl DatabaseCounters { pub fn sum(rows: &[DatabaseCounters]) -> DatabaseCounters }`

- [ ] **Step 1: Write the failing tests**

Create `src/collector/rates.rs` containing only the test module and the type imports:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn counters(commit: i64, rollback: i64, hit: i64, read: i64) -> DatabaseCounters {
        DatabaseCounters {
            xact_commit: commit,
            xact_rollback: rollback,
            blks_hit: hit,
            blks_read: read,
            ..DatabaseCounters::default()
        }
    }

    #[test]
    fn derives_transactions_per_second_from_the_delta() {
        let prev = counters(1_000, 100, 0, 0);
        let cur = counters(3_000, 200, 0, 0);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(2)).unwrap();
        // (3000-1000) + (200-100) = 2100 over 2 seconds
        assert_eq!(rates.transactions_per_sec, 1_050.0);
    }

    #[test]
    fn cache_hit_ratio_uses_the_interval_not_the_cumulative_totals() {
        // A long-running server with a 99.99% lifetime ratio that is currently
        // missing cache on every single read. The naive cumulative calculation
        // would report ~0.9999; the correct interval calculation reports 0.0.
        let prev = counters(0, 0, 999_900, 100);
        let cur = counters(0, 0, 999_900, 1_100);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(1)).unwrap();
        assert_eq!(rates.cache_hit_ratio, Some(0.0));
    }

    #[test]
    fn cache_hit_ratio_is_none_when_no_blocks_were_accessed() {
        let prev = counters(10, 0, 500, 20);
        let cur = counters(20, 0, 500, 20);
        let rates = derive_rates(&prev, &cur, Duration::from_secs(1)).unwrap();
        assert_eq!(rates.cache_hit_ratio, None);
    }

    #[test]
    fn returns_none_when_a_counter_goes_backwards() {
        // pg_stat_reset() or a server restart.
        let prev = counters(5_000, 100, 900, 100);
        let cur = counters(12, 0, 4, 1);
        assert_eq!(derive_rates(&prev, &cur, Duration::from_secs(2)), None);
    }

    #[test]
    fn returns_none_when_no_time_has_elapsed() {
        let prev = counters(1_000, 0, 0, 0);
        let cur = counters(2_000, 0, 0, 0);
        assert_eq!(derive_rates(&prev, &cur, Duration::ZERO), None);
    }

    #[test]
    fn sums_counters_across_databases() {
        let a = counters(10, 1, 100, 5);
        let b = counters(20, 2, 200, 10);
        let total = DatabaseCounters::sum(&[a, b]);
        assert_eq!(total.xact_commit, 30);
        assert_eq!(total.xact_rollback, 3);
        assert_eq!(total.blks_hit, 300);
        assert_eq!(total.blks_read, 15);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib rates 2>&1 | head -20
```

Expected: FAIL — `cannot find type DatabaseCounters in this scope`, `cannot find function derive_rates`.

- [ ] **Step 3: Write `src/collector/snapshot.rs`**

```rust
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatabaseCounters {
    pub xact_commit: i64,
    pub xact_rollback: i64,
    pub blks_read: i64,
    pub blks_hit: i64,
    pub tup_returned: i64,
    pub tup_fetched: i64,
    pub tup_inserted: i64,
    pub tup_updated: i64,
    pub tup_deleted: i64,
    pub deadlocks: i64,
    pub temp_bytes: i64,
}

impl DatabaseCounters {
    /// Server-wide totals. `pg_stat_database` returns one row per database;
    /// the Overview page shows the sum, because "how loaded is this server"
    /// is the question it answers.
    pub fn sum(rows: &[DatabaseCounters]) -> DatabaseCounters {
        rows.iter().fold(DatabaseCounters::default(), |mut acc, r| {
            acc.xact_commit += r.xact_commit;
            acc.xact_rollback += r.xact_rollback;
            acc.blks_read += r.blks_read;
            acc.blks_hit += r.blks_hit;
            acc.tup_returned += r.tup_returned;
            acc.tup_fetched += r.tup_fetched;
            acc.tup_inserted += r.tup_inserted;
            acc.tup_updated += r.tup_updated;
            acc.tup_deleted += r.tup_deleted;
            acc.deadlocks += r.deadlocks;
            acc.temp_bytes += r.temp_bytes;
            acc
        })
    }

    /// True if any counter is lower than in `previous`, which means the
    /// statistics were reset or the server restarted.
    pub fn went_backwards_from(&self, previous: &DatabaseCounters) -> bool {
        self.xact_commit < previous.xact_commit
            || self.xact_rollback < previous.xact_rollback
            || self.blks_read < previous.blks_read
            || self.blks_hit < previous.blks_hit
            || self.tup_returned < previous.tup_returned
            || self.tup_fetched < previous.tup_fetched
            || self.tup_inserted < previous.tup_inserted
            || self.tup_updated < previous.tup_updated
            || self.tup_deleted < previous.tup_deleted
            || self.deadlocks < previous.deadlocks
            || self.temp_bytes < previous.temp_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DatabaseRates {
    pub transactions_per_sec: f64,
    /// `None` when no blocks were accessed in the interval — there is no
    /// ratio to report, and reporting zero would be a lie.
    pub cache_hit_ratio: Option<f64>,
    pub tuples_returned_per_sec: f64,
    pub tuples_fetched_per_sec: f64,
    pub tuples_inserted_per_sec: f64,
    pub tuples_updated_per_sec: f64,
    pub tuples_deleted_per_sec: f64,
    pub deadlocks_per_sec: f64,
    pub temp_bytes_per_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCounts {
    pub active: usize,
    pub idle: usize,
    pub idle_in_transaction: usize,
    pub other: usize,
}

impl SessionCounts {
    pub fn total(&self) -> usize {
        self.active + self.idle + self.idle_in_transaction + self.other
    }
}

// No `Eq`: `query_duration_secs` is an f64.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub pid: i32,
    pub user_name: Option<String>,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub backend_type: Option<String>,
    /// Seconds since `query_start`, computed server-side so the client clock
    /// is irrelevant. `None` when no query is running.
    pub query_duration_secs: Option<f64>,
    /// `None` when the connected role lacks `pg_monitor` and the backend
    /// belongs to another user.
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub max_connections: i32,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub taken_at: Instant,
    pub totals: DatabaseCounters,
    pub rates: Option<DatabaseRates>,
    pub connected_database_size_bytes: Option<i64>,
    pub session_counts: SessionCounts,
    pub sessions: Vec<Session>,
    pub settings: ServerSettings,
}
```

- [ ] **Step 4: Write the implementation in `src/collector/rates.rs`**

Put this above the existing `#[cfg(test)] mod tests`:

```rust
use std::time::Duration;

use super::snapshot::{DatabaseCounters, DatabaseRates};

/// Derive per-interval rates from two consecutive counter readings.
///
/// Returns `None` when no rate can honestly be reported: zero elapsed time,
/// or a counter that went backwards because the statistics were reset.
pub fn derive_rates(
    prev: &DatabaseCounters,
    cur: &DatabaseCounters,
    elapsed: Duration,
) -> Option<DatabaseRates> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    if cur.went_backwards_from(prev) {
        return None;
    }

    let per_sec = |cur_v: i64, prev_v: i64| (cur_v - prev_v) as f64 / secs;

    let hit_delta = cur.blks_hit - prev.blks_hit;
    let read_delta = cur.blks_read - prev.blks_read;
    let block_delta = hit_delta + read_delta;
    let cache_hit_ratio = if block_delta > 0 {
        Some(hit_delta as f64 / block_delta as f64)
    } else {
        None
    };

    Some(DatabaseRates {
        transactions_per_sec: per_sec(
            cur.xact_commit + cur.xact_rollback,
            prev.xact_commit + prev.xact_rollback,
        ),
        cache_hit_ratio,
        tuples_returned_per_sec: per_sec(cur.tup_returned, prev.tup_returned),
        tuples_fetched_per_sec: per_sec(cur.tup_fetched, prev.tup_fetched),
        tuples_inserted_per_sec: per_sec(cur.tup_inserted, prev.tup_inserted),
        tuples_updated_per_sec: per_sec(cur.tup_updated, prev.tup_updated),
        tuples_deleted_per_sec: per_sec(cur.tup_deleted, prev.tup_deleted),
        deadlocks_per_sec: per_sec(cur.deadlocks, prev.deadlocks),
        temp_bytes_per_sec: per_sec(cur.temp_bytes, prev.temp_bytes),
    })
}
```

Create `src/collector/mod.rs`:

```rust
pub mod rates;
pub mod snapshot;
```

Add `pub mod collector;` to `src/lib.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --lib 2>&1 | tail -20
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: snapshot types and interval-based rate derivation"
```

---

## Task 3: The SQL statements and row mapping

**Files:**
- Create: `src/collector/queries.rs`
- Modify: `src/collector/mod.rs`

**Interfaces:**
- Consumes: `DatabaseCounters`, `Session`, `SessionCounts`, `ServerSettings` from Task 2
- Produces:
  - `pub const DATABASE_STATS_SQL: &str`, `pub const ACTIVITY_SQL: &str`, `pub const SETTINGS_SQL: &str`, `pub const DATABASE_SIZE_SQL: &str`
  - `pub fn map_database_counters(row: &tokio_postgres::Row) -> DatabaseCounters`
  - `pub fn map_session(row: &tokio_postgres::Row) -> Session`
  - `pub fn count_sessions(sessions: &[Session]) -> SessionCounts`

Phase 1 needs no per-version SQL: every column used is unchanged across PostgreSQL 14 to 18. Task 6's integration tests are what prove that claim. Do not build a `sql_for(version)` selector.

- [ ] **Step 1: Write the failing test**

Only `count_sessions` is testable without a database; the SQL itself is proven in Task 6. Add to `src/collector/queries.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn session(state: Option<&str>) -> Session {
        Session {
            pid: 1,
            user_name: None,
            application_name: None,
            client_addr: None,
            database: None,
            state: state.map(str::to_string),
            wait_event_type: None,
            wait_event: None,
            backend_type: None,
            query_duration_secs: None,
            query: None,
        }
    }

    #[test]
    fn counts_sessions_by_state() {
        let sessions = vec![
            session(Some("active")),
            session(Some("active")),
            session(Some("idle")),
            session(Some("idle in transaction")),
            session(Some("fastpath function call")),
            session(None),
        ];
        let counts = count_sessions(&sessions);
        assert_eq!(counts.active, 2);
        assert_eq!(counts.idle, 1);
        assert_eq!(counts.idle_in_transaction, 1);
        assert_eq!(counts.other, 2);
        assert_eq!(counts.total(), 6);
    }

    #[test]
    fn idle_in_transaction_aborted_counts_as_idle_in_transaction() {
        let counts = count_sessions(&[session(Some("idle in transaction (aborted)"))]);
        assert_eq!(counts.idle_in_transaction, 1);
        assert_eq!(counts.idle, 0);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib queries 2>&1 | head -20
```

Expected: FAIL — `cannot find function count_sessions`.

- [ ] **Step 3: Write the implementation**

Above the test module in `src/collector/queries.rs`:

```rust
use tokio_postgres::Row;

use super::snapshot::{DatabaseCounters, ServerSettings, Session, SessionCounts};

/// Server-wide cumulative counters, one row per database.
/// `datname IS NOT NULL` excludes the shared-object pseudo-row.
pub const DATABASE_STATS_SQL: &str = "\
SELECT xact_commit, xact_rollback, blks_read, blks_hit,
       tup_returned, tup_fetched, tup_inserted, tup_updated, tup_deleted,
       deadlocks, temp_bytes
  FROM pg_stat_database
 WHERE datname IS NOT NULL";

/// Current sessions. `query` is NULL for other users' backends when the
/// connected role lacks pg_monitor; that is expected, not an error.
/// The duration is computed server-side so the client clock is irrelevant.
pub const ACTIVITY_SQL: &str = "\
SELECT pid,
       usename::text            AS user_name,
       application_name,
       client_addr::text        AS client_addr,
       datname                  AS database,
       state,
       wait_event_type,
       wait_event,
       backend_type,
       EXTRACT(EPOCH FROM (now() - query_start))::float8 AS query_duration_secs,
       query
  FROM pg_stat_activity
 WHERE pid <> pg_backend_pid()";

pub const SETTINGS_SQL: &str = "SELECT current_setting('max_connections')::int AS max_connections";

pub const DATABASE_SIZE_SQL: &str = "SELECT pg_database_size(current_database())::bigint AS size";

pub fn map_database_counters(row: &Row) -> DatabaseCounters {
    DatabaseCounters {
        xact_commit: row.get("xact_commit"),
        xact_rollback: row.get("xact_rollback"),
        blks_read: row.get("blks_read"),
        blks_hit: row.get("blks_hit"),
        tup_returned: row.get("tup_returned"),
        tup_fetched: row.get("tup_fetched"),
        tup_inserted: row.get("tup_inserted"),
        tup_updated: row.get("tup_updated"),
        tup_deleted: row.get("tup_deleted"),
        deadlocks: row.get("deadlocks"),
        temp_bytes: row.get("temp_bytes"),
    }
}

pub fn map_session(row: &Row) -> Session {
    Session {
        pid: row.get("pid"),
        user_name: row.get("user_name"),
        application_name: row.get("application_name"),
        client_addr: row.get("client_addr"),
        database: row.get("database"),
        state: row.get("state"),
        wait_event_type: row.get("wait_event_type"),
        wait_event: row.get("wait_event"),
        backend_type: row.get("backend_type"),
        query_duration_secs: row.get("query_duration_secs"),
        query: row.get("query"),
    }
}

pub fn count_sessions(sessions: &[Session]) -> SessionCounts {
    let mut counts = SessionCounts {
        active: 0,
        idle: 0,
        idle_in_transaction: 0,
        other: 0,
    };
    for session in sessions {
        match session.state.as_deref() {
            Some("active") => counts.active += 1,
            Some("idle") => counts.idle += 1,
            Some(s) if s.starts_with("idle in transaction") => counts.idle_in_transaction += 1,
            _ => counts.other += 1,
        }
    }
    counts
}

pub fn map_settings(row: &Row) -> ServerSettings {
    ServerSettings {
        max_connections: row.get("max_connections"),
    }
}
```

Add `pub mod queries;` to `src/collector/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 8 passed`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: catalog SQL statements and row mapping"
```

---

## Task 4: Connection parameters and credentials

**Files:**
- Create: `src/connection/mod.rs`, `src/connection/params.rs`, `src/connection/credentials.rs`
- Modify: `src/lib.rs` (add `pub mod connection;`)

**Interfaces:**
- Consumes: nothing
- Produces:
  - `ConnectionParams { id: Uuid, label: String, host: String, port: u16, database: String, user: String, ssl_mode: SslMode }` — `Serialize`, `Deserialize`, and a **manual `Debug` that never prints a password** (it holds none, and the type must stay that way)
  - `enum SslMode { Disable, Prefer, Require }`
  - `impl ConnectionParams { pub fn to_config(&self, password: &str) -> tokio_postgres::Config }`
  - `pub fn store_password(id: &Uuid, password: &str) -> Result<(), CredentialError>`
  - `pub fn fetch_password(id: &Uuid) -> Result<Option<String>, CredentialError>`
  - `pub fn delete_password(id: &Uuid) -> Result<(), CredentialError>`

- [ ] **Step 1: Write the failing tests**

`src/connection/params.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ConnectionParams {
        ConnectionParams {
            id: Uuid::nil(),
            label: "prod".to_string(),
            host: "db.example.com".to_string(),
            port: 5432,
            database: "appdb".to_string(),
            user: "monitor".to_string(),
            ssl_mode: SslMode::Require,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let original = params();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ConnectionParams = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serialised_form_never_contains_a_password_field() {
        // The type holds no password by construction. This test fails loudly
        // if anyone ever adds one, because the JSON goes into GSettings.
        let json = serde_json::to_string(&params()).unwrap();
        assert!(!json.to_lowercase().contains("password"));
        assert!(!json.to_lowercase().contains("secret"));
    }

    #[test]
    fn builds_a_postgres_config_with_an_application_name() {
        let config = params().to_config("hunter2");
        assert_eq!(config.get_hosts().len(), 1);
        assert_eq!(config.get_ports(), &[5432]);
        assert_eq!(config.get_user(), Some("monitor"));
        assert_eq!(config.get_dbname(), Some("appdb"));
        assert_eq!(config.get_application_name(), Some("mission-centre-pg"));
    }

    #[test]
    fn debug_output_does_not_leak_the_password_argument() {
        // to_config takes the password but ConnectionParams never stores it,
        // so debugging the params can never expose it.
        let rendered = format!("{:?}", params());
        assert!(!rendered.contains("hunter2"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib params 2>&1 | head -20
```

Expected: FAIL — `cannot find type ConnectionParams`.

- [ ] **Step 3: Write `src/connection/params.rs`**

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
}

impl SslMode {
    fn to_pg(self) -> tokio_postgres::config::SslMode {
        match self {
            SslMode::Disable => tokio_postgres::config::SslMode::Disable,
            SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
            SslMode::Require => tokio_postgres::config::SslMode::Require,
        }
    }
}

/// Everything needed to reach a server *except* the password, which lives in
/// the system secret store. This type is serialised into GSettings, so it must
/// never gain a password field.
// Debug is hand-written below, not derived: a derived Debug would
// automatically print any field added later, including a password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionParams {
    pub id: Uuid,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub ssl_mode: SslMode,
}

impl ConnectionParams {
    pub fn to_config(&self, password: &str) -> tokio_postgres::Config {
        let mut config = tokio_postgres::Config::new();
        config
            .host(&self.host)
            .port(self.port)
            .dbname(&self.database)
            .user(&self.user)
            .password(password)
            .ssl_mode(self.ssl_mode.to_pg())
            .application_name("mission-centre-pg");
        config
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib params 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Write `src/connection/credentials.rs`**

There is no unit test here: the keyring talks to the live session secret store, and asserting against it in CI is not worth the fragility. It is exercised manually in Task 11.

```rust
use keyring::Entry;
use uuid::Uuid;

const SERVICE: &str = "mission-centre-pg";

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("secret store unavailable: {0}")]
    Store(#[from] keyring::Error),
}

fn entry(id: &Uuid) -> Result<Entry, CredentialError> {
    Ok(Entry::new(SERVICE, &id.to_string())?)
}

pub fn store_password(id: &Uuid, password: &str) -> Result<(), CredentialError> {
    entry(id)?.set_password(password)?;
    Ok(())
}

/// `Ok(None)` means no password has been stored for this server, which is a
/// normal state — not an error.
pub fn fetch_password(id: &Uuid) -> Result<Option<String>, CredentialError> {
    match entry(id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CredentialError::Store(e)),
    }
}

pub fn delete_password(id: &Uuid) -> Result<(), CredentialError> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(CredentialError::Store(e)),
    }
}
```

Create `src/connection/mod.rs`:

```rust
pub mod credentials;
pub mod params;
```

Add `pub mod connection;` to `src/lib.rs`.

- [ ] **Step 6: Build and run all tests**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 12 passed`.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: connection parameters and keyring credential storage"
```

---

## Task 5: Server probe and privilege detection

**Files:**
- Create: `src/connection/probe.rs`
- Modify: `src/connection/mod.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks
- Produces:
  - `enum PrivilegeLevel { Superuser, Monitor, Limited }`
  - `impl PrivilegeLevel { pub fn hides_other_sessions(&self) -> bool }`
  - `ServerInfo { version_num: i32, version_display: String, privilege: PrivilegeLevel }`
  - `pub const PROBE_SQL: &str`
  - `pub fn map_server_info(row: &tokio_postgres::Row) -> ServerInfo`
  - `pub fn format_version(version_num: i32) -> String`
  - `pub const MIN_SUPPORTED_VERSION: i32 = 140000`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_modern_version_number() {
        assert_eq!(format_version(140011), "14.11");
        assert_eq!(format_version(180004), "18.4");
        assert_eq!(format_version(160000), "16.0");
    }

    #[test]
    fn superuser_and_monitor_see_everything() {
        assert!(!PrivilegeLevel::Superuser.hides_other_sessions());
        assert!(!PrivilegeLevel::Monitor.hides_other_sessions());
    }

    #[test]
    fn a_limited_role_hides_other_sessions() {
        assert!(PrivilegeLevel::Limited.hides_other_sessions());
    }

    #[test]
    fn classifies_privilege_from_probe_flags() {
        assert_eq!(PrivilegeLevel::classify(true, true), PrivilegeLevel::Superuser);
        assert_eq!(PrivilegeLevel::classify(true, false), PrivilegeLevel::Superuser);
        assert_eq!(PrivilegeLevel::classify(false, true), PrivilegeLevel::Monitor);
        assert_eq!(PrivilegeLevel::classify(false, false), PrivilegeLevel::Limited);
    }

    #[test]
    fn recognises_versions_below_the_floor() {
        assert!(130015 < MIN_SUPPORTED_VERSION);
        assert!(140000 >= MIN_SUPPORTED_VERSION);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test --lib probe 2>&1 | head -20
```

Expected: FAIL — `cannot find function format_version`.

- [ ] **Step 3: Write the implementation**

```rust
use tokio_postgres::Row;

/// PostgreSQL 14. Connection is never refused on version grounds; pages that
/// need a newer server gate themselves. See spec §5.
pub const MIN_SUPPORTED_VERSION: i32 = 140000;

pub const PROBE_SQL: &str = "\
SELECT current_setting('server_version_num')::int AS version_num,
       pg_has_role(current_user, 'pg_monitor', 'member') AS is_monitor,
       COALESCE((SELECT rolsuper FROM pg_roles WHERE rolname = current_user), false) AS is_superuser";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeLevel {
    Superuser,
    Monitor,
    Limited,
}

impl PrivilegeLevel {
    pub fn classify(is_superuser: bool, is_monitor: bool) -> Self {
        if is_superuser {
            PrivilegeLevel::Superuser
        } else if is_monitor {
            PrivilegeLevel::Monitor
        } else {
            PrivilegeLevel::Limited
        }
    }

    /// True when PostgreSQL will return NULL query text for backends owned by
    /// other users, which the window must explain rather than silently render
    /// as blanks.
    pub fn hides_other_sessions(&self) -> bool {
        matches!(self, PrivilegeLevel::Limited)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub version_num: i32,
    pub version_display: String,
    pub privilege: PrivilegeLevel,
}

/// PostgreSQL 10 and later encode the version as MMmmmm: major * 10000 + minor.
pub fn format_version(version_num: i32) -> String {
    format!("{}.{}", version_num / 10000, version_num % 10000)
}

pub fn map_server_info(row: &Row) -> ServerInfo {
    let version_num: i32 = row.get("version_num");
    let is_monitor: bool = row.get("is_monitor");
    let is_superuser: bool = row.get("is_superuser");
    ServerInfo {
        version_num,
        version_display: format_version(version_num),
        privilege: PrivilegeLevel::classify(is_superuser, is_monitor),
    }
}
```

Add `pub mod probe;` to `src/connection/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 17 passed`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: server version and privilege probe"
```

---

## Task 6: Integration tests against PostgreSQL 14 and 18

This is the task that earns its keep: it is the only thing that can catch a column that does not exist on the floor version. Unit tests never will.

**Files:**
- Create: `tests/portability.rs`, `docs/development.md`
- Modify: none

**Interfaces:**
- Consumes: `DATABASE_STATS_SQL`, `ACTIVITY_SQL`, `SETTINGS_SQL`, `DATABASE_SIZE_SQL`, `map_database_counters`, `map_session`, `map_settings` (Task 3); `PROBE_SQL`, `map_server_info`, `PrivilegeLevel` (Task 5)
- Produces: a passing `cargo test --test portability`

`tests/` is an integration-test directory, so it can only use the crate's public API. The lib target already exists from Task 1, and Tasks 2–5 added `collector` and `connection` to `src/lib.rs`, so nothing needs restructuring here.

- [ ] **Step 1: Confirm the library exposes what the tests need**

```bash
grep -n "pub mod" src/lib.rs
```

Expected: `i18n`, `collector` and `connection` are all listed. Add any that are missing.

- [ ] **Step 2: Configure podman as the container runtime**

Docker is not installed on this machine; podman is. `testcontainers` speaks the Docker API, which podman serves.

```bash
systemctl --user enable --now podman.socket
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
podman info --format '{{.Host.RemoteSocket.Path}}'
```

Expected: prints the socket path. Record this in `docs/development.md`:

```markdown
# Development

## Running the integration tests

The portability tests start real PostgreSQL containers. This machine has podman
rather than docker, so point the Docker API client at podman's socket:

    systemctl --user enable --now podman.socket
    export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
    cargo test --test portability

The tests pull `docker.io/library/postgres:14` and `:18` on first run.
```

- [ ] **Step 3: Write the failing test**

`tests/portability.rs`:

```rust
//! Proves the Phase 1 SQL runs unchanged on the version floor and the newest
//! supported release. If a column is missing on PostgreSQL 14, this is what
//! catches it.

use mission_centre_pg::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use mission_centre_pg::collector::snapshot::DatabaseCounters;
use mission_centre_pg::connection::probe::{map_server_info, PrivilegeLevel, PROBE_SQL};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn connect(tag: &str) -> (tokio_postgres::Client, testcontainers::ContainerAsync<Postgres>) {
    let container = Postgres::default()
        .with_tag(tag)
        .start()
        .await
        .expect("failed to start the PostgreSQL container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to read the mapped port");

    let (client, connection) = tokio_postgres::Config::new()
        .host("127.0.0.1")
        .port(port)
        .user("postgres")
        .password("postgres")
        .dbname("postgres")
        .connect(tokio_postgres::NoTls)
        .await
        .expect("failed to connect to the container");

    tokio::spawn(async move {
        let _ = connection.await;
    });

    (client, container)
}

async fn assert_all_statements_run(tag: &str) {
    let (client, _container) = connect(tag).await;

    let probe = client.query_one(PROBE_SQL, &[]).await.expect("probe failed");
    let info = map_server_info(&probe);
    assert!(
        info.version_num >= 140000,
        "unexpected server version {}",
        info.version_display
    );
    assert_eq!(
        info.privilege,
        PrivilegeLevel::Superuser,
        "the container's postgres role should be a superuser"
    );

    let rows = client
        .query(DATABASE_STATS_SQL, &[])
        .await
        .expect("pg_stat_database query failed");
    assert!(!rows.is_empty(), "pg_stat_database returned no rows");
    let counters: Vec<DatabaseCounters> = rows.iter().map(map_database_counters).collect();
    let totals = DatabaseCounters::sum(&counters);
    assert!(totals.xact_commit > 0, "expected some committed transactions");

    let rows = client
        .query(ACTIVITY_SQL, &[])
        .await
        .expect("pg_stat_activity query failed");
    let sessions: Vec<_> = rows.iter().map(map_session).collect();
    let _ = count_sessions(&sessions);

    let row = client
        .query_one(SETTINGS_SQL, &[])
        .await
        .expect("settings query failed");
    assert!(map_settings(&row).max_connections > 0);

    let row = client
        .query_one(DATABASE_SIZE_SQL, &[])
        .await
        .expect("database size query failed");
    let size: i64 = row.get("size");
    assert!(size > 0, "database size should be positive");
}

#[tokio::test]
async fn all_statements_run_on_postgres_14() {
    assert_all_statements_run("14").await;
}

#[tokio::test]
async fn all_statements_run_on_postgres_18() {
    assert_all_statements_run("18").await;
}

#[tokio::test]
async fn a_role_without_pg_monitor_is_classified_as_limited() {
    let (client, _container) = connect("18").await;
    client
        .batch_execute("CREATE ROLE watcher LOGIN PASSWORD 'watcher'")
        .await
        .expect("failed to create the limited role");

    let limited = client
        .query_one(
            "SELECT pg_has_role('watcher', 'pg_monitor', 'member') AS is_monitor,
                    (SELECT rolsuper FROM pg_roles WHERE rolname = 'watcher') AS is_superuser",
            &[],
        )
        .await
        .expect("privilege query failed");
    let is_monitor: bool = limited.get("is_monitor");
    let is_superuser: bool = limited.get("is_superuser");
    assert_eq!(
        PrivilegeLevel::classify(is_superuser, is_monitor),
        PrivilegeLevel::Limited
    );
}
```

- [ ] **Step 4: Run the tests**

```bash
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability 2>&1 | tail -20
```

Expected: `test result: ok. 3 passed`. The first run pulls two images and takes a few minutes.

If a statement fails on 14, that is the test doing its job — fix the SQL in `src/collector/queries.rs` so one statement satisfies both versions, and re-run.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A
git commit -m "test: prove Phase 1 SQL runs on PostgreSQL 14 and 18"
```

---

## Task 7: The collector thread

**Files:**
- Create: `src/collector/worker.rs`
- Modify: `src/collector/mod.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–5
- Produces:
  - `enum CollectorEvent { Connecting, Connected(ServerInfo), Sample(Box<Snapshot>), Error(CollectorError), Disconnected }`
  - `enum CollectorError { Connect(String), Query(String), Timeout, LostConnection }` — each with a user-facing `Display`
  - `struct CollectorHandle { events: async_channel::Receiver<CollectorEvent> }`
  - `pub fn spawn(params: ConnectionParams, password: String, interval: Duration) -> CollectorHandle`
  - `impl CollectorHandle { pub fn stop(&self) }`
  - `pub fn backoff_delay(consecutive_failures: u32) -> Duration`

- [ ] **Step 1: Write the failing test for the backoff schedule**

The sampling loop itself is proven by Task 6's containers; the backoff schedule is pure and worth pinning down.

In `src/collector/worker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_then_caps_at_thirty_seconds() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(50), Duration::from_secs(30));
    }

    #[test]
    fn errors_render_without_exposing_connection_details() {
        let error = CollectorError::Connect("password authentication failed".to_string());
        let rendered = error.to_string();
        assert!(rendered.contains("password authentication failed"));
        assert!(!rendered.contains("postgresql://"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib worker 2>&1 | head -20
```

Expected: FAIL — `cannot find function backoff_delay`.

- [ ] **Step 3: Write the implementation**

```rust
use std::time::{Duration, Instant};

use tokio_postgres::Client;

use crate::collector::queries::{
    count_sessions, map_database_counters, map_session, map_settings, ACTIVITY_SQL,
    DATABASE_SIZE_SQL, DATABASE_STATS_SQL, SETTINGS_SQL,
};
use crate::collector::rates::derive_rates;
use crate::collector::snapshot::{DatabaseCounters, ServerSettings, Snapshot};
use crate::connection::params::ConnectionParams;
use crate::connection::probe::{map_server_info, ServerInfo, PROBE_SQL};

/// Guards against a wedged server hanging the sampler for ever.
const STATEMENT_TIMEOUT: &str = "SET statement_timeout = '5s'";

/// Consecutive failed samples before the collector declares the connection lost.
const FAILURES_BEFORE_DISCONNECT: u32 = 3;

#[derive(Debug, Clone, thiserror::Error)]
pub enum CollectorError {
    #[error("Could not connect: {0}")]
    Connect(String),
    #[error("Query failed: {0}")]
    Query(String),
    #[error("The server did not respond within five seconds")]
    Timeout,
    #[error("The connection to the server was lost")]
    LostConnection,
}

#[derive(Debug, Clone)]
pub enum CollectorEvent {
    Connecting,
    Connected(ServerInfo),
    Sample(Box<Snapshot>),
    Error(CollectorError),
    Disconnected,
}

pub struct CollectorHandle {
    pub events: async_channel::Receiver<CollectorEvent>,
    stop: async_channel::Sender<()>,
}

impl CollectorHandle {
    pub fn stop(&self) {
        let _ = self.stop.try_send(());
    }
}

/// 1s, 2s, 4s, 8s, 16s, then 30s for ever.
pub fn backoff_delay(consecutive_failures: u32) -> Duration {
    // saturating_shl is not a stable integer method; min() already bounds the shift.
    let seconds = 1u64 << consecutive_failures.min(16);
    Duration::from_secs(seconds.min(30))
}

pub fn spawn(
    params: ConnectionParams,
    password: String,
    interval: Duration,
) -> CollectorHandle {
    let (event_tx, event_rx) = async_channel::bounded(32);
    let (stop_tx, stop_rx) = async_channel::bounded(1);

    std::thread::Builder::new()
        .name("mcpg-collector".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the collector runtime");
            runtime.block_on(run(params, password, interval, event_tx, stop_rx));
        })
        .expect("failed to spawn the collector thread");

    CollectorHandle {
        events: event_rx,
        stop: stop_tx,
    }
}

async fn run(
    params: ConnectionParams,
    password: String,
    interval: Duration,
    events: async_channel::Sender<CollectorEvent>,
    stop: async_channel::Receiver<()>,
) {
    let mut consecutive_connect_failures = 0u32;

    loop {
        if stop.try_recv().is_ok() {
            return;
        }

        let _ = events.send(CollectorEvent::Connecting).await;

        match connect(&params, &password).await {
            Ok((client, info)) => {
                consecutive_connect_failures = 0;
                let _ = events.send(CollectorEvent::Connected(info)).await;
                sample_loop(&client, interval, &events, &stop).await;
                let _ = events.send(CollectorEvent::Disconnected).await;
            }
            Err(e) => {
                let _ = events.send(CollectorEvent::Error(e)).await;
                let delay = backoff_delay(consecutive_connect_failures);
                consecutive_connect_failures = consecutive_connect_failures.saturating_add(1);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = stop.recv() => return,
                }
            }
        }
    }
}

async fn connect(
    params: &ConnectionParams,
    password: &str,
) -> Result<(Client, ServerInfo), CollectorError> {
    let config = params.to_config(password);

    let (client, connection) = config
        .connect(tokio_postgres::NoTls)
        .await
        .map_err(|e| CollectorError::Connect(e.to_string()))?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    client
        .batch_execute(STATEMENT_TIMEOUT)
        .await
        .map_err(|e| CollectorError::Query(e.to_string()))?;

    let row = client
        .query_one(PROBE_SQL, &[])
        .await
        .map_err(|e| CollectorError::Query(e.to_string()))?;

    Ok((client, map_server_info(&row)))
}

/// Samples serially: the next sample starts only once the previous one has
/// finished or timed out, so a slow server spreads samples out rather than
/// piling overlapping queries onto one connection.
async fn sample_loop(
    client: &Client,
    interval: Duration,
    events: &async_channel::Sender<CollectorEvent>,
    stop: &async_channel::Receiver<()>,
) {
    let mut previous: Option<(DatabaseCounters, Instant)> = None;
    let mut consecutive_failures = 0u32;

    loop {
        if stop.try_recv().is_ok() {
            return;
        }

        match sample(client, previous).await {
            Ok(snapshot) => {
                consecutive_failures = 0;
                previous = Some((snapshot.totals, snapshot.taken_at));
                let _ = events.send(CollectorEvent::Sample(Box::new(snapshot))).await;
            }
            Err(e) => {
                consecutive_failures += 1;
                let _ = events.send(CollectorEvent::Error(e)).await;
                if consecutive_failures >= FAILURES_BEFORE_DISCONNECT {
                    return;
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stop.recv() => return,
        }
    }
}

async fn sample(
    client: &Client,
    previous: Option<(DatabaseCounters, Instant)>,
) -> Result<Snapshot, CollectorError> {
    let taken_at = Instant::now();

    let stat_rows = client
        .query(DATABASE_STATS_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let per_database: Vec<DatabaseCounters> =
        stat_rows.iter().map(map_database_counters).collect();
    let totals = DatabaseCounters::sum(&per_database);

    let activity_rows = client
        .query(ACTIVITY_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let sessions: Vec<_> = activity_rows.iter().map(map_session).collect();
    let session_counts = count_sessions(&sessions);

    let settings_row = client
        .query_one(SETTINGS_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let settings: ServerSettings = map_settings(&settings_row);

    let size_row = client
        .query_one(DATABASE_SIZE_SQL, &[])
        .await
        .map_err(map_query_error)?;
    let connected_database_size_bytes: Option<i64> = size_row.get("size");

    let rates = previous.and_then(|(prev_counters, prev_at)| {
        derive_rates(&prev_counters, &totals, taken_at.duration_since(prev_at))
    });

    Ok(Snapshot {
        taken_at,
        totals,
        rates,
        connected_database_size_bytes,
        session_counts,
        sessions,
        settings,
    })
}

fn map_query_error(e: tokio_postgres::Error) -> CollectorError {
    let text = e.to_string();
    if text.contains("statement timeout") || text.contains("canceling statement") {
        CollectorError::Timeout
    } else if e.is_closed() {
        CollectorError::LostConnection
    } else {
        CollectorError::Query(text)
    }
}
```

Add `pub mod worker;` to `src/collector/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 19 passed`.

- [ ] **Step 5: Verify against the local server manually**

Write a scratch example to prove the collector really samples. Create `examples/collect.rs`:

```rust
use std::time::Duration;

use mission_centre_pg::collector::worker::{spawn, CollectorEvent};
use mission_centre_pg::connection::params::{ConnectionParams, SslMode};

fn main() {
    let params = ConnectionParams {
        id: uuid::Uuid::nil(),
        label: "local".to_string(),
        host: "/run/postgresql".to_string(),
        port: 5432,
        database: std::env::var("PGDATABASE").unwrap_or_else(|_| "postgres".to_string()),
        user: std::env::var("USER").unwrap_or_else(|_| "postgres".to_string()),
        ssl_mode: SslMode::Disable,
    };

    let handle = spawn(params, String::new(), Duration::from_secs(1));
    for _ in 0..4 {
        match handle.events.recv_blocking() {
            Ok(CollectorEvent::Connected(info)) => println!("connected: {info:?}"),
            Ok(CollectorEvent::Sample(s)) => {
                println!("sessions={} rates={:?}", s.sessions.len(), s.rates)
            }
            Ok(other) => println!("{other:?}"),
            Err(_) => break,
        }
    }
    handle.stop();
}
```

Add `uuid = { version = "1.24", features = ["v4"] }` to `[dev-dependencies]` if the example does not compile.

```bash
cargo run --example collect
```

Expected: `connected: ServerInfo { version_num: 180004, ... }`, then samples. The **first** sample prints `rates=None` — that is correct, there is no previous reading to compare against. Subsequent samples print `rates=Some(...)`.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: collector thread with serial sampling and reconnect backoff"
```

---

## Task 8: Vendor the graph widgets

**Files:**
- Create: `src/widgets/mod.rs`, `src/widgets/graph_widget.rs`, `src/widgets/graph_widget_utils.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `GraphWidget` (a `gtk::Widget` subclass) with its upstream API intact

Source: `/home/paul/gitlab/mission-center`, commit `050213c`. Spec §9 records exactly what may be changed.

- [ ] **Step 1: Copy the two files**

```bash
cd /home/paul/gitHUB/mission-centre-postgresql
mkdir -p src/widgets
cp /home/paul/gitlab/mission-center/src/performance_page/widgets/graph_widget.rs src/widgets/
cp /home/paul/gitlab/mission-center/src/performance_page/widgets/graph_widget_utils.rs src/widgets/
```

- [ ] **Step 2: Add the provenance note to both files**

Insert directly beneath the existing `Copyright 2026 Mission Center Developers` line in each file:

```rust
 * Vendored into Mission Centre PostgreSQL from Mission Center
 * (https://gitlab.com/mission-center-devs/mission-center) at commit 050213c,
 * originally src/performance_page/widgets/graph_widget.rs.
```

Use the matching original path in `graph_widget_utils.rs`.

- [ ] **Step 3: Fix the two imports in `graph_widget_utils.rs`**

Replace:

```rust
use crate::performance_page::widgets::GraphWidget;
use crate::preferences::{MAX_POINTS, MIN_POINTS};
```

with:

```rust
use crate::widgets::graph_widget::GraphWidget;

/// Upstream sources these from `crate::preferences`; Phase 1 has no
/// preferences module, so the same values live here.
pub const MAX_POINTS: i32 = 600;
pub const MIN_POINTS: i32 = 10;
```

- [ ] **Step 4: Fix the import in `graph_widget.rs`**

Replace:

```rust
use crate::performance_page::widgets::graph_widget_utils::{
```

with:

```rust
use crate::widgets::graph_widget_utils::{
```

- [ ] **Step 5: Write `src/widgets/mod.rs`**

```rust
pub mod graph_widget;
pub mod graph_widget_utils;

pub use graph_widget::GraphWidget;
```

Add `pub mod widgets;` to `src/lib.rs`.

- [ ] **Step 6: Build**

```bash
cargo build 2>&1 | tail -30
```

Expected: compiles. If the compiler reports further `crate::` paths that do not resolve, fix the path only — do not restructure the vendored code. If it reports a missing `SidebarDropHint`, that import belongs to a feature Phase 1 does not use: delete the import and the code path that uses it, and note the deletion in the provenance comment.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: vendor GraphWidget from Mission Center (commit 050213c)"
```

---

## Task 9: Sidebar row with a live sparkline

**Files:**
- Create: `src/widgets/sidebar_row.rs`, `resources/ui/sidebar_row.blp`
- Modify: `src/widgets/mod.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `GraphWidget` and `DatasetGroup` (Task 8)
- Produces:
  - `enum ConnectionState { Disconnected, Connecting, Connected, Failed }`
  - `McpgSidebarRow` widget with `set_heading(&str)`, `set_subheading(&str)`,
    `set_state(ConnectionState)`, `push_value(f64)`, `reset_series()`

### How `GraphWidget` actually works

Task 8 established its real API, which is **not** what earlier drafts of this plan assumed.
There is no "load a whole series" setter. It is a streaming widget that owns its own bounded
ring buffer per dataset:

```rust
let graph = GraphWidget::new(None);            // or Some(&settings)
graph.set_data_points(60);                     // ring-buffer CAPACITY, a u32 property
graph.add_dataset(DatasetGroup::new());        // register one series, once
graph.add_data_point(vec![vec![value as f32]]); // push one row per tick
```

`set_data_points` sets the **capacity**, not the values — the name is misleading, so read it
carefully. `add_data_point` takes one inner `Vec<f32>` per registered `DatasetGroup`, in the
order they were added. Values are `f32`.

Because the widget owns the buffer, this project keeps **no parallel ring buffer of its own**.
The widget outlives any single connection, so a reconnect does not wipe the history — which is
what the spec asks for.

- [ ] **Step 1: Write `resources/ui/sidebar_row.blp`**

```blueprint
using Gtk 4.0;

template $McpgSidebarRow: Gtk.Box {
  orientation: horizontal;
  spacing: 12;

  Gtk.Image state_icon {
    icon-name: "media-playback-stop-symbolic";
    valign: center;
  }

  Gtk.Box {
    orientation: vertical;
    hexpand: true;
    valign: center;

    Gtk.Label heading_label {
      xalign: 0;
      ellipsize: end;
      styles ["heading"]
    }

    Gtk.Label subheading_label {
      xalign: 0;
      ellipsize: end;
      styles ["caption", "dim-label"]
    }
  }

  $GraphWidget graph {
    width-request: 72;
    height-request: 28;
    valign: center;
  }
}
```

If blueprint-compiler rejects the `$GraphWidget` custom-type syntax, drop the graph from the
template, put a `Gtk.Box graph_holder {}` in its place, and append a `GraphWidget` to it from
Rust in `constructed()`. Say which route you took.

- [ ] **Step 2: Write `src/widgets/sidebar_row.rs`**

```rust
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::widgets::graph_widget::GraphWidget;
use crate::widgets::graph_widget_utils::DatasetGroup;

/// Points retained in a sidebar sparkline. Deliberately shorter than the
/// full pages' graphs — the row is 72px wide.
const SPARKLINE_POINTS: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

impl ConnectionState {
    fn icon_name(self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "media-playback-stop-symbolic",
            ConnectionState::Connecting => "content-loading-symbolic",
            ConnectionState::Connected => "media-record-symbolic",
            ConnectionState::Failed => "dialog-warning-symbolic",
        }
    }
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/sidebar_row.ui")]
    pub struct McpgSidebarRow {
        #[template_child]
        pub state_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub heading_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub subheading_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub graph: TemplateChild<GraphWidget>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgSidebarRow {
        const NAME: &'static str = "McpgSidebarRow";
        type Type = super::McpgSidebarRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            GraphWidget::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgSidebarRow {
        fn constructed(&self) {
            self.parent_constructed();
            self.graph.set_data_points(SPARKLINE_POINTS);
            self.graph.add_dataset(DatasetGroup::new());
        }
    }

    impl WidgetImpl for McpgSidebarRow {}
    impl BoxImpl for McpgSidebarRow {}
}

glib::wrapper! {
    pub struct McpgSidebarRow(ObjectSubclass<imp::McpgSidebarRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgSidebarRow {
    pub fn new(heading: &str) -> Self {
        let row: Self = glib::Object::new();
        row.set_heading(heading);
        row
    }

    pub fn set_heading(&self, text: &str) {
        self.imp().heading_label.set_text(text);
    }

    pub fn set_subheading(&self, text: &str) {
        self.imp().subheading_label.set_text(text);
    }

    pub fn set_state(&self, state: ConnectionState) {
        self.imp().state_icon.set_icon_name(Some(state.icon_name()));
    }

    /// Append one point to the sparkline.
    pub fn push_value(&self, value: f64) {
        self.imp().graph.add_data_point(vec![vec![value as f32]]);
    }

    /// Drop the series so selecting a different server does not inherit the
    /// previous one's shape.
    pub fn reset_series(&self) {
        let graph = self.imp().graph.get();
        graph.clear_datasets();
        graph.add_dataset(DatasetGroup::new());
    }
}

impl Default for McpgSidebarRow {
    fn default() -> Self {
        Self::new("")
    }
}
```

- [ ] **Step 3: Update `src/widgets/mod.rs`**

```rust
pub mod graph_widget;
pub mod graph_widget_utils;
pub mod sidebar_row;

pub use graph_widget::GraphWidget;
pub use sidebar_row::{ConnectionState, McpgSidebarRow};
```

- [ ] **Step 4: Register the new Blueprint**

In `resources/meson.build`, change the `input:` line to:

```meson
  input: files('ui/window.blp', 'ui/sidebar_row.blp'),
```

In `resources/mission-centre-pg.gresource.xml`, add inside `<gresource>`:

```xml
    <file preprocess="xml-stripblanks">ui/sidebar_row.ui</file>
```

- [ ] **Step 5: Build**

```bash
ninja -C build 2>&1 | tail -20
cargo test --lib 2>&1 | tail -5
```

Expected: build succeeds; the existing 23 tests still pass. There are no new unit tests —
this is a widget with no logic to assert beyond what the compiler checks.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: sidebar row with a live sparkline"
```

---

## Task 10: Server registry persisted to GSettings

**Files:**
- Create: `src/connection/registry.rs`
- Modify: `src/connection/mod.rs`

**Interfaces:**
- Consumes: `ConnectionParams` (Task 4)
- Produces:
  - `pub fn load(settings: &gio::Settings) -> Vec<ConnectionParams>`
  - `pub fn save(settings: &gio::Settings, servers: &[ConnectionParams]) -> Result<(), glib::BoolError>`
  - `pub fn parse(json: &str) -> Vec<ConnectionParams>` and `pub fn serialise(servers: &[ConnectionParams]) -> String` — the pure halves, which are what the tests exercise

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::params::SslMode;

    fn server(label: &str) -> ConnectionParams {
        ConnectionParams {
            id: Uuid::nil(),
            label: label.to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "postgres".to_string(),
            user: "paul".to_string(),
            ssl_mode: SslMode::Prefer,
        }
    }

    #[test]
    fn round_trips_a_list_of_servers() {
        let servers = vec![server("one"), server("two")];
        let parsed = parse(&serialise(&servers));
        assert_eq!(parsed, servers);
    }

    #[test]
    fn an_empty_setting_yields_no_servers() {
        assert!(parse("[]").is_empty());
    }

    #[test]
    fn corrupt_json_yields_no_servers_rather_than_panicking() {
        // A hand-edited or downgraded setting must not crash the application
        // on startup.
        assert!(parse("{ not json").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn serialised_servers_never_contain_a_password() {
        let json = serialise(&[server("prod")]);
        assert!(!json.to_lowercase().contains("password"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --lib registry 2>&1 | head -20
```

Expected: FAIL — `cannot find function parse`.

- [ ] **Step 3: Write the implementation**

```rust
use gtk::prelude::SettingsExt;
use gtk::{gio, glib};
use uuid::Uuid;

use crate::connection::params::ConnectionParams;

const KEY: &str = "servers";

/// A malformed value yields an empty list rather than a panic: the setting is
/// user-editable, and a bad edit must not stop the application starting.
pub fn parse(json: &str) -> Vec<ConnectionParams> {
    serde_json::from_str(json).unwrap_or_default()
}

pub fn serialise(servers: &[ConnectionParams]) -> String {
    serde_json::to_string(servers).unwrap_or_else(|_| "[]".to_string())
}

pub fn load(settings: &gio::Settings) -> Vec<ConnectionParams> {
    parse(settings.string(KEY).as_str())
}

pub fn save(
    settings: &gio::Settings,
    servers: &[ConnectionParams],
) -> Result<(), glib::BoolError> {
    settings.set_string(KEY, &serialise(servers))
}
```

Add `pub mod registry;` to `src/connection/mod.rs`.

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 28 passed`.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: server registry persisted to GSettings"
```

---

## Task 11: Add Server dialog

**Files:**
- Create: `src/dialogs/mod.rs`, `src/dialogs/add_server.rs`, `resources/ui/add_server_dialog.blp`
- Modify: `src/lib.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `ConnectionParams`, `SslMode` (Task 4); `credentials::store_password` (Task 4)
- Produces: `AddServerDialog` with `connect_added(F)` where `F: Fn(&ConnectionParams) + 'static`, emitting after the password has been stored in the keyring

- [ ] **Step 1: Write `resources/ui/add_server_dialog.blp`**

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgAddServerDialog: Adw.Dialog {
  title: _("Add Server");
  content-width: 460;

  child: Adw.ToolbarView {
    [top]
    Adw.HeaderBar {
      show-end-title-buttons: false;

      [start]
      Gtk.Button cancel_button {
        label: _("Cancel");
      }

      [end]
      Gtk.Button add_button {
        label: _("Add");
        styles ["suggested-action"]
      }
    }

    content: Adw.PreferencesPage {
      Adw.PreferencesGroup {
        Adw.EntryRow label_row {
          title: _("Name");
        }

        Adw.EntryRow host_row {
          title: _("Host");
        }

        Adw.EntryRow port_row {
          title: _("Port");
        }

        Adw.EntryRow database_row {
          title: _("Database");
        }

        Adw.EntryRow user_row {
          title: _("User");
        }

        Adw.PasswordEntryRow password_row {
          title: _("Password");
        }

        Adw.ComboRow ssl_row {
          title: _("SSL Mode");
          model: Gtk.StringList {
            strings [_("Disable"), _("Prefer"), _("Require")]
          };
          selected: 1;
        }
      }
    };
  };
}
```

- [ ] **Step 2: Write `src/dialogs/add_server.rs`**

```rust
use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use uuid::Uuid;

use crate::connection::credentials;
use crate::connection::params::{ConnectionParams, SslMode};

type AddedCallback = Box<dyn Fn(&ConnectionParams)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/add_server_dialog.ui")]
    pub struct McpgAddServerDialog {
        #[template_child]
        pub label_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub host_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub port_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub database_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub user_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub password_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub ssl_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,

        pub on_added: RefCell<Option<AddedCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgAddServerDialog {
        const NAME: &'static str = "McpgAddServerDialog";
        type Type = super::McpgAddServerDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgAddServerDialog {
        fn constructed(&self) {
            self.parent_constructed();

            self.host_row.set_text("localhost");
            self.port_row.set_text("5432");
            self.database_row.set_text("postgres");

            let dialog = self.obj().clone();
            self.cancel_button.connect_clicked(move |_| dialog.close());

            let dialog = self.obj().clone();
            self.add_button.connect_clicked(move |_| dialog.submit());
        }
    }

    impl WidgetImpl for McpgAddServerDialog {}
    impl AdwDialogImpl for McpgAddServerDialog {}
}

glib::wrapper! {
    pub struct McpgAddServerDialog(ObjectSubclass<imp::McpgAddServerDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl McpgAddServerDialog {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn connect_added<F: Fn(&ConnectionParams) + 'static>(&self, callback: F) {
        self.imp().on_added.replace(Some(Box::new(callback)));
    }

    fn submit(&self) {
        let imp = self.imp();

        let host = imp.host_row.text().trim().to_string();
        if host.is_empty() {
            imp.host_row.add_css_class("error");
            return;
        }
        imp.host_row.remove_css_class("error");

        let port: u16 = match imp.port_row.text().trim().parse() {
            Ok(port) => port,
            Err(_) => {
                imp.port_row.add_css_class("error");
                return;
            }
        };
        imp.port_row.remove_css_class("error");

        let label = imp.label_row.text().trim().to_string();
        let label = if label.is_empty() {
            format!("{host}:{port}")
        } else {
            label
        };

        let params = ConnectionParams {
            id: Uuid::new_v4(),
            label,
            host,
            port,
            database: imp.database_row.text().trim().to_string(),
            user: imp.user_row.text().trim().to_string(),
            ssl_mode: match imp.ssl_row.selected() {
                0 => SslMode::Disable,
                2 => SslMode::Require,
                _ => SslMode::Prefer,
            },
        };

        // The password goes straight to the secret store and is never held on
        // ConnectionParams, which is serialised into GSettings.
        let password = imp.password_row.text();
        if let Err(e) = credentials::store_password(&params.id, &password) {
            gtk::glib::g_warning!("mission-centre-pg", "could not store the password: {e}");
        }

        if let Some(callback) = imp.on_added.borrow().as_ref() {
            callback(&params);
        }
        self.close();
    }
}

impl Default for McpgAddServerDialog {
    fn default() -> Self {
        Self::new()
    }
}
```

`src/dialogs/mod.rs`:

```rust
pub mod add_server;

pub use add_server::McpgAddServerDialog;
```

Add `pub mod dialogs;` to `src/lib.rs`.

- [ ] **Step 3: Register the Blueprint**

Add `'ui/add_server_dialog.blp'` to the `input:` files in `resources/meson.build`, and

```xml
    <file preprocess="xml-stripblanks">ui/add_server_dialog.ui</file>
```

to `resources/mission-centre-pg.gresource.xml`.

- [ ] **Step 4: Build**

```bash
ninja -C build 2>&1 | tail -20
```

Expected: build succeeds. The dialog is not yet reachable from the UI — Task 13 wires it up.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: Add Server dialog storing credentials in the secret store"
```

---

## Task 12: Overview and Sessions pages

**Files:**
- Create: `src/pages/mod.rs`, `src/pages/overview.rs`, `src/pages/sessions.rs`, `resources/ui/overview_page.blp`, `resources/ui/sessions_page.blp`
- Modify: `src/lib.rs`, `resources/meson.build`, `resources/mission-centre-pg.gresource.xml`

**Interfaces:**
- Consumes: `Snapshot`, `DatabaseRates`, `SessionCounts`, `Session` (Task 2); `GraphWidget` and `DatasetGroup` (Task 8)
- Produces:
  - `OverviewPage` with `pub fn update(&self, snapshot: &Snapshot)` and `pub fn set_graph_points(&self, points: usize)`
  - `SessionsPage` with `pub fn update(&self, sessions: &[Session])`, `pub fn set_hide_idle(&self, hide: bool)`, and `pub fn set_privilege_limited(&self, limited: bool)`
  - `pub fn format_rate(value: f64) -> String` and `pub fn format_bytes(bytes: i64) -> String` in `src/pages/format.rs`

- [ ] **Step 1: Write the failing tests for the formatters**

Create `src/pages/format.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rates_with_thousands_separators() {
        assert_eq!(format_rate(1284.0), "1,284");
        assert_eq!(format_rate(0.0), "0");
        assert_eq!(format_rate(999.0), "999");
        assert_eq!(format_rate(1_234_567.0), "1,234,567");
    }

    #[test]
    fn formats_small_rates_with_one_decimal() {
        assert_eq!(format_rate(0.4), "0.4");
        assert_eq!(format_rate(9.6), "9.6");
    }

    #[test]
    fn formats_bytes_in_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
        assert_eq!(format_bytes(3_221_225_472), "3.0 GiB");
    }

    #[test]
    fn formats_a_ratio_as_a_percentage() {
        assert_eq!(format_ratio(Some(0.9987)), "99.9%");
        assert_eq!(format_ratio(Some(0.0)), "0.0%");
    }

    #[test]
    fn an_absent_ratio_renders_as_a_dash_not_zero() {
        // No blocks were accessed this interval. Showing 0% would claim every
        // read missed cache, which is not what happened.
        assert_eq!(format_ratio(None), "—");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --lib format 2>&1 | head -20
```

Expected: FAIL — `cannot find function format_rate`.

- [ ] **Step 3: Write the formatters**

```rust
pub fn format_rate(value: f64) -> String {
    if value > 0.0 && value < 10.0 {
        return format!("{value:.1}");
    }
    let rounded = value.round() as i64;
    let digits = rounded.abs().to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if rounded < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `None` renders as an em dash. Rendering it as 0% would assert that every
/// block read missed cache, which is not what an absent ratio means.
pub fn format_ratio(ratio: Option<f64>) -> String {
    match ratio {
        Some(value) => format!("{:.1}%", value * 100.0),
        None => "—".to_string(),
    }
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: `test result: ok. 33 passed`.

- [ ] **Step 5: Write `resources/ui/overview_page.blp`**

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgOverviewPage: Gtk.Box {
  orientation: vertical;

  Gtk.ScrolledWindow {
    vexpand: true;
    hscrollbar-policy: never;

    child: Adw.Clamp {
      maximum-size: 900;
      margin-start: 18;
      margin-end: 18;
      margin-top: 18;
      margin-bottom: 18;

      child: Gtk.Box {
        orientation: vertical;
        spacing: 24;

        Gtk.Box {
          orientation: vertical;
          spacing: 6;

          Gtk.Box {
            Gtk.Label { label: _("Connections"); xalign: 0; hexpand: true; styles ["heading"] }
            Gtk.Label connections_value { xalign: 1; styles ["numeric"] }
          }

          $GraphWidget connections_graph { height-request: 120; }
        }

        Gtk.Box {
          orientation: vertical;
          spacing: 6;

          Gtk.Box {
            Gtk.Label { label: _("Transactions per second"); xalign: 0; hexpand: true; styles ["heading"] }
            Gtk.Label tps_value { xalign: 1; styles ["numeric"] }
          }

          $GraphWidget tps_graph { height-request: 120; }
        }

        Gtk.Box {
          orientation: vertical;
          spacing: 6;

          Gtk.Box {
            Gtk.Label { label: _("Cache hit ratio"); xalign: 0; hexpand: true; styles ["heading"] }
            Gtk.Label cache_value { xalign: 1; styles ["numeric"] }
          }

          $GraphWidget cache_graph { height-request: 120; }
        }

        Gtk.Box {
          orientation: vertical;
          spacing: 6;

          Gtk.Box {
            Gtk.Label { label: _("Tuples returned per second"); xalign: 0; hexpand: true; styles ["heading"] }
            Gtk.Label tuples_value { xalign: 1; styles ["numeric"] }
          }

          $GraphWidget tuples_graph { height-request: 120; }
        }

        Adw.PreferencesGroup {
          title: _("Server");

          Adw.ActionRow database_size_row {
            title: _("Database size");
            [suffix]
            Gtk.Label database_size_value { styles ["numeric", "dim-label"] }
          }

          Adw.ActionRow deadlocks_row {
            title: _("Deadlocks per second");
            [suffix]
            Gtk.Label deadlocks_value { styles ["numeric", "dim-label"] }
          }

          Adw.ActionRow temp_row {
            title: _("Temporary bytes per second");
            [suffix]
            Gtk.Label temp_value { styles ["numeric", "dim-label"] }
          }
        }
      };
    };
  }
}
```

- [ ] **Step 6: Write `src/pages/overview.rs`**

`GraphWidget` owns its own ring buffer (see Task 9). Register one `DatasetGroup` per graph at
construction, set the capacity, then push one value per sample. Keep no parallel buffer.

```rust
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::collector::snapshot::Snapshot;
use crate::i18n::i18n_f;
use crate::pages::format::{format_bytes, format_ratio, format_rate};
use crate::widgets::graph_widget::GraphWidget;
use crate::widgets::graph_widget_utils::DatasetGroup;

const DEFAULT_POINTS: u32 = 300;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/overview_page.ui")]
    pub struct McpgOverviewPage {
        #[template_child]
        pub connections_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub connections_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub tps_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub tps_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub cache_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub cache_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub tuples_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub tuples_graph: TemplateChild<GraphWidget>,
        #[template_child]
        pub database_size_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub deadlocks_value: TemplateChild<gtk::Label>,
        #[template_child]
        pub temp_value: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgOverviewPage {
        const NAME: &'static str = "McpgOverviewPage";
        type Type = super::McpgOverviewPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            GraphWidget::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgOverviewPage {
        fn constructed(&self) {
            self.parent_constructed();
            for graph in self.obj().graphs() {
                graph.set_data_points(DEFAULT_POINTS);
                graph.add_dataset(DatasetGroup::new());
            }
            // A ratio is always 0-100, so pin the scale rather than letting it
            // auto-fit and make a flat 99% line look dramatic.
            self.cache_graph.set_dataset_max_scale(0, 100.0);
        }
    }

    impl WidgetImpl for McpgOverviewPage {}
    impl BoxImpl for McpgOverviewPage {}
}

glib::wrapper! {
    pub struct McpgOverviewPage(ObjectSubclass<imp::McpgOverviewPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgOverviewPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn graphs(&self) -> [GraphWidget; 4] {
        let imp = self.imp();
        [
            imp.connections_graph.get(),
            imp.tps_graph.get(),
            imp.cache_graph.get(),
            imp.tuples_graph.get(),
        ]
    }

    pub fn set_graph_points(&self, points: u32) {
        for graph in self.graphs() {
            graph.set_data_points(points);
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        let imp = self.imp();

        let connections = snapshot.session_counts.total() as f64;
        let max_connections = snapshot.settings.max_connections;
        imp.connections_value.set_text(&i18n_f(
            "{} / {}",
            &[&format_rate(connections), &max_connections.to_string()],
        ));
        imp.connections_graph
            .set_dataset_max_scale(0, max_connections as f32);
        imp.connections_graph
            .add_data_point(vec![vec![connections as f32]]);

        imp.database_size_value.set_text(
            &snapshot
                .connected_database_size_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "—".to_string()),
        );

        // The first sample after connecting has no previous reading, so there
        // are no rates yet. Push nothing rather than a fabricated zero.
        let Some(rates) = snapshot.rates else {
            for label in [
                &imp.tps_value,
                &imp.cache_value,
                &imp.tuples_value,
                &imp.deadlocks_value,
                &imp.temp_value,
            ] {
                label.set_text("—");
            }
            return;
        };

        imp.tps_value
            .set_text(&format_rate(rates.transactions_per_sec));
        imp.tps_graph
            .add_data_point(vec![vec![rates.transactions_per_sec as f32]]);

        imp.cache_value.set_text(&format_ratio(rates.cache_hit_ratio));
        if let Some(ratio) = rates.cache_hit_ratio {
            imp.cache_graph
                .add_data_point(vec![vec![(ratio * 100.0) as f32]]);
        }

        imp.tuples_value
            .set_text(&format_rate(rates.tuples_returned_per_sec));
        imp.tuples_graph
            .add_data_point(vec![vec![rates.tuples_returned_per_sec as f32]]);

        imp.deadlocks_value
            .set_text(&format_rate(rates.deadlocks_per_sec));
        imp.temp_value
            .set_text(&format_bytes(rates.temp_bytes_per_sec as i64));
    }
}

impl Default for McpgOverviewPage {
    fn default() -> Self {
        Self::new()
    }
}
```

Task 9 established that blueprint-compiler accepts `$GraphWidget` directly in a template — the
generated `.ui` carries `<object class="GraphWidget" id="...">` — provided `GraphWidget::ensure_type()`
runs in `class_init`. Use that route; no `Gtk.Box` fallback is needed.

- [ ] **Step 7: Write `resources/ui/sessions_page.blp`**

```blueprint
using Gtk 4.0;
using Adw 1;

template $McpgSessionsPage: Gtk.Box {
  orientation: vertical;

  Gtk.Box {
    spacing: 12;
    margin-start: 12;
    margin-end: 12;
    margin-top: 12;
    margin-bottom: 6;

    Gtk.SearchEntry filter_entry {
      hexpand: true;
      placeholder-text: _("Filter by user, database, application or query");
    }

    Gtk.ToggleButton hide_idle_toggle {
      label: _("Hide idle");
      active: true;
    }
  }

  Adw.Banner privilege_note {
    revealed: false;
    title: _("Query text for other users' sessions is hidden without pg_monitor.");
  }

  Gtk.ScrolledWindow {
    vexpand: true;

    child: Gtk.ColumnView column_view {
      show-column-separators: true;
      reorderable: false;
    };
  }
}
```

- [ ] **Step 8: Write `src/pages/sessions.rs`**

```rust
use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::prelude::{Cast, CastNone};
use gtk::{gio, glib};

use crate::collector::snapshot::Session;

glib::wrapper! {
    pub struct SessionObject(ObjectSubclass<session_object::SessionObject>);
}

mod session_object {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct SessionObject {
        pub session: RefCell<Option<Session>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SessionObject {
        const NAME: &'static str = "McpgSessionObject";
        type Type = super::SessionObject;
    }

    impl ObjectImpl for SessionObject {}
}

impl SessionObject {
    pub fn new(session: Session) -> Self {
        let object: Self = glib::Object::new();
        object.imp().session.replace(Some(session));
        object
    }

    pub fn session(&self) -> Session {
        self.imp()
            .session
            .borrow()
            .clone()
            .expect("SessionObject always holds a session")
    }
}

/// Column definitions: title, and how to render a session as text.
const COLUMNS: &[(&str, fn(&Session) -> String)] = &[
    ("PID", |s| s.pid.to_string()),
    ("User", |s| s.user_name.clone().unwrap_or_default()),
    ("Database", |s| s.database.clone().unwrap_or_default()),
    ("Application", |s| s.application_name.clone().unwrap_or_default()),
    ("Client", |s| s.client_addr.clone().unwrap_or_else(|| "local".to_string())),
    ("State", |s| s.state.clone().unwrap_or_default()),
    ("Wait", |s| match (&s.wait_event_type, &s.wait_event) {
        (Some(kind), Some(event)) => format!("{kind}: {event}"),
        _ => String::new(),
    }),
    ("Duration", |s| match s.query_duration_secs {
        Some(secs) if secs >= 1.0 => format!("{secs:.0}s"),
        Some(secs) => format!("{:.0}ms", secs * 1000.0),
        None => String::new(),
    }),
    ("Query", |s| {
        s.query
            .clone()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }),
];

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/sessions_page.ui")]
    pub struct McpgSessionsPage {
        #[template_child]
        pub filter_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub hide_idle_toggle: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub privilege_note: TemplateChild<adw::Banner>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,

        pub store: RefCell<Option<gio::ListStore>>,
        pub filter: RefCell<Option<gtk::CustomFilter>>,
        pub hide_idle: Cell<bool>,
        pub filter_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for McpgSessionsPage {
        const NAME: &'static str = "McpgSessionsPage";
        type Type = super::McpgSessionsPage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for McpgSessionsPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.hide_idle.set(true);
            self.obj().build_model();
            self.obj().build_columns();

            let page = self.obj().clone();
            self.filter_entry.connect_search_changed(move |entry| {
                page.imp()
                    .filter_text
                    .replace(entry.text().to_lowercase().to_string());
                page.refilter();
            });

            let page = self.obj().clone();
            self.hide_idle_toggle.connect_toggled(move |button| {
                page.imp().hide_idle.set(button.is_active());
                page.refilter();
            });
        }
    }

    impl WidgetImpl for McpgSessionsPage {}
    impl BoxImpl for McpgSessionsPage {}
}

glib::wrapper! {
    pub struct McpgSessionsPage(ObjectSubclass<imp::McpgSessionsPage>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl McpgSessionsPage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn build_model(&self) {
        let imp = self.imp();
        let store = gio::ListStore::new::<SessionObject>();

        let page = self.clone();
        let filter = gtk::CustomFilter::new(move |object| {
            let session = object
                .downcast_ref::<SessionObject>()
                .expect("the model only holds SessionObject")
                .session();
            page.matches(&session)
        });

        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        // Incremental filtering plus rapid items-changed is the combination
        // implicated in the upstream GTK sort/filter crash; keep it off.
        filtered.set_incremental(false);

        let sorted = gtk::SortListModel::new(Some(filtered), self.imp().column_view.sorter());
        sorted.set_incremental(false);

        imp.column_view
            .set_model(Some(&gtk::NoSelection::new(Some(sorted))));
        imp.store.replace(Some(store));
        imp.filter.replace(Some(filter));
    }

    fn build_columns(&self) {
        let imp = self.imp();
        for (title, render) in COLUMNS {
            let render = *render;
            let factory = gtk::SignalListItemFactory::new();

            factory.connect_setup(|_, item| {
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                item.downcast_ref::<gtk::ListItem>()
                    .expect("a ListItem")
                    .set_child(Some(&label));
            });

            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("a ListItem");
                let label = item
                    .child()
                    .and_downcast::<gtk::Label>()
                    .expect("the child set in setup");
                let session = item
                    .item()
                    .and_downcast::<SessionObject>()
                    .expect("a SessionObject")
                    .session();
                let text = render(&session);
                label.set_tooltip_text(Some(&text));
                label.set_text(&text);
            });

            let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
            column.set_resizable(true);
            column.set_expand(*title == "Query");
            imp.column_view.append_column(&column);
        }
    }

    fn matches(&self, session: &Session) -> bool {
        let imp = self.imp();

        if imp.hide_idle.get() && session.state.as_deref() == Some("idle") {
            return false;
        }

        let needle = imp.filter_text.borrow();
        if needle.is_empty() {
            return true;
        }

        let haystack = [
            session.user_name.as_deref(),
            session.database.as_deref(),
            session.application_name.as_deref(),
            session.query.as_deref(),
        ];
        haystack
            .iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(needle.as_str()))
    }

    fn refilter(&self) {
        if let Some(filter) = self.imp().filter.borrow().as_ref() {
            filter.changed(gtk::FilterChange::Different);
        }
    }

    pub fn set_hide_idle(&self, hide: bool) {
        self.imp().hide_idle_toggle.set_active(hide);
    }

    pub fn set_privilege_limited(&self, limited: bool) {
        self.imp().privilege_note.set_revealed(limited);
    }

    pub fn update(&self, sessions: &[Session]) {
        let imp = self.imp();
        let Some(store) = imp.store.borrow().clone() else {
            return;
        };
        // Replacing the contents in one splice keeps items-changed to a single
        // emission per sample rather than one per row.
        let objects: Vec<SessionObject> = sessions
            .iter()
            .cloned()
            .map(SessionObject::new)
            .collect();
        store.splice(0, store.n_items(), &objects);
    }
}

impl Default for McpgSessionsPage {
    fn default() -> Self {
        Self::new()
    }
}
```

Note the two `set_incremental(false)` calls. Mission Center's `gtk-issue.md` documents a GTK 4.22 crash in `gtk_sort_list_model_items_changed_cb` with a `FilterListModel → SortListModel` chain in incremental mode under rapid `items-changed`. This page has exactly that shape and updates every two seconds. Do not remove those calls.

`src/pages/mod.rs`:

```rust
pub mod format;
pub mod overview;
pub mod sessions;

pub use overview::McpgOverviewPage;
pub use sessions::McpgSessionsPage;
```

Add `pub mod pages;` to `src/lib.rs`.

- [ ] **Step 9: Register both Blueprints**

Add `'ui/overview_page.blp'` and `'ui/sessions_page.blp'` to `input:` in `resources/meson.build`, and both `.ui` files to `resources/mission-centre-pg.gresource.xml`.

- [ ] **Step 10: Build and run all tests**

```bash
ninja -C build 2>&1 | tail -20
cargo test --lib 2>&1 | tail -10
```

Expected: build succeeds; `test result: ok. 33 passed`.

- [ ] **Step 11: Commit**

```bash
cargo fmt
git add -A
git commit -m "feat: Overview and Sessions pages"
```

---

## Task 13: Wire the window together

The task that turns the parts into an application.

**Files:**
- Modify: `src/window.rs`, `resources/ui/window.blp`

**Interfaces:**
- Consumes: everything from Tasks 7, 9, 10, 11, 12
- Produces: a running application meeting all eight Phase 1 success criteria

- [ ] **Step 1: Rewrite `resources/ui/window.blp`**

```blueprint
using Gtk 4.0;
using Adw 1;

template $MissionCentrePgWindow: Adw.ApplicationWindow {
  title: _("Mission Centre PostgreSQL");
  default-width: 1100;
  default-height: 700;

  content: Adw.NavigationSplitView split_view {
    sidebar: Adw.NavigationPage {
      title: _("Servers");

      child: Adw.ToolbarView {
        [top]
        Adw.HeaderBar {
          [end]
          Gtk.Button add_server_button {
            icon-name: "list-add-symbolic";
            tooltip-text: _("Add Server");
          }
        }

        content: Gtk.ScrolledWindow {
          child: Gtk.ListBox server_list {
            selection-mode: single;
            styles ["navigation-sidebar"]
          };
        };
      };
    };

    content: Adw.NavigationPage {
      title: _("Monitor");

      child: Adw.ToolbarView {
        [top]
        Adw.HeaderBar {
          title-widget: Adw.ViewSwitcher {
            stack: view_stack;
            policy: wide;
          };
        }

        [top]
        Adw.Banner privilege_banner {
          revealed: false;
        }

        [top]
        Adw.Banner error_banner {
          revealed: false;
        }

        content: Adw.ViewStack view_stack {
          Adw.ViewStackPage {
            name: "overview";
            title: _("Overview");
            icon-name: "utilities-system-monitor-symbolic";
            child: $McpgOverviewPage overview_page {};
          }

          Adw.ViewStackPage {
            name: "sessions";
            title: _("Sessions");
            icon-name: "view-list-symbolic";
            child: $McpgSessionsPage sessions_page {};
          }
        };
      };
    };
  };
}
```

- [ ] **Step 2: Rewrite `src/window.rs`**

```rust
use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::prelude::{Cast, IsA};
use gtk::{gio, glib};

use mission_centre_pg::collector::worker::{spawn, CollectorEvent, CollectorHandle};
use mission_centre_pg::connection::params::ConnectionParams;
use mission_centre_pg::connection::{credentials, registry};
use mission_centre_pg::dialogs::McpgAddServerDialog;
use mission_centre_pg::pages::{McpgOverviewPage, McpgSessionsPage};
use mission_centre_pg::widgets::sidebar_row::{ConnectionState, McpgSidebarRow};

use mission_centre_pg::i18n::{i18n, i18n_f};

use crate::application::APP_ID;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/paulsnow/MissionCentrePg/ui/window.ui")]
    pub struct MissionCentrePgWindow {
        #[template_child]
        pub server_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub add_server_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub privilege_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub error_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub overview_page: TemplateChild<McpgOverviewPage>,
        #[template_child]
        pub sessions_page: TemplateChild<McpgSessionsPage>,

        pub settings: RefCell<Option<gio::Settings>>,
        pub servers: RefCell<Vec<ConnectionParams>>,
        pub collector: RefCell<Option<CollectorHandle>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MissionCentrePgWindow {
        const NAME: &'static str = "MissionCentrePgWindow";
        type Type = super::MissionCentrePgWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            McpgOverviewPage::ensure_type();
            McpgSessionsPage::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MissionCentrePgWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let settings = gio::Settings::new(APP_ID);
            self.overview_page
                .set_graph_points(settings.int("graph-points").max(1) as u32);
            self.sessions_page
                .set_hide_idle(settings.boolean("hide-idle-sessions"));
            self.settings.replace(Some(settings));

            let window = self.obj().clone();
            self.add_server_button
                .connect_clicked(move |_| window.present_add_server_dialog());

            let window = self.obj().clone();
            self.server_list.connect_row_selected(move |_, row| {
                if let Some(row) = row {
                    window.select_server(row.index());
                }
            });

            self.obj().reload_servers();
        }
    }

    impl WidgetImpl for MissionCentrePgWindow {}
    impl WindowImpl for MissionCentrePgWindow {}
    impl ApplicationWindowImpl for MissionCentrePgWindow {}
    impl AdwApplicationWindowImpl for MissionCentrePgWindow {}
}

glib::wrapper! {
    pub struct MissionCentrePgWindow(ObjectSubclass<imp::MissionCentrePgWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl MissionCentrePgWindow {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn settings(&self) -> gio::Settings {
        self.imp()
            .settings
            .borrow()
            .clone()
            .expect("settings are created in constructed()")
    }

    fn reload_servers(&self) {
        let imp = self.imp();
        let servers = registry::load(&self.settings());

        while let Some(child) = imp.server_list.first_child() {
            imp.server_list.remove(&child);
        }

        for server in &servers {
            let row = McpgSidebarRow::new(&server.label);
            row.set_subheading(&format!("{}:{}", server.host, server.port));
            row.set_state(ConnectionState::Disconnected);
            imp.server_list.append(&row);
        }

        imp.servers.replace(servers);
    }

    fn present_add_server_dialog(&self) {
        let dialog = McpgAddServerDialog::new();
        let window = self.clone();
        dialog.connect_added(move |params| {
            let mut servers = registry::load(&window.settings());
            servers.push(params.clone());
            if let Err(e) = registry::save(&window.settings(), &servers) {
                gtk::glib::g_warning!("mission-centre-pg", "could not save the server list: {e}");
            }
            window.reload_servers();
        });
        dialog.present(Some(self));
    }

    fn select_server(&self, index: i32) {
        let imp = self.imp();

        if let Some(handle) = imp.collector.take() {
            handle.stop();
        }

        let Some(params) = imp.servers.borrow().get(index as usize).cloned() else {
            return;
        };

        let password = credentials::fetch_password(&params.id)
            .ok()
            .flatten()
            .unwrap_or_default();

        let interval = std::time::Duration::from_millis(
            self.settings().int("sample-interval-ms").max(500) as u64,
        );

        let handle = spawn(params, password, interval);
        let events = handle.events.clone();
        imp.collector.replace(Some(handle));

        let window = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                window.handle_event(event);
            }
        });
    }

    fn selected_row(&self) -> Option<McpgSidebarRow> {
        self.imp()
            .server_list
            .selected_row()
            .and_then(|row| row.downcast::<McpgSidebarRow>().ok())
    }

    fn handle_event(&self, event: CollectorEvent) {
        let imp = self.imp();

        match event {
            CollectorEvent::Connecting => {
                imp.error_banner.set_revealed(false);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connecting);
                }
            }
            CollectorEvent::Connected(info) => {
                imp.error_banner.set_revealed(false);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connected);
                    row.set_subheading(&i18n_f("PostgreSQL {}", &[&info.version_display]));
                }

                let limited = info.privilege.hides_other_sessions();
                imp.privilege_banner.set_revealed(limited);
                imp.privilege_banner.set_title(&i18n(
                    "Connected without pg_monitor — query text and statistics for other users' sessions are hidden.",
                ));
                imp.sessions_page.set_privilege_limited(limited);

                if info.is_below_floor() {
                    imp.error_banner.set_revealed(true);
                    imp.error_banner.set_title(&i18n_f(
                        "PostgreSQL {} is older than the supported floor of 14. Some statistics may be missing.",
                        &[&info.version_display],
                    ));
                }
            }
            CollectorEvent::Sample(snapshot) => {
                imp.error_banner.set_revealed(false);
                imp.overview_page.update(&snapshot);
                imp.sessions_page.update(&snapshot.sessions);
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Connected);
                    row.push_value(snapshot.session_counts.total() as f64);
                }
            }
            CollectorEvent::Error(error) => {
                imp.error_banner.set_revealed(true);
                imp.error_banner.set_title(&error.to_string());
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Failed);
                }
            }
            CollectorEvent::Disconnected => {
                if let Some(row) = self.selected_row() {
                    row.set_state(ConnectionState::Disconnected);
                }
            }
        }
    }
}
```

- [ ] **Step 3: Build**

```bash
ninja -C build 2>&1 | tail -30
```

Expected: build succeeds.

- [ ] **Step 4: Run against the local server and verify each success criterion**

```bash
export MCPG_RESOURCE_DIR="$PWD/build/resources"
export GSETTINGS_SCHEMA_DIR="$PWD/build/data"
glib-compile-schemas --strict data && mv data/gschemas.compiled build/data/
./build/src/mission-centre-pg
```

Check, in order:

1. The window opens with an empty server list.
2. **Add Server** accepts `localhost` / `5432` / `postgres` / your username, and the row appears.
3. Selecting the row connects; the sidebar shows ● and "PostgreSQL 18.4".
4. The Overview page shows `— ` for every rate on the first sample, then real numbers from the second.
5. Connections reads `n / 100` and the graph fills over time.
6. The Sessions page lists backends; idle ones are hidden until the toggle is released; the filter narrows rows.
7. `sudo systemctl stop postgresql` — the error banner appears and the row turns ⚠. `sudo systemctl start postgresql` — it reconnects within 30 seconds and the graphs retain their earlier history.
8. Restart the application: the server is still listed and connects without re-entering the password.

- [ ] **Step 5: Verify the password never reaches disk in clear text**

```bash
gsettings --schemadir build/data get io.github.paulsnow.MissionCentrePg servers
```

Expected: JSON with no password field. Confirm the secret is in the keyring instead:

```bash
secret-tool search service mission-centre-pg 2>&1 | head -5
```

- [ ] **Step 6: Final check and commit**

```bash
cargo fmt --check
cargo test --lib
export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
cargo test --test portability
git add -A
git commit -m "feat: wire the window to the collector, completing Phase 1"
```

Expected: `cargo fmt --check` silent; all unit tests pass; all three portability tests pass.

---

## Self-Review Notes

Checked against the spec:

- §2.2 Phase 1 scope — Tasks 1–13 cover subsystems 1, 2 and 4 plus the Overview and Sessions pages. History (§2.2 Phase 3) and actions (Phase 4) are absent by design.
- §3 architecture — Task 7 implements the collector thread and channel; Task 13 the `spawn_future_local` consumer.
- §4.2 state ownership — ring buffers live in the pages (Task 12), not the collector.
- §4.4 delta-based rates — Task 2 Step 1 tests it explicitly, including the cumulative-versus-interval trap.
- §5 version floor — Task 5 defines it; Task 6 proves the SQL on 14 and 18; Task 13 gates by banner, never by refusal.
- §6 privilege detection — Task 5 classifies; Task 13 shows the window-level banner.
- §7.1 persistence — Task 4 (keyring), Task 10 (GSettings), verified in Task 13 Step 5.
- §8 error handling — timeout, backoff and three-strike disconnect in Task 7; banner and recovery verified in Task 13 Step 4.7.
- §9 vendoring — Task 8, with provenance comments and the two permitted edits.
- §10 testing — unit tests throughout, containers in Task 6.
- §11 success criteria — all eight verified in Task 13 Steps 4–6.

Two known soft spots, flagged rather than hidden:

1. **`GraphWidget`'s exact API is unverified.** The plan calls `set_data_points(&[f64], f64)`, which is a plausible shape but not confirmed against the upstream source. Task 12 Step 6 instructs the implementer to read `src/widgets/graph_widget.rs` and adapt, centralising the adaptation in one helper. Expect to adjust these call sites.
2. **Blueprint's `$GraphWidget` syntax for a custom type** requires the type to be registered before the template is instantiated. `ensure_type()` calls are placed in `class_init` for this reason. If Blueprint rejects the syntax, declare the graph as a plain `Gtk.Box` placeholder and insert the widget from Rust in `constructed()`.
