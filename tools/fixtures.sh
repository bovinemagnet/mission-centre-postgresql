#!/usr/bin/env bash
#
# Copyright 2026 Paul Snow
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.
#
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Servers for the by-hand walkthroughs in the phase plans.
#
# The automated tests need none of this: tests/portability.rs uses
# testcontainers, which starts and removes a container per test. What cannot be
# automated away is the state a person has to look at — real lock contention, a
# streaming standby, a role that cannot see very much — and rebuilding that by
# hand each time means meeting the same traps again. They are recorded here.

set -euo pipefail

IMAGE_TAG="${IMAGE_TAG:-18}"
IMAGE="docker.io/library/postgres:${IMAGE_TAG}"

# PostgreSQL 18's image moved PGDATA. Anything writing to the old path silently
# creates a directory the server never reads.
pgdata() {
    if [[ "${IMAGE_TAG}" == "18" ]]; then
        echo "/var/lib/postgresql/18/docker"
    else
        echo "/var/lib/postgresql/data"
    fi
}

log() { printf '  %s\n' "$*"; }

wait_ready() {
    podman exec "$1" bash -c 'until pg_isready -U postgres -q; do sleep 1; done'
}

psql_() {
    local container="$1"; shift
    podman exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 "$@"
}

# Anything after the port is the container's own command, and podman wants it
# after the image name — putting it before makes podman read the first word as
# the image and fail on the short name.
start() {
    local name="$1" port="$2"; shift 2
    podman rm -f "$name" >/dev/null 2>&1 || true
    podman run --rm -d --name "$name" \
        -e POSTGRES_PASSWORD=postgres \
        -e POSTGRES_HOST_AUTH_METHOD=trust \
        -p "${port}:5432" "$IMAGE" "$@" >/dev/null
    wait_ready "$name"
}

# ---------------------------------------------------------------------------
# statements — the Plan page (issue #5) and the Queries page
# ---------------------------------------------------------------------------
# pg_stat_statements has to be preloaded before the server starts; CREATE
# EXTENSION alone is not enough. The join is run once so a normalised statement
# with a $1 placeholder is waiting to be explained.
fixture_statements() {
    local name=mcpg-statements port=55436
    log "starting $name on 127.0.0.1:$port"
    start "$name" "$port" postgres -c shared_preload_libraries=pg_stat_statements
    psql_ "$name" -qc "CREATE EXTENSION pg_stat_statements"
    psql_ "$name" -qc "CREATE TABLE customers (id int PRIMARY KEY, name text)"
    psql_ "$name" -qc "CREATE TABLE orders (id bigserial PRIMARY KEY, customer int, note text)"
    psql_ "$name" -qc "INSERT INTO customers SELECT g, 'c'||g FROM generate_series(1,1000) g"
    psql_ "$name" -qc "INSERT INTO orders (customer, note) SELECT (random()*999)::int+1, 'n' FROM generate_series(1,50000)"
    psql_ "$name" -qc "ANALYZE"
    psql_ "$name" -qc "SELECT o.id, c.name FROM orders o JOIN customers c ON c.id = o.customer WHERE o.id = 42" >/dev/null
    log "a join is captured and ready to explain — connect as postgres/postgres"
}

# ---------------------------------------------------------------------------
# locks — the Locks page
# ---------------------------------------------------------------------------
# Three sessions in one chain. Each holds its transaction open with pg_sleep,
# because a session that ends releases its lock; multiple -c arguments run in
# one session, so the BEGIN persists across them.
fixture_locks() {
    local name=mcpg-locks port=55432 hold="${HOLD_SECONDS:-7200}"
    log "starting $name on 127.0.0.1:$port"
    start "$name" "$port"
    psql_ "$name" -qc "CREATE TABLE conflict (id int PRIMARY KEY, note text)"
    psql_ "$name" -qc "INSERT INTO conflict VALUES (1, 'a'), (2, 'b')"
    psql_ "$name" -qc "CREATE ROLE plain LOGIN PASSWORD 'plain'"

    for label in root second third; do
        podman exec -i "$name" psql -U postgres \
            -c "BEGIN" \
            -c "UPDATE conflict SET note = '$label' WHERE id = 1" \
            -c "SELECT pg_sleep($hold)" >/dev/null 2>&1 &
        sleep 1
    done

    sleep 2
    log "chain held for ${hold}s:"
    psql_ "$name" -c "SELECT pid, state, pg_blocking_pids(pid) AS blocked_by
                      FROM pg_stat_activity
                      WHERE datname='postgres' AND backend_type='client backend'
                        AND pid <> pg_backend_pid() ORDER BY pid"
    log "role 'plain' exists for the without-pg_monitor case"
}

