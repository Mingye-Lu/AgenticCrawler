FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://github.com/Mingye-Lu/AgenticCrawler/releases/latest/download/acrawl-linux-x64 \
    -o /usr/local/bin/acrawl && chmod +x /usr/local/bin/acrawl
ENTRYPOINT ["acrawl", "mcp"]
