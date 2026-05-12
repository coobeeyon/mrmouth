# Stage 1: Build litebrite (lb) and trapperkeeper (trk) — static musl binaries
FROM rust:slim AS tools-builder
RUN apt-get update && apt-get install -y musl-tools && rm -rf /var/lib/apt/lists/*
RUN MUSL_TARGET="$(uname -m)-unknown-linux-musl" && \
    rustup target add "$MUSL_TARGET" && \
    cargo install --git https://github.com/coobeeyon/litebrite.git --target "$MUSL_TARGET" && \
    cargo install --git https://github.com/coobeeyon/trapperkeeper.git --target "$MUSL_TARGET"

# Stage 2: Runtime image
FROM node:22

# Layer 1: System deps (changes ~never)
RUN apt-get update && apt-get install -y --no-install-recommends \
    unzip openssh-client sudo curl git-lfs ripgrep \
  && git lfs install \
  && rm -rf /var/lib/apt/lists/*

# Layer 2: GitHub CLI (changes occasionally)
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg && \
    chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      | tee /etc/apt/sources.list.d/github-cli.list > /dev/null && \
    apt-get update && apt-get install -y --no-install-recommends gh && \
    rm -rf /var/lib/apt/lists/*

# Layer 3: GitHub SSH known host (changes ~never)
RUN mkdir -p /root/.ssh && \
    ssh-keyscan github.com >> /root/.ssh/known_hosts

# Layer 4: Copy tool binaries from builder
COPY --from=tools-builder /usr/local/cargo /usr/local/cargo
COPY --from=tools-builder /usr/local/rustup /usr/local/rustup
COPY --from=tools-builder /usr/local/cargo/bin/lb /usr/local/bin/lb
COPY --from=tools-builder /usr/local/cargo/bin/trk /usr/local/bin/trk
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH="/usr/local/cargo/bin:${PATH}"
RUN rustup component add rustfmt && \
    ln -sf /usr/local/cargo/bin/cargo /usr/local/bin/cargo && \
    ln -sf /usr/local/cargo/bin/rustc /usr/local/bin/rustc && \
    ln -sf /usr/local/cargo/bin/rustfmt /usr/local/bin/rustfmt

# Layer 5: Agent CLIs (changes occasionally)
RUN npm install -g @anthropic-ai/claude-code @openai/codex

# Layer 6: Non-root user matching host UID (for SSH agent socket access)
ARG HOST_UID=1000
ARG HOST_GID=1000
RUN userdel -r node 2>/dev/null || true && \
    groupadd -g ${HOST_GID} runner 2>/dev/null || true && \
    useradd -m -s /bin/bash -u ${HOST_UID} -g ${HOST_GID} runner && \
    echo "runner ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/runner && \
    cp -r /root/.ssh /home/runner/.ssh && \
    chown -R runner:runner /home/runner/.ssh
USER runner
ENV HOME=/home/runner
RUN git config --global user.name "agent-runner" && \
    git config --global user.email "agent-runner@local"

ENTRYPOINT ["bash"]