# ---------------------------------------------------------------------------
# roles — the Phase 4 privilege model
# ---------------------------------------------------------------------------
# Two traps here. psql -U app connects to a database named after the role
# unless told otherwise, and PostgreSQL 15 and later revoke CREATE on schema
# public from PUBLIC, so app cannot create its own table without the grant.
fixture_roles() {
    local name=mcpg-roles port=55437
    log "starting $name on 127.0.0.1:$port"
    start "$name" "$port" postgres -c shared_preload_libraries=pg_stat_statements
    psql_ "$name" -qc "CREATE EXTENSION pg_stat_statements"
    psql_ "$name" -qc "CREATE ROLE app LOGIN PASSWORD 'app'"
    psql_ "$name" -qc "CREATE ROLE watcher LOGIN PASSWORD 'watcher' IN ROLE pg_monitor"
    psql_ "$name" -qc "GRANT CREATE ON SCHEMA public TO app"
    podman exec -i -e PGPASSWORD=app "$name" psql -U app -h 127.0.0.1 -d postgres -v ON_ERROR_STOP=1 \
        -qc "CREATE TABLE app_orders (id bigserial PRIMARY KEY, note text)"
    psql_ "$name" -qc "CREATE TABLE ops_audit (id bigserial PRIMARY KEY, note text)"
    psql_ "$name" -qc "ALTER TABLE app_orders SET (autovacuum_enabled = false)"
    log "roles: postgres (superuser), watcher (pg_monitor, cannot signal), app (owns app_orders)"
    log "ops_audit is owned by postgres, so app can see it but not maintain it"
}

# ---------------------------------------------------------------------------
# standby — the Replication page
# ---------------------------------------------------------------------------
# POSTGRES_HOST_AUTH_METHOD=trust does not cover replication connections, which
# match a separate pg_hba entry; without it pg_basebackup is refused.
fixture_standby() {
    local primary=mcpg-primary standby=mcpg-standby
    local net=mcpg-net data; data="$(pgdata)"

    podman network rm "$net" >/dev/null 2>&1 || true
    podman network create "$net" >/dev/null

    log "starting $primary on 127.0.0.1:55434"
    podman rm -f "$primary" >/dev/null 2>&1 || true
    podman run --rm -d --name "$primary" --network "$net" \
        -e POSTGRES_PASSWORD=postgres -e POSTGRES_HOST_AUTH_METHOD=trust \
        -p 55434:5432 "$IMAGE" >/dev/null
    wait_ready "$primary"

    podman exec -i "$primary" bash -c "echo 'host replication all all trust' >> $data/pg_hba.conf"
    psql_ "$primary" -qtc "SELECT pg_reload_conf()" >/dev/null
    psql_ "$primary" -qtc "SELECT pg_create_physical_replication_slot('standby_slot')" >/dev/null

    log "building $standby from a base backup, on 127.0.0.1:55435"
    podman rm -f "$standby" >/dev/null 2>&1 || true
    podman run --rm -d --name "$standby" --network "$net" -p 55435:5432 \
        --entrypoint bash "$IMAGE" -c "
            rm -rf $data; mkdir -p $data; chown postgres:postgres $data
            su postgres -c 'pg_basebackup -h $primary -U postgres -D $data -Fp -Xs -R -S standby_slot'
            chmod 700 $data
            su postgres -c 'postgres -D $data'" >/dev/null
    wait_ready "$standby"

    sleep 2
    psql_ "$primary" -c "SELECT application_name, state, sync_state, replay_lag FROM pg_stat_replication"
    log "pause replay to make the lag grow:"
    log "  podman exec -i $standby psql -U postgres -c 'SELECT pg_wal_replay_pause()'"
    log "stop the standby to see its slot go inactive:"
    log "  podman stop $standby"
}

fixture_down() {
    podman rm -f mcpg-statements mcpg-locks mcpg-roles mcpg-primary mcpg-standby >/dev/null 2>&1 || true
    podman network rm mcpg-net >/dev/null 2>&1 || true
    log "all fixtures removed"
}

usage() {
    cat <<'USAGE'
Usage: tools/fixtures.sh <fixture>...

  statements   pg_stat_statements with a captured join, for the Plan and
               Queries pages                                   127.0.0.1:55436
  locks        a three-deep lock chain held open, plus a role
               without pg_monitor                              127.0.0.1:55432
  roles        the postgres/watcher/app trio and their tables,
               for the privilege model                         127.0.0.1:55437
  standby      a primary streaming to a standby         55434 and 55435
  all          every fixture above
  down         remove all of them

Every server uses postgres/postgres. IMAGE_TAG=15 selects another version;
HOLD_SECONDS changes how long the lock chain is held.

The automated tests need none of this — they start their own containers.
USAGE
}

main() {
    [[ $# -gt 0 ]] || { usage; exit 2; }
    command -v podman >/dev/null || { echo "podman is not on PATH" >&2; exit 1; }

    for fixture in "$@"; do
        case "$fixture" in
            statements) fixture_statements ;;
            locks)      fixture_locks ;;
            roles)      fixture_roles ;;
            standby)    fixture_standby ;;
            all)        fixture_statements; fixture_locks; fixture_roles; fixture_standby ;;
            down)       fixture_down ;;
            *)          usage; exit 2 ;;
        esac
    done
}

main "$@"
