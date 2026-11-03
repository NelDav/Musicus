# syntax=docker/dockerfile:1.7
#
# Musicus in one image: the server binary plus the built web client.
#
#   docker build -t musicus .
#   docker compose up -d                  # see compose.yaml
#
# For a Raspberry Pi (arm64) build on the Pi itself, or cross-build with
#   docker buildx build --platform linux/arm64 -t musicus .

ARG RUST_IMAGE=rust:1.97-bookworm
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot

# --- Dioxus CLI: its own stage, so the slow install is cached --------------
FROM ${RUST_IMAGE} AS dx
ARG DX_VERSION=0.7.10
RUN cargo install --locked dioxus-cli --version ${DX_VERSION} --root /opt/dx

# --- Build the web client and the server ----------------------------------
FROM ${RUST_IMAGE} AS build
RUN rustup target add wasm32-unknown-unknown
COPY --from=dx /opt/dx/bin/dx /usr/local/bin/dx
WORKDIR /src
COPY . .
# pdf.js is pinned and checksum-verified by the script.
RUN ./scripts/fetch-pdfjs.sh
# `dx` downloads wasm-bindgen, wasm-opt and esbuild on first use.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    dx bundle --platform web --package musicus-web --release \
    && cargo build --release --locked -p musicus-server \
    && mkdir -p /out/data \
    && cp target/release/musicus-server /out/ \
    && cp -r target/dx/musicus-web/release/web/public /out/web

# --- Runtime: no shell, no package manager, not root -----------------------
FROM ${RUNTIME_IMAGE}
COPY --from=build /out/musicus-server /usr/local/bin/musicus-server
COPY --from=build /out/web /app/web
# An empty data directory owned by the runtime user, so a fresh volume
# inherits that ownership.
COPY --from=build --chown=65532:65532 /out/data /data
ENV MUSICUS_DATA_DIR=/data \
    MUSICUS_WEB_DIR=/app/web \
    MUSICUS_LISTEN=0.0.0.0:8080
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/usr/local/bin/musicus-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/musicus-server"]
CMD ["serve"]
