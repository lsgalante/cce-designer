.PHONY: build install run clean

build:
	cargo build --release

install: build
	mkdir -p ~/.local/bin
	install -m 755 ../target/release/cce-design-interface ~/.local/bin/cce-design-interface

run:
	cargo run

clean:
	cargo clean
