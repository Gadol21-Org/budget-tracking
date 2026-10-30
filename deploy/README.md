# Deploying to Oracle Cloud

Every push to `main` runs [`.github/workflows/deploy.yml`](../.github/workflows/deploy.yml):

1. The [`Dockerfile`](../Dockerfile) builds the app (frontend embedded in the Rust binary) and pushes it to the repo's private GitHub container registry, `ghcr.io/gadol21-org/budget-tracking`, tagged with the commit.
2. GitHub Actions connects to the VM over SSH and runs [`deploy.sh`](deploy.sh), which pulls the image and replaces the `budget-tracking` container (`--restart unless-stopped`, published on `127.0.0.1:8082` of the VM only). It fails the run, with the last log lines, if the container doesn't stay up for 20 seconds.

Pull requests build the image without pushing or deploying. Deploy by hand from **Actions > Deploy > Run workflow**. The deploy step is skipped until the `ORACLE_HOST` variable is set.

The data lives in an Oracle **MySQL HeatWave** database (Always Free), not on the VM; the app reaches it over the VCN's private network with `MYSQL_URL` (section A). The older Oracle NoSQL backend is still in the code (`STORAGE=nosql`, see its notes at the end of section A). The app's settings come from the `BUDGET_ENV` secret, written to `~/budget-tracking/.env` on the VM before each deploy.

The public https address comes from `cloudflared` on the VM, shared with other services. The app has **no login of its own**, so put **Cloudflare Access** in front of the hostname before exposing it (step 6).

### Ports on this VM

| Port | Service |
|---|---|
| 8080 | (taken by another service on the VM) |
| 8082 | `budget-tracking` (`host_port` in `deploy.sh`) |

## One-time setup

### A. MySQL HeatWave (Oracle Cloud console)

The database has no public address. It lives in the same virtual network as the VM and only the VM can reach it.

1. **Region and network.** Use your home region (top bar). Note the VCN and the subnet the VM uses (Compute > Instances > your VM > Primary VNIC).
2. **Create the DB system.** Databases > MySQL > DB Systems > Create DB system. Choose **Always Free** (shape `MySQL.Free`; Always Free needs the home region and gives about 50 GB, no HeatWave cluster). Set an admin username and password. Under Networking pick the **same VCN** as the VM and the same (or any) subnet. Leave "Enable HeatWave cluster" off. Create it and wait until it is Active.
3. **Copy its private IP** from the DB system's page (Endpoint, e.g. `10.0.1.20`, port 3306).
4. **Allow the VM to connect.** Networking > Virtual cloud networks > your VCN > the DB system's subnet > Security list (or network security group): add an **ingress rule** with source CIDR of the VM's subnet (e.g. `10.0.0.0/24`), protocol TCP, destination port **3306**. If the VM and database share a subnet, that one rule is enough; otherwise also check the VM subnet's egress allows it (the default allows all egress).
5. **Create the database and a user** (from the VM, which can reach it):
   ```bash
   docker run --rm -it mysql:8 mysql -h 10.0.1.20 -u <admin> -p
   ```
   ```sql
   CREATE DATABASE budget CHARACTER SET utf8mb4;
   CREATE USER 'budget'@'%' IDENTIFIED BY '<a long password>';
   GRANT ALL ON budget.* TO 'budget'@'%';
   ```
   The app creates its tables itself on startup.
6. Put the connection string in `BUDGET_ENV` (below). Percent-encode special characters in the password (`#` is `%23`, `@` is `%40`).

Nothing about the database is opened to the internet, and no IAM policy is needed.

#### Only if you use `STORAGE=nosql` instead
Needs a compartment, a dynamic group for the VM and the policy `Allow dynamic-group <group> to manage nosql-family in compartment <name>`, plus `ORACLE_NOSQL_AUTH=instance`, `ORACLE_NOSQL_REGION` and `ORACLE_NOSQL_COMPARTMENT_ID` in `BUDGET_ENV`. Tables are created on startup with 25 read/write units each (the Always Free maximum). Reads scan whole tables, so it is slow.

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

`BUDGET_ENV` holds one `NAME=value` per line, no quotes:

```
STORAGE=mysql
MYSQL_URL=mysql://budget:<password>@10.0.1.20:3306/budget
```

Add `GOOGLE_SERVICE_ACCOUNT_JSON_B64` and `GOOGLE_SHEET_ID` for the nightly Sheets export if you want it. Leave `PAAMONIM_MIRROR` unset (off).

Then merge to `main`, or run the workflow by hand.

## Running it

```bash
docker logs -f budget-tracking     # the app's logs
journalctl -u cloudflared -f       # the tunnel's logs
```

To change a setting, update the secret and run the workflow again; restarting the container doesn't re-read the file.
