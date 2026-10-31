# Staging environment

A second copy of the app on the same Oracle VM, for showing the app to others and for trying a
branch before it is merged to `main`. It holds **made-up sample data only**.

| | Production | Staging |
|---|---|---|
| Container | `budget-tracking` | `budget-staging` |
| Port on the VM | `127.0.0.1:8082` (via Cloudflare tunnel) | `0.0.0.0:8083` |
| Data | Oracle MySQL | SQLite in Docker volume `budget-staging-data` |
| Google Sheets / Paamonim | per `BUDGET_ENV` | **off** (no credentials are passed in, mirroring forced off) |

Staging is started without `--env-file`, so it cannot see the production MySQL URL, the Sheets key or the Paamonim login.

## Deploy a branch

Actions > **Staging** > Run workflow > choose the branch. The URL is in the run summary: `http://<ORACLE_HOST>:8083`.

From a thread, with the GitHub MCP tools:

```
actions_run_trigger  method=run_workflow  workflow_id=staging.yml  ref=<branch>  inputs={"reset":"false"}
```

Then watch the run with `actions_get` / `actions_list` until it succeeds, and give Omer the URL. Deploys replace the previous staging container, one at a time (the last one wins); the data is kept between deploys.

## Reset / reseed

Run the workflow with `reset` = true (`inputs={"reset":"true"}`). It deletes the staging volume and loads the sample data again ([`seed-staging.sh`](seed-staging.sh): categories, six months of transactions up to today, one repeating charge). Seeding only happens into an empty database, so a normal deploy never overwrites what was entered by hand.

## One-time setup (needs Omer)

The workflow reuses the production secrets and variables (`ORACLE_SSH_KEY`, `ORACLE_KNOWN_HOSTS`, `ORACLE_HOST`, `ORACLE_USER`, `BUILD_RUNNER`). It needs no new secret. What has to be opened by hand:

1. **Oracle security list:** Networking > VCN > the VM's subnet > Security list > add an ingress rule: source `0.0.0.0/0` (or just your IPs), TCP, destination port **8083**.
2. **VM firewall** (Oracle's Ubuntu images block it by default):
   ```
   sudo iptables -I INPUT 6 -m state --state NEW -p tcp --dport 8083 -j ACCEPT
   sudo netfilter-persistent save
   ```
3. **Alternative to 1-2:** add a second Cloudflare tunnel hostname (e.g. `staging.example.com` -> `http://localhost:8083`) with Cloudflare Access in front. Then no port needs opening. For that, change `0.0.0.0:` to `127.0.0.1:` in `deploy-staging.sh`.

The app has no login, so anyone who knows `http://<ORACLE_HOST>:8083` can use it. That is acceptable only because it holds sample data; never put real data in staging.
