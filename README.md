# Platform Engine

## Development

```sh
make dev
make test
```

## Project structure

- `src/`
    - `core/`
        - `app/`
            - `source.rs`: can be a git repository or existing image
            - `builder.rs`: build source to image
            - `image.rs`: image that will be deployed
        - `gitops.rs`: control the system via git, changes will be applied by a GitOps engine
    - `activities/`: wrapper for core logic
    - `workflows/`: workflows that will be triggered by the client
    - `bin/`
        - `client.rs`: control plane program to trigger workflows
        - `worker.rs`: data plane program to execute logic
