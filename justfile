# ShopPhoto task runner. Install: `brew install just` (or `cargo install just`).
default: run

run:
    cargo run -p sp-app

test:
    cargo test --workspace

lint:
    cargo fmt --all --check
    cargo clippy --all-targets --workspace -- -D warnings

fmt:
    cargo fmt --all

bench:
    cargo bench --workspace

bundle-macos:
    ./scripts/bundle-macos.sh

package-linux:
    ./scripts/package-linux.sh
