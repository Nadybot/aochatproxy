# aochatproxy

aochatproxy is a lightweight, fast and reliable chat proxy for Anarchy Online Bots using [nadylib](https://github.com/Nadybot/nadylib).

It allows to have more than 1000 buddies by distributing them among slaves and can distribute spam messages over them as well.

## Configuration

Put the following in a file called `.env`:

```ini
RUST_LOG=info
PROXY_PORT_NUMBER=9993
SERVER_ADDRESS=chat.d1.funcom.com:7105
SPAM_BOT_SUPPORT=true
SEND_TELLS_OVER_MAIN=false
RELAY_SLAVE_TELLS=false

SLAVE1_USERNAME=myslave
SLAVE1_PASSWORD=mypass
SLAVE1_CHARACTERNAME=mychar

SLAVE2_USERNAME=myslave2
SLAVE2_PASSWORD=mypass2
SLAVE2_CHARACTERNAME=mychar2
```

## Running

We provide prebuilt docker images for x86_64, aarch64 (64-bit ARM) and armv7l (32-bit ARM like Raspberry Pi).

Via Docker/Podman:

```bash
docker run --rm -it --env-file .env --init quay.io/nadyita/aochatproxy:rust-rewrite
```

## Implementation for clients

For each slave, the bot will send a LoginOk packet to the client to calculate the amount of buddies that it can have. Whenever a BuddyAdd packet is sent from the client, the proxy will instead send it from the slave or client connection, depending on which has the least buddies. BuddyRemove is handled on all of them.

For outgoing tell messages, they will be proxied over the client by default unless `spam` is used as the routing key instead of `\0`. If spam is enabled for a message and in the config, it will distribute these across all slaves to avoid ratelimits.

When relaying tells from slaves to the client in the config, the routing key will be `N` where N is the ID of the slave. This can be used to send `spam-N` as the outgoing key to send it over a specific slave.
