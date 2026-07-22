install_data(
  '@0@.gschema.xml'.format(app_id),
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

gnome.compile_schemas(build_by_default: true, depend_files: files('@0@.gschema.xml'.format(app_id)))
