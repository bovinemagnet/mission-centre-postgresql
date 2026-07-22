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
