# aochatproxy

aochatproxy is a lightweight, fast and reliable chat proxy for Anarchy Online Bots using [nadylib](https://github.com/Nadybot/nadylib).

It allows to have more than 1000 buddies by distributing them among workers and can distribute spam messages over them as well.

## Configuration

Put the following in a file called `config.json`:

```javascript
{
  "rust_log": "info",
  "port_number": 9993,
  "server_address": "chat.d1.funcom.com:7105",
  "spam_bot_support": true,
  "send_tells_over_main": false,
  "relay_worker_tells": true,
  "default_mode": "round-robin",
  "auto_unfreeze_accounts": true,
  "unfreeze_accounts_with_proxy": true,
  "accounts": [
    {
      "username": "myaccount1",
      "password": "mypass1",
      "character": "mychar1"
    },
    {
      "username": "myaccount2",
      "password": "mypass2",
      "character": "mychar2"
    }
  ]
}
```

- `rust_log` configures the logging verbosity. Leave this at `info` for normal use or `debug`/`trace` if you need to see packets
- `port_number` sets the port where the client will be able to connect on
- `server_address` sets the chat server that it will connect to
- `spam_bot_support` toggles support for distributing messages sent via `spam` over workers
- `send_tells_over_main` will define whether distributing these messages will also use the main client
- `relay_worker_tells` toggles relaying tells to the workers to the main
- `relay_by_id` will change the method for choosing a worker for spam messages from round-robin to character IDs

With `spam_bot_support` enabled, at least one worker is required unless `send_tells_over_main` is also enabled.

## Running

If you want to use containers, take a look at the `.env.example` file to get a brief of the enviroment variables used for configuration and create such file, then run it:

```bash
docker run --rm -it --env-file .env quay.io/nadyita/aochatproxy:stable
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
  "version": "5.0.0",
  "type": "capabilities",
  "supported-cmds": ["capabilities", "ping"],
  "rate-limited": true,
  "default-mode": "round-robin",
  "workers": ["charname1", "charname2"],
  "started-at": 5791571957557915,
  "send-modes": [
    "round-robin",
    "by-charid",
    "by-msgid",
    "proxy-default",
    "by-worker"
  ],
  "buddy-modes": ["by-worker"]
}
```

whereas `started-at` is a UNIX timestamp in seconds and `default-mode` any of `send-modes` that will equal `proxy-default`.

### Buddylist

For _each_ worker, the bot will send a LoginOk packet to the client. This means when you have 5 workers, you will get a total of 6 LoginOk packets, including your own. From that, you can calculate the buddylist limit by 6 \* 1000 = 6000. The proxy takes care of the rest.

You may send `{"mode": "by-worker", "worker": 1}` as the routing key on BuddyAdd to force adding a buddy on worker 1 instead of the default choice (lowest buddy count).

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
