project(
  'mission-centre-pg',
  version: '0.0.0',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0.0',
)

gnome = import('gnome')

app_id = 'io.github.paulsnow.MissionCentrePg'

dependency('gtk4', version: '>= 4.12')
dependency('libadwaita-1', version: '>= 1.5')

cargo = find_program('cargo', required: true)
blueprint_compiler = find_program('blueprint-compiler', required: true)

subdir('data')
subdir('resources')
subdir('src')

gnome.post_install(glib_compile_schemas: true)
