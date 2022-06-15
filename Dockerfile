FROM lukemathwalker/cargo-chef as planner
WORKDIR /keyboard_shared
COPY keyboard_shared/. .
WORKDIR /app
COPY keyboard_control/. .
RUN cargo chef prepare  --recipe-path recipe.json

FROM lukemathwalker/cargo-chef as cacher
WORKDIR /keyboard_shared
COPY keyboard_shared/. .
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

FROM rust as builder
WORKDIR /keyboard_shared
COPY keyboard_shared/. .
WORKDIR /app
COPY keyboard_control/. .
# Copy over the cached dependencies
COPY --from=cacher /app/target target
COPY --from=cacher $CARGO_HOME $CARGO_HOME
RUN cargo build --release --bin keyboard-control

FROM debian as runtime
WORKDIR app
COPY --from=builder /app/target/release/keyboard-control /usr/local/bin
ENTRYPOINT ["/usr/local/bin/keyboard-control"]
