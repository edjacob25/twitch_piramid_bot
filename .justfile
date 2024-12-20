build_docker:
    podman build -t bot:latest .

run_docker:
    podman run -it --rm -v=./data:/data local:bot