# Multi-stage build for Arnis
FROM rust:latest as builder

WORKDIR /build

# Install system dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    libwebkit2gtk-4.1-dev \
    curl \
    wget \
    file \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy project files
COPY . .

# Build the project
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy built binary from builder
COPY --from=builder /build/target/release/arnis /app/arnis

ENTRYPOINT ["/app/arnis"]
