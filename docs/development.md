# Development

## Running the integration tests

The portability tests start real PostgreSQL containers. This machine has podman
rather than docker, so point the Docker API client at podman's socket:

    systemctl --user enable --now podman.socket
    export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
    cargo test --test portability

The tests pull `docker.io/library/postgres:14` and `:18` on first run.

## Inspecting the running user interface

`tools/uicheck.py` reads the running application over the accessibility bus.
GTK4 publishes the widget tree there automatically, so this needs no code in
the application and no special build — only `python-gobject` and
`at-spi2-core`, which GTK already depends on.

    ./build/src/mission-centre-pg &        # with the usual MCPG_* variables
    tools/uicheck.py digest

`digest` prints what is on screen: the selected page, every button with
whether it is sensitive, the table rows with their indentation, and any
privilege or truncation notice.

    tools/uicheck.py criteria --dsn "host=127.0.0.1 port=55432 user=postgres"

`criteria` evaluates the success criteria that can be judged without
touching anything. Criteria whose state is not on screen are reported as
`SKIP` rather than passed, so a run against the wrong page cannot be
mistaken for a clean sweep. One criterion — that the lock inventory query
does not run while its view is hidden — is checked against
`pg_stat_activity` instead, since the absence of a query is not something
the interface can show.

The tool never clicks, types or activates. Putting the application into the
state to be checked stays deliberate, and the criteria that need a
selection or a click still need a person.
