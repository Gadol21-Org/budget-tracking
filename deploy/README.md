# Deploying to Oracle Cloud

Every push to `main` runs [`.github/workflows/deploy.yml`](../.github/workflows/deploy.yml):

1. The [`Dockerfile`](../Dockerfile) builds the app (frontend embedded in the Rust binary) and pushes it to the repo's private GitHub container registry, `ghcr.io/gadol21-org/budget-tracking`, tagged with the commit.
2. GitHub Actions connects to the VM over SSH and runs [`deploy.sh`](deploy.sh), which pulls the image and replaces the `budget-tracking` container (`--restart unless-stopped`, published on `127.0.0.1:8082` of the VM only). It fails the run, with the last log lines, if the container doesn't stay up for 20 seconds.

Pull requests build the image without pushing or deploying. Deploy by hand from **Actions > Deploy > Run workflow**. The deploy step is skipped until the `ORACLE_HOST` variable is set.

The data lives in Oracle NoSQL, not on the VM. The container authenticates to it with the VM's **instance principal** (`ORACLE_NOSQL_AUTH=instance`), so there is no API key or OCI config file anywhere. The app's settings come from the `BUDGET_ENV` secret, written to `~/budget-tracking/.env` on the VM before each deploy.

The public https address comes from `cloudflared` on the VM, shared with other services. The app has **no login of its own**, so put **Cloudflare Access** in front of the hostname before exposing it (step 6).

### Ports on this VM

| Port | Service |
|---|---|
| 8080 | (taken by another service on the VM) |
| 8082 | `budget-tracking` (`host_port` in `deploy.sh`) |

## One-time setup

### A. Oracle NoSQL and IAM (Oracle Cloud console)

1. **Compartment.** Identity & Security > Compartments > Create compartment, e.g. `budget`, under the root. Copy its OCID (`ocid1.compartment...`) for `ORACLE_NOSQL_COMPARTMENT_ID`. (The root compartment works too, but a dedicated one keeps the policy narrow.)
2. **Region.** Use your home region (top bar). Always Free NoSQL tables must be created there. Note its identifier, e.g. `il-jerusalem-1`, for `ORACLE_NOSQL_REGION`.
3. **Tables.** Nothing to create by hand. On startup the app runs `create table if not exists` for `transactions` and `lookups` (10 read units, 10 write units, 1 GB each), leaving one free under the Always Free limit of three. To create them yourself instead: Databases > NoSQL Database > Tables > Create table, in the `budget` compartment, using DDL input with the statements in `backend/src/nosql.rs`, capacity mode provisioned 10/10/1 GB.
4. **Dynamic group** (Identity & Security > Domains > your domain > Dynamic groups) named `budget-vm`, with this rule (use the instance's OCID, or match the compartment it lives in):
   ```
   ANY {instance.id = 'ocid1.instance.oc1...'}
   ```
5. **Policy** (Identity & Security > Policies, in the root compartment) named `budget-nosql`:
   ```
   Allow dynamic-group budget-vm to manage nosql-family in compartment budget
   ```
   If the group is in a non-default identity domain, write it as `dynamic-group <domain>/budget-vm`. `manage` is needed because the app creates its tables on startup.
6. No NoSQL "endpoint" needs exposing: the app talks to the regional service endpoint over the VM's normal outbound internet access. Nothing about the database is opened to the internet.

### B. VM and tunnel

If you already have a VM with Docker and a tunnel, reuse it and skip to 4.

1. Create a compute instance with the **Ubuntu 24.04** image (Always Free `VM.Standard.A1.Flex` arm64, or `VM.Standard.E2.1.Micro` AMD with `BUILD_RUNNER=ubuntu-24.04`). Paste your SSH public key.
2. Install Docker: `scp deploy/setup-vm.sh ubuntu@<vm-ip>: && ssh ubuntu@<vm-ip> ./setup-vm.sh`, then log out and back in.
3. Install `cloudflared` as a system service from Cloudflare dashboard > Networks > Tunnels (an existing tunnel can be reused).
4. Add a **public hostname** to the tunnel, e.g. `budget.example.com`, service HTTP, URL `localhost:8082`.
5. Create a deploy key and add it to the VM (`ssh-keygen -t ed25519 -N "" -C github-deploy -f oracle-deploy`, `ssh-copy-id -i oracle-deploy.pub ubuntu@<vm-ip>`, `ssh-keyscan <vm-ip>`).
6. **Protect the hostname.** Cloudflare Zero Trust > Access > Applications > Add a self-hosted application for `budget.example.com`, with a policy that allows only your email (one-time PIN or Google login). Without this anyone who finds the URL can read and edit your transactions.

### C. GitHub settings

In this repo, **Settings > Secrets and variables > Actions**:

| Kind | Name | Value |
|---|---|---|
| Secret | `ORACLE_SSH_KEY` | The private deploy key. |
| Secret | `ORACLE_KNOWN_HOSTS` | Optional: the `ssh-keyscan` output. |
| Secret | `BUDGET_ENV` | The app's `.env`, see below. |
| Variable | `ORACLE_HOST` | The VM's public IP. Deploys are skipped until it is set. |
| Variable | `ORACLE_USER` | Optional, default `ubuntu`. |
| Variable | `BUILD_RUNNER` | Optional, default `ubuntu-24.04-arm` (builds natively for an arm64 VM). Use `ubuntu-24.04` for an AMD VM. |

`BUDGET_ENV` holds one `NAME=value` per line. Write values without quotes (a `#` inside a value is fine, e.g. `PAAMONIM_PASSWORD=pass#`); the workflow strips Windows line endings and one pair of surrounding quotes, because `docker --env-file` would otherwise pass them to the app as part of the value:

```
STORAGE=nosql
ORACLE_NOSQL_AUTH=instance
ORACLE_NOSQL_REGION=il-jerusalem-1
ORACLE_NOSQL_COMPARTMENT_ID=ocid1.compartment.oc1..xxxx
```

Add `GOOGLE_SERVICE_ACCOUNT_JSON_B64` and `GOOGLE_SHEET_ID` for the nightly Sheets export if you want it. Leave `PAAMONIM_MIRROR` unset (off).

Then merge to `main`, or run the workflow by hand.

## Running it

```bash
docker logs -f budget-tracking     # the app's logs
journalctl -u cloudflared -f       # the tunnel's logs
```

To change a setting, update the secret and run the workflow again; restarting the container doesn't re-read the file.
