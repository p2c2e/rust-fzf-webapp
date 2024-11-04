# Use Rust official image as builder
FROM rust:1.75-slim-bullseye as builder

# Create a new empty shell project
WORKDIR /usr/src/fuzzy-search
RUN cargo new --bin fuzzy-search-webapp

# Copy over manifests and source code
COPY ./Cargo.toml ./fuzzy-search-webapp/Cargo.toml
COPY ./src ./fuzzy-search-webapp/src

# Build dependencies - this is done in a separate step to cache dependencies
WORKDIR /usr/src/fuzzy-search/fuzzy-search-webapp
RUN cargo build --release

# Final stage
FROM debian:bullseye-slim

# Install fzf and find (part of findutils)
RUN apt-get update && \
    apt-get install -y fzf findutils && \
    rm -rf /var/lib/apt/lists/*

# Create data directory
RUN mkdir /data

# Copy the binary from builder
COPY --from=builder /usr/src/fuzzy-search/fuzzy-search-webapp/target/release/fuzzy-search-webapp /usr/local/bin/fuzzy-search-webapp

# Expose the port the app runs on
EXPOSE 3000

# Command to run the application
CMD ["/usr/local/bin/fuzzy-search-webapp", "/data"]
