FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Install Tauri dependencies
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install --no-install-recommends \
    libwebkit2gtk-4.1-dev \
    build-essential \
    curl \
    wget \
    file \
    libssl-dev \
    libgtk-3-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Bun
ENV BUN_INSTALL=/usr/local
RUN curl -fsSL https://bun.sh/install | bash
