FROM rust:1 AS builder

WORKDIR /src

RUN apt-get update \
    && apt-get install -y protobuf-compiler

# Dummy source to cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target "$(uname -m)-unknown-linux-gnu" \
    && rm -rf src

# Actual source code
COPY . .
RUN RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target "$(uname -m)-unknown-linux-gnu"
RUN cp "/src/target/$(uname -m)-unknown-linux-gnu/release/worker" /usr/local/bin/worker
RUN cp "/src/target/$(uname -m)-unknown-linux-gnu/release/server" /usr/local/bin/server

FROM nixos/nix:latest

RUN echo "experimental-features = flakes nix-command" >> /etc/nix/nix.conf \
    && echo "filter-syscalls = false" >> /etc/nix/nix.conf

RUN nix-env --install --quiet --attr \
    nixpkgs.docker \
    nixpkgs.git \
    nixpkgs.nixpacks \
    nixpkgs.kubernetes-helm \
    nixpkgs.oras

COPY --from=builder /usr/local/bin/worker /usr/local/bin/worker
COPY --from=builder /usr/local/bin/server /usr/local/bin/server

CMD [ "/usr/local/bin/worker" ]
