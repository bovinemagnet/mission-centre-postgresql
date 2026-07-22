# Development

## Running the integration tests

The portability tests start real PostgreSQL containers. This machine has podman
rather than docker, so point the Docker API client at podman's socket:

    systemctl --user enable --now podman.socket
    export DOCKER_HOST="unix:///run/user/$(id -u)/podman/podman.sock"
    cargo test --test portability

The tests pull `docker.io/library/postgres:14` and `:18` on first run.
