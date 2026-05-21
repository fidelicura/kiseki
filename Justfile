set quiet

[private]
default:
	{{just_executable()}} --list --unsorted --no-aliases

bench profile="release":
    cargo bench \
        --profile {{profile}} \
        --bench trace \
        -- \
        pipeline_to_vec

build profile="dev":
    cargo build \
        --profile {{profile}}

clean:
    cargo clean
    rm -f tarpaulin-report.html

coverage profile="dev":
    cargo tarpaulin \
        --profile {{profile}} \
        --skip-clean \
        --out Html

doc:
    cargo doc

fmt:
    cargo fmt --all -- --check

lint:
    cargo check --all-targets
    cargo clippy --all-targets

test profile="dev":
    cargo test \
        --profile {{profile}} \
        --all-targets
    cargo test \
        --profile {{profile}} \
        --doc
