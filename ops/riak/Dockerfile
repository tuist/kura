FROM erlang:25-slim AS build

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      build-essential \
      git \
      curl \
      ca-certificates \
      python3 \
      libssl-dev \
      libncurses-dev \
      libsnappy-dev \
      libpam0g-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src

RUN git clone --depth 1 --branch develop https://github.com/OpenRiak/riak.git .
RUN make rel

FROM erlang:25-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      curl \
      ca-certificates \
      libsnappy1v5 \
      libpam0g && \
    rm -rf /var/lib/apt/lists/*

COPY --from=build /src/rel/riak /opt/riak
COPY ops/riak/entrypoint.sh /usr/local/bin/riak-entrypoint

RUN chmod +x /usr/local/bin/riak-entrypoint && \
    mkdir -p /opt/riak/data /opt/riak/log

EXPOSE 8087 8098

ENTRYPOINT ["/usr/local/bin/riak-entrypoint"]
