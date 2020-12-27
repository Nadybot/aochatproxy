# aochatproxy

aochatproxy is a lightweight, fast and reliable chat proxy for Anarchy Online Bots using [nadylib](https://github.com/Nadybot/nadylib).

It allows to have more than 1000 buddies by distributing them among workers and can distribute spam messages over them as well.

## Configuration

Put the following in a file called `.env`:

```ini
RUST_LOG=info
PROXY_PORT_NUMBER=9993
SERVER_ADDRESS=chat.d1.funcom.com:7105
SPAM_BOT_SUPPORT=true
SEND_TELLS_OVER_MAIN=false
RELAY_WORKER_TELLS=false
RELAY_BY_ID=false

WORKER1_USERNAME=myworker
WORKER1_PASSWORD=mypass
WORKER1_CHARACTERNAME=mychar

WORKER2_USERNAME=myworker2
WORKER2_PASSWORD=mypass2
WORKER2_CHARACTERNAME=mychar2
```

- `RUST_LOG` configures the logging verbosity. Leave this at `info` for normal use or `debug`/`trace` if you need to see packets
- `PROXY_PORT_NUMBER` sets the port where the client will be able to connect on
- `SERVER_ADDRESS` sets the chat server that it will connect to
- `SPAM_BOT_SUPPORT` toggles support for distributing messages sent via `spam` over workers
- `SEND_TELLS_OVER_MAIN` will define whether distributing these messages will also use the main client
- `RELAY_WORKER_TELLS` toggles relaying tells to the workers to the main
- `RELAY_BY_ID` will change the method for choosing a worker for spam messages from round-robin to character IDs

With `SPAM_BOT_SUPPORT` enabled, at least one worker is required unless `SEND_TELLS_OVER_MAIN` is also enabled.

## Running

Via Docker/Podman:

```bash
docker run --rm -it --env-file .env quay.io/nadyita/aochatproxy:rust-rewrite
```

Release binaries:

Head [here](https://github.com/Nadybot/aochatproxy/releases/latest) and download the binary for your system. Mark it as executable (Shell: `chmod +x aochatproxy-xy`) and then run directly (Shell: `./aochatproxy-xy`). Make sure to have the config set up before.

## Implementation for clients

### Handshake

Optionally, the bot connected may send a `Ping` packet to the proxy with a body of `{"cmd": "capabilities"}` to determine whether the proxy supports legacy of futureproof messaging.

aochatproxy currently returns this (in another `Ping` packet, with different values possible in default-mode, workers and started-at):

```json
{
  "name": "aochatproxy",
  "version": "0.1.0",
  "default-mode": "round-robin",
  "workers": ["charname1", "charname2"],
  "started-at": 57915719575,
  "send_modes": [
    "round-robin",
    "by-charid",
    "by-msgid",
    "proxy-default",
    "by-worker"
  ]
}
```

whereas `started-at` is a UNIX timestamp in seconds and `default-mode` any of `send_modes` that will equal `proxy-default`.

### Buddylist

For _each_ worker, the bot will send a LoginOk packet to the client. This means when you have 5 workers, you will get a total of 6 LoginOk packets, including your own. From that, you can calculate the buddylist limit by 6 \* 1000 = 6000. The proxy takes care of the rest.

### Spam Messages

Spam messages can use two formats: _Legacy_ or the extensible "futureproof" one with more features.

In any case, when `RELAY_WORKER_TELLS` is enabled, the routing key for messages from workers will be `{"id": 1, "name": "mychar1"}`, which is especially useful for the futureproof format.

#### Legacy

The legacy format uses `spam` as a routing key in the MsgPrivate packets instead of `\0`. It will use whatever is configured on the proxy to distribute them and the client has no further work to do. This only allows for relaying by character ID or round-robin.

#### Futureproof

The modern format uses JSON instead of plaintext in the routing key to enable a multitude of possible relaying options.

```json
{
  "mode": "round-robin",
  "msgid": 1,
  "worker": 1
}
```

`mode` is **required** and has the following options: `round-robin`, `by-charid`, `by-msgid`, `proxy-default`, `by-worker`.

`msgid` is required if `by-msgid` is set as the mode.

`worker` is required if `by-worker` is set as the mode.

The mode configures how the proxy should relay the message across workers. `round-robin` and `by-charid` are the two proxy-native versions and _should_ not be used, instead, `proxy-default` should be preferred. `by-msgid` allows for sending multiple messages (for example a `!config`) with an internal counter so paging messages will arrive from the same worker. `by-worker` will use the worker specified in the payload, which is useful when relaying tells from workers and you want to reply from this worker.

### Ecosystem Support

To be extended.

| Bot                                           | Buddylist | Legacy tells | Modern tells |
| --------------------------------------------- | --------- | ------------ | ------------ |
| [Nadybot](https://github.com/Nadybot/Nadybot) | Yes       | Yes          | Yes          |
| [Tyrbot](https://github.com/Budabot/Tyrbot)   | Yes       | No           | No           |
