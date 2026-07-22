blueprints = custom_target(
  'blueprints',
  input: files('ui/window.blp', 'ui/sidebar_row.blp', 'ui/add_server_dialog.blp'),
  output: '.',
  command: [blueprint_compiler, 'batch-compile', '@OUTPUT@', '@CURRENT_SOURCE_DIR@', '@INPUT@'],
)

resources_gresource = gnome.compile_resources(
  'mission-centre-pg',
  'mission-centre-pg.gresource.xml',
  dependencies: blueprints,
  source_dir: meson.current_build_dir(),
  gresource_bundle: true,
  build_by_default: true,
)
