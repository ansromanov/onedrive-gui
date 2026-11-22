default:
    @just --list

# build the daemon and the app
build:
    cargo build --release --manifest-path daemon/Cargo.toml
    cmake -S app -B app/build -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build app/build

# format check, lint
lint:
    cargo fmt --manifest-path daemon/Cargo.toml -- --check
    cargo clippy --manifest-path daemon/Cargo.toml --all-targets -- -D warnings
    shellcheck install.sh tests/run.sh daemon/tests/*.sh dolphin/tests/*.sh

# unit tests + end-to-end on a private D-Bus
test:
    tests/run.sh

# install (Arch: pacman package; elsewhere ~/.local)
install:
    ./install.sh
