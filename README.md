# Platform Engine

Nothing to see here.

![](https://i.giphy.com/media/joV1k1sNOT5xC/giphy.webp)

If you still wanna see, it's an experimental playground for my other custom PaaS called Netamos
(still private for now, will opensource once ready).

## CLI

CLI talk to the server, they don't mutate GitOps state locally, server do the actual work.

Login:

```sh
netamos login

# or explicitly set the server URL
netamos --server https://netamos.production.khuedoan.com login
```

Config location: `~/.config/netamos/`

Commands work interactive mode or script mode with explicit flags.

```sh
netamos login
netamos logout
netamos whoami
netamos list

netamos create
netamos delete --tenant khuedoan --project blog --environment production --watch
# TODO: add component for existing app environments.
netamos add
netamos status
netamos status --commit HEAD --watch

# TODO: Implement repo workflows.
netamos repo create
netamos repo clone
```
