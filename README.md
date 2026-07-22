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
