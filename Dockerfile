# deskhand-server as a container: one static binary on an empty image.
# Mount a volume at /data (database and cached agent builds). Railway, Fly and the like set
# PORT; elsewhere it listens on 8080.
FROM oven/bun:1.4.2-alpine AS web
WORKDIR /src/apps/web
COPY apps/web/package.json apps/web/bun.lock ./
RUN bun install --frozen-lockfile
COPY apps/web ./
RUN bun run build

FROM rust:1.98.1-alpine AS server
# A C compiler for the bundled SQLite; musl links it all statically.
RUN apk add --no-cache musl-dev gcc
WORKDIR /src
COPY . .
COPY --from=web /src/apps/web/dist apps/web/dist
# An explicit --target keeps .cargo/config.toml's static flags off the host proc macros.
RUN t=$(uname -m)-unknown-linux-musl \
 && cargo build --release --locked --target "$t" -p deskhand-server \
 && cp "target/$t/release/deskhand-server" /deskhand-server \
 && mkdir -p /out/data /out/tmp && chmod 1777 /out/tmp

FROM scratch
COPY --from=server /out/ /
COPY --from=server /deskhand-server /deskhand-server
# Behind the platform's TLS proxy; the agents dir defaults to /data/agents.
ENV DESKHAND_DB=/data/deskhand.db DESKHAND_TRUST_PROXY=1
EXPOSE 8080
ENTRYPOINT ["/deskhand-server"]
