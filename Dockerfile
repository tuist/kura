FROM erlang:25-slim AS riak-build

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

FROM elixir:1.19.4-otp-28-slim

ENV MIX_ENV=prod

RUN apt-get update && \
    echo "deb http://deb.debian.org/debian bullseye main" > /etc/apt/sources.list.d/bullseye.list && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
      build-essential \
      git \
      curl \
      ca-certificates \
      tini \
      libssl1.1 \
      libsnappy1v5 \
      libpam0g && \
    rm -f /etc/apt/sources.list.d/bullseye.list && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN mix local.hex --force && mix local.rebar --force

COPY mix.exs mix.lock .formatter.exs ./
COPY config config

RUN mix deps.get --only prod && mix deps.compile

COPY lib lib
COPY priv priv

RUN mix compile

COPY --from=riak-build /src/rel/riak /opt/riak
COPY ops/riak/entrypoint.sh /usr/local/bin/riak-entrypoint
COPY ops/riak/cluster-maintainer.sh /usr/local/bin/riak-cluster-maintainer
COPY ops/container/entrypoint.sh /usr/local/bin/cache-next-entrypoint

RUN chmod +x /usr/local/bin/riak-entrypoint /usr/local/bin/riak-cluster-maintainer /usr/local/bin/cache-next-entrypoint && \
    mkdir -p /opt/riak/data /opt/riak/log

EXPOSE 4000 8087 8098

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/cache-next-entrypoint"]
