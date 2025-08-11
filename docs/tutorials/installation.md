# Installation

## Prerequisites

- One or more supported infrastructure provider:
    - Proxmox
    - AWS
    - Oracle Cloud
- Public IP
- One or more domain managed by supported provider:
    - Cloudflare
- S3-compatible bucket to store Terraform state (there's built-in support for Cloudflare R2)
- Nix on the initial controller
