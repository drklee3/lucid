# Image for ExecutionBackend::Sandboxed dispatches (src/harness/mod.rs).
# Built as `lucid-sandbox:latest` — see docs/wiki/architecture/sandboxed-execution.md
# for the mount/network policy this image runs under.
#
# Bundles both harness CLIs lucid can dispatch to (HarnessKind::ClaudeCode,
# HarnessKind::Codex) in one image rather than per-kind images, since a single
# dispatch never needs to select the image at runtime — `profile.cmd` picks
# the binary, the image just needs to have it.
FROM node:22-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @anthropic-ai/claude-code @openai/codex \
    && npm cache clean --force

# Dispatch runs with an arbitrary host-matching -u uid:gid (no /etc/passwd
# entry for it), so tools that assume $HOME exists need one set explicitly.
ENV HOME=/tmp

WORKDIR /workspace
