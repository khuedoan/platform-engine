# Platform Engine

Nothing to see here.

![](https://i.giphy.com/media/joV1k1sNOT5xC/giphy.webp)

If you still wanna see, it's an experimental playground for my other custom PaaS called Netamos
(still private for now, will opensource once ready).

## CLI

CLI talk to the server, they don't mutate GitOps state locally, server do the actual work.

Login:

```sh
netamos

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
# TODO: Prompt/filter by tenant when that UX is added.

# TODO: Prompt for missing create values instead of requiring every flag.
netamos create \
  --tenant khuedoan \
  --project blog \
  --environment production \
  --source-repo khuedoan/blog \
  --port 8080 \
  --service \
  --hostname www.khuedoan.com
netamos delete --tenant khuedoan --project blog --environment production --watch
netamos deploy --repo khuedoan/blog --revision HEAD --environment production --watch
netamos status
netamos status --commit HEAD --watch
netamos open push-to-deploy-blog-<sha>

# TODO: Implement repo workflows.
netamos repo create
netamos repo clone

# TODO: add component for existing app environments.
netamos add
```
