# Development

## Running the integration tests

The portability tests start real PostgreSQL containers. This machine has podman
rather than docker, so point the Docker API client at podman's socket:

    systemctl --user enable --now podman.socket
    export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
    cargo test --test portability

The tests pull `docker.io/library/postgres:14` and `:18` on first run.

## Servers for the by-hand walkthroughs

The integration tests need no setup: they start and remove their own
containers. The success criteria in the phase plans do, because contention,
a streaming standby and an unprivileged role are states a person has to look
at. `tools/fixtures.sh` builds them:

    tools/fixtures.sh statements   # pg_stat_statements and a captured join   :55436
    tools/fixtures.sh locks        # a three-deep lock chain, held open       :55432
    tools/fixtures.sh roles        # the postgres/watcher/app trio            :55437
    tools/fixtures.sh standby      # a primary streaming to a standby  :55434 :55435
    tools/fixtures.sh all
    tools/fixtures.sh down

Every server uses `postgres`/`postgres`. `IMAGE_TAG=15` selects another
version, which is how the version-gated messages get checked; `HOLD_SECONDS`
changes how long the lock chain is held.

The script exists mainly to record the things that are easy to get wrong:
PostgreSQL 18's image moved `PGDATA`, `pg_basebackup` needs a `replication`
line in `pg_hba.conf` that `POSTGRES_HOST_AUTH_METHOD=trust` does not
provide, `psql -U app` connects to a database named after the role unless
told otherwise, and PostgreSQL 15 and later revoke `CREATE` on schema
`public` from `PUBLIC`.

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
