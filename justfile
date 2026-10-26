# Show available recipes
[private]
default:
    @just --list

# Format Rust sources
fmt:
    cargo fmt --all

# Static checks CI runs
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo run --locked -q -p sych-style -- crates

# Insert the blank lines the house style asks for, then format
style-fix:
    cargo run --locked -q -p sych-style -- --fix crates
    cargo fmt --all

# Run unit, integration and documentation tests
test:
    cargo test --workspace --all-features --locked

# Everything CI runs
check: lint test

# Build the application
build:
    cargo build --workspace --locked

# Run the CLI (for example: just run --version)
run *args="--help":
    cargo run --locked -p sych-app --bin sych -- {{args}}

# Regenerate JSON Schemas from the Rust DTOs
schemas:
    SYCH_BLESS=1 cargo test --locked -p sych-core --test contracts generated_
