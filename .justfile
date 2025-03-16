default:
  @just --list

build_docker:
    podman build -t bot:latest .

run_docker:
    podman run -it --rm -v=./data:/data bot:latest

pretty_html:
    npx prettier templates/* --write

format: pretty_html
    cargo fmt
